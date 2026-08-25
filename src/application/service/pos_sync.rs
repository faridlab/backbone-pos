//! The offline reconciliation verb (`sync_from_ui`) (hand-authored, user-owned).
//!
//! An `impl PosWriteService` chunk over the vocabulary in [`super::pos_write_service`]. An offline
//! client (register app that sold without connectivity) replays a ticket here; the server is the
//! book of record and TRUSTS THE REPLAY FOR IDENTITY ONLY:
//!
//! - **Identity** is the payload's `client_uuid` — a replay whose uuid already names a live ticket
//!   UPDATES that ticket (while it is still draft) instead of creating a second one. A finalized
//!   ticket short-circuits as [`SyncAction::ReplayFinalized`] — the server's state wins, nothing is
//!   rewritten.
//! - **Money** is ALWAYS re-derived: the replay's lines + tenders run through the SAME compute core
//!   as `ring_sale` ([`super::pos_compute`]: register templates → document-grade tax →
//!   register-config cash rounding), and the tender sum implies paid/change. Any client-claimed
//!   total was discarded at the DTO boundary — this verb never sees one.
//! - **Partner + session are validated, not trusted**: a replay cannot re-assign a ticket's
//!   customer, and it cannot ring under a session that has since closed unless it names an open
//!   RESCUE session (the typed refusal [`PosError::SessionClosedRescueRequired`] is the
//!   rescue-or-refuse contract: the client either re-attributes the ticket to the new shift or
//!   surfaces the mismatch to a human).
//! - **Refund lineage is single-parent**: one refund names one parent (`refund_of_client_uuid`);
//!   re-pointing a refund at a DIFFERENT parent than it was first recorded against is refused
//!   ([`PosError::RefundLineageConflict`]), and a parent that does not resolve refuses
//!   ([`PosError::RefundParentNotFound`]). A refund replay is a privileged mutation — the manager
//!   PIN is verified first.
//!
//! Per the module's 4-layer rule this file holds no SQL — the statements live on
//! `PosInvoiceRepository` / `PosInvoiceItemRepository` / `PosPaymentRepository` /
//! `PosOpeningEntryRepository` / `PosProfileRepository`.

use backbone_orm::company_scope;
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::infrastructure::persistence::{NewDraftInvoiceRow, NewInvoiceItemRow, NewTenderRow};

use super::pos_compute::ComputeLineIn;
use super::pos_events::{PosEvent, PosTenderCompleted};
use super::pos_ports::{BillingPort, PaymentPort, PosTaxComputePort, PosTaxDocumentType};
use super::pos_write_service::{
    map_invoice_dup, money, NewSyncSale, PosError, PosWriteService, SyncAction, SyncOutcome, TicketTotals,
};

impl PosWriteService {
    /// Replay one offline ticket. See the module doc for the identity/trust contract.
    /// `tax` resolves the document-grade compute (as on `ring_sale`); `billing`/`payment` are only
    /// touched when the replay is a REFUND (they drive the same reversal pair `return_sale` does).
    pub async fn sync_from_ui(
        &self,
        s: NewSyncSale,
        tax: &dyn PosTaxComputePort,
        billing: &dyn BillingPort,
        payment: &dyn PaymentPort,
    ) -> Result<SyncOutcome, PosError> {
        let company = s.company_id;
        company_scope::with_company_scope(Some(company), async move {
            // Identity first: does a live ticket already carry this client uuid?
            if let Some(existing) = self.invoices.find_by_client_uuid(&self.db_pool, s.client_uuid, company).await? {
                // Finalized server-side (paid/returned): the replay changes nothing. The server's
                // persisted money is the answer.
                if existing.status != "draft" {
                    return Ok(SyncOutcome {
                        pos_invoice_id: existing.id,
                        action: SyncAction::ReplayFinalized,
                        totals: TicketTotals {
                            net_total: existing.net_total,
                            tax_total: existing.tax_total,
                            grand_total: existing.grand_total,
                            rounding_adjustment: existing.rounding_adjustment,
                            rounded_total: existing.rounded_total,
                            paid_total: existing.paid_total,
                            change_due: existing.change_due,
                        },
                    });
                }
                return self.sync_update_draft(s, existing, tax).await;
            }

            // No live ticket carries the uuid. A refund replay never creates a draft — it drives the
            // (privileged, idempotent) full-return flow against its parent.
            if let Some(parent_uuid) = s.refund_of_client_uuid {
                // Privileged: the manager PIN is verified BEFORE anything is read or written.
                let manager = s.manager.as_ref().ok_or(PosError::ManagerAuthRequired)?;
                self.verify_manager_internal(company, manager, s.source_ip.as_deref()).await?;
                let parent = self.invoices
                    .find_by_client_uuid(&self.db_pool, parent_uuid, company)
                    .await?
                    .ok_or(PosError::RefundParentNotFound(parent_uuid))?;
                // The replay's own totals are discarded by contract: a full refund reverses the
                // parent's persisted money, whatever the client says it handed back.
                let out = self
                    .return_sale(parent.id, billing, payment, None)
                    .await?;
                // return_sale is idempotent on the parent's paid→returned flip; whether THIS call
                // recorded the return ticket or replayed an existing one is observable from the
                // parent's state we already read.
                let action = if parent.status == "paid" { SyncAction::Created } else { SyncAction::ReplayFinalized };
                return Ok(SyncOutcome {
                    pos_invoice_id: out.return_ticket_id,
                    action,
                    totals: TicketTotals {
                        net_total: parent.net_total,
                        tax_total: parent.tax_total,
                        grand_total: parent.grand_total,
                        rounding_adjustment: Decimal::ZERO,
                        rounded_total: parent.rounded_total,
                        paid_total: parent.rounded_total,
                        change_due: Decimal::ZERO,
                    },
                });
            }

            self.sync_create_draft(s, tax).await
        })
        .await
    }

    /// The CREATE half: a fresh offline ticket, rung through the shared compute core into a draft
    /// carrying the sync identity.
    async fn sync_create_draft(&self, s: NewSyncSale, tax: &dyn PosTaxComputePort) -> Result<SyncOutcome, PosError> {
        // Session validation (rescue-or-refuse): the session the client rang under must be open, or
        // the replay must name an open rescue session to attribute the ticket to.
        let session = self.resolve_session_for_create(&s).await?;
        // Tender methods are validated against the enum up front — a typo'd method is a typed 422,
        // not a DB cast failure mid-transaction.
        self.validate_tender_methods(&s).await?;
        // Restaurant lane — seating, same contract as the online ring: the named table must exist
        // in this tenant and hold no other draft (the DB partial unique is the race backstop).
        if let Some(table) = s.pos_table_id {
            if self.tables.fetch_table(&self.db_pool, table, s.company_id).await?.is_none() {
                return Err(PosError::TableNotFound(table));
            }
            if let Some(occupant) = self
                .invoices
                .find_draft_on_table(&self.db_pool, table, s.company_id, None)
                .await?
            {
                return Err(PosError::TableOccupied { pos_table_id: table, draft_invoice_id: occupant });
            }
        }
        // Order-level discount: rate resolved from the tenant's master, folded before the compute.
        let order_discount = self
            .resolve_order_discount(s.company_id, s.pos_profile_id, s.discount_id)
            .await?;

        let mut compute_lines: Vec<ComputeLineIn> = s
            .lines
            .iter()
            .map(|l| ComputeLineIn {
                item_id: l.item_id,
                revenue_account_id: l.revenue_account_id,
                description: l.description.clone(),
                course: l.course,
                quantity: l.quantity,
                unit_price: l.unit_price,
                discount_amount: l.discount_amount,
            })
            .collect();
        if let Some(d) = &order_discount {
            Self::fold_order_discount(&mut compute_lines, d.percentage)?;
        }
        let computed = self
            .compute_ticket(
                s.company_id,
                s.pos_profile_id,
                s.posting_at.date(),
                PosTaxDocumentType::Invoice,
                compute_lines,
                tax,
            )
            .await?;
        let paid = s.tenders.iter().map(|t| t.amount).sum::<Decimal>();
        let totals = Self::totals_with_tenders(&computed, paid);

        // The server mints the receipt number, deterministically from the sync identity (company
        // fragment + uuid fragment): a retried create collides on it (and on the uuid) instead of
        // double-ringing, while two tenants replaying the same device uuid never collide — the
        // receipt-number unique index is global.
        let receipt_number = format!(
            "SYNC-{}-{}",
            &s.company_id.simple().to_string()[..4],
            &s.client_uuid.simple().to_string()[..12]
        );
        let id = Uuid::new_v4();
        let mut tx = self.db_pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let r = self.invoices.insert_draft(&mut tx, &NewDraftInvoiceRow {
            id,
            company_id: s.company_id,
            client_uuid: Some(s.client_uuid),
            pos_profile_id: s.pos_profile_id,
            opening_entry_id: session,
            branch_id: s.branch_id,
            customer_id: s.customer_id,
            pos_table_id: s.pos_table_id,
            receipt_number: &receipt_number,
            posting_at: s.posting_at,
            net_total: computed.net_total,
            tax_total: computed.tax_total,
            grand_total: computed.grand_total,
            rounding_adjustment: computed.rounding_adjustment,
            rounded_total: computed.rounded_total,
        }).await;
        if let Err(e) = r {
            return Err(map_invoice_dup(
                e, &receipt_number, Some(s.client_uuid), s.pos_table_id, s.company_id,
                &self.invoices, &self.db_pool, None,
            ).await);
        }
        self.sync_write_children(&mut tx, id, s.company_id, &computed, &s).await?;
        // The replay can carry tenders from the first moment (the offline client already took
        // payment). Re-sum what was just written onto the header so paid/change can never disagree
        // with the tender rows — same contract `add_tender` holds on the online path.
        let paid_total = self.payments.sum_paid_total_on(&mut tx, id).await?;
        let change_due = if paid_total > computed.rounded_total {
            paid_total - computed.rounded_total
        } else {
            Decimal::ZERO
        };
        self.invoices.update_tender_totals(&mut tx, id, paid_total, change_due).await?;
        tx.commit().await?;
        self.emit_sync_events(id, s.company_id, totals.rounded_total, paid_total).await;
        Ok(SyncOutcome {
            pos_invoice_id: id,
            action: SyncAction::Created,
            totals: TicketTotals { paid_total, change_due, ..totals },
        })
    }

    /// The UPDATE half: rewrite a live DRAFT ticket from its replay (full-snapshot semantics — the
    /// replay carries the complete basket + tender set). Partner, session, and refund lineage are
    /// validated before anything is written.
    async fn sync_update_draft(
        &self,
        s: NewSyncSale,
        existing: crate::infrastructure::persistence::SyncLookupRow,
        tax: &dyn PosTaxComputePort,
    ) -> Result<SyncOutcome, PosError> {
        // Partner is validated, never re-assigned silently.
        if existing.customer_id != s.customer_id {
            return Err(PosError::SyncPartnerMismatch);
        }
        // Refund lineage: a ticket that was never a refund cannot become one on replay (the refund
        // path is the privileged full-return flow, not a draft rewrite), and a refund ticket cannot
        // be re-pointed at a different parent.
        if existing.is_return || existing.return_against.is_some() || s.refund_of_client_uuid.is_some() {
            return Err(PosError::RefundLineageConflict);
        }
        // Session: equal names must both be open; a different name is only honored when the
        // ORIGINAL session closed and the payload's session (or named rescue) is open — and the
        // target must be the same register's session.
        let session = self.resolve_session_for_update(&s, &existing).await?;
        self.validate_tender_methods(&s).await?;
        // Restaurant lane — table TRANSFER: the replay's table is honored as the ticket's new seat
        // (diners move; a draft is not pinned to its birth table the way it is to its register).
        // The named table must exist, and if it differs from the current seat it must hold no
        // OTHER draft — the occupancy check excludes THIS ticket.
        if let Some(table) = s.pos_table_id {
            if self.tables.fetch_table(&self.db_pool, table, s.company_id).await?.is_none() {
                return Err(PosError::TableNotFound(table));
            }
            if Some(table) != existing.pos_table_id {
                if let Some(occupant) = self
                    .invoices
                    .find_draft_on_table(&self.db_pool, table, s.company_id, Some(existing.id))
                    .await?
                {
                    return Err(PosError::TableOccupied { pos_table_id: table, draft_invoice_id: occupant });
                }
            }
        }
        // Order-level discount: rate resolved from the tenant's master, folded before the compute.
        let order_discount = self
            .resolve_order_discount(s.company_id, existing.pos_profile_id, s.discount_id)
            .await?;

        // The ticket keeps the register it was born on — its tax + rounding config — even when the
        // session is rescued to a new shift of that same register.
        let mut compute_lines: Vec<ComputeLineIn> = s
            .lines
            .iter()
            .map(|l| ComputeLineIn {
                item_id: l.item_id,
                revenue_account_id: l.revenue_account_id,
                description: l.description.clone(),
                course: l.course,
                quantity: l.quantity,
                unit_price: l.unit_price,
                discount_amount: l.discount_amount,
            })
            .collect();
        if let Some(d) = &order_discount {
            Self::fold_order_discount(&mut compute_lines, d.percentage)?;
        }
        let computed = self
            .compute_ticket(
                s.company_id,
                existing.pos_profile_id,
                s.posting_at.date(),
                PosTaxDocumentType::Invoice,
                compute_lines,
                tax,
            )
            .await?;
        let paid = s.tenders.iter().map(|t| t.amount).sum::<Decimal>();
        let totals = Self::totals_with_tenders(&computed, paid);

        let id = existing.id;
        let mut tx = self.db_pool.begin().await?;
        company_scope::bind_current_company(&mut tx).await?;
        let now = chrono::Utc::now();
        // Full snapshot: retire the prior lines + tenders (soft delete — audit trail kept, sync
        // uuid uniqueness freed), then write the replay's.
        self.items.soft_delete_lines_for_ticket(&mut tx, id, now).await?;
        self.payments.soft_delete_tenders_for_ticket(&mut tx, id, now).await?;
        let r = self.invoices
            .update_draft_from_sync(
                &mut tx, id, session, s.branch_id, s.customer_id, s.pos_table_id,
                computed.net_total, computed.tax_total, computed.grand_total,
                computed.rounding_adjustment, computed.rounded_total,
                totals.paid_total, totals.change_due,
            )
            .await;
        if let Err(e) = r {
            // A unique violation here is the table partial unique racing the occupancy pre-check
            // (a transfer onto a table another draft just claimed) — map it to the typed 409.
            return Err(map_invoice_dup(
                e, "", Some(s.client_uuid), s.pos_table_id, s.company_id,
                &self.invoices, &self.db_pool, Some(id),
            ).await);
        }
        self.sync_write_children(&mut tx, id, s.company_id, &computed, &s).await?;
        tx.commit().await?;
        self.emit_sync_events(id, s.company_id, totals.rounded_total, totals.paid_total).await;
        Ok(SyncOutcome { pos_invoice_id: id, action: SyncAction::Updated, totals })
    }

    /// Write the replay's lines + tenders onto a ticket inside the caller's transaction. The tenders
    /// re-sum to the header's paid/change (they were computed from the same sum), so the header and
    /// its tender lines can never disagree.
    async fn sync_write_children(
        &self,
        tx: &mut sqlx::PgConnection,
        pos_invoice_id: Uuid,
        company_id: Uuid,
        computed: &super::pos_compute::ComputedTicket,
        s: &NewSyncSale,
    ) -> Result<(), PosError> {
        for (l, src) in computed.lines.iter().zip(&s.lines) {
            self.items.insert_line(tx, &NewInvoiceItemRow {
                id: Uuid::new_v4(),
                company_id,
                pos_invoice_id,
                client_uuid: Some(src.client_uuid),
                item_id: l.item_id,
                description: l.description.as_deref(),
                course: l.course,
                quantity: l.quantity,
                unit_price: l.unit_price,
                discount_amount: l.discount_amount,
                net_amount: l.net_amount,
                revenue_account_id: l.revenue_account_id,
            }).await?;
        }
        for t in &s.tenders {
            if t.amount <= Decimal::ZERO {
                return Err(PosError::NegativeAmount);
            }
            self.payments.insert_tender(tx, &NewTenderRow {
                id: Uuid::new_v4(),
                company_id,
                pos_invoice_id,
                client_uuid: Some(t.client_uuid),
                payment_method: &t.method,
                amount: money(t.amount),
                reference_no: t.reference_no.as_deref(),
            }).await?;
        }
        Ok(())
    }

    /// Emit recognition triggers for a replay that (now) crosses full payment — the SAME contract as
    /// `add_tender`: durable outbox staging first (when configured), fire-and-forget sink second.
    /// Double delivery is safe: recognition is replay-safe end to end.
    async fn emit_sync_events(&self, pos_invoice_id: Uuid, company_id: Uuid, rounded_total: Decimal, paid_total: Decimal) {
        if paid_total >= rounded_total && rounded_total > Decimal::ZERO {
            if let Some(schema) = &self.outbox_schema {
                let rec = super::pos_tender::tender_completed_outbox_record(pos_invoice_id, company_id, chrono::Utc::now());
                if let Ok(mut tx) = self.db_pool.begin().await {
                    if backbone_outbox::outbox::stage(&mut *tx, schema, &rec).await.is_ok() {
                        let _ = tx.commit().await;
                    }
                }
            }
            self.sink.publish(PosEvent::PosTenderCompleted(PosTenderCompleted {
                pos_invoice_id, company_id,
            }));
        }
    }

    /// CREATE-session resolution: the named session must be open, or closed WITH an open rescue.
    /// Refuses with the typed rescue-refusal otherwise.
    async fn resolve_session_for_create(&self, s: &NewSyncSale) -> Result<Uuid, PosError> {
        let named = self.openings
            .fetch_status(&self.db_pool, s.opening_entry_id, s.company_id)
            .await?
            .ok_or(PosError::SessionNotFound(s.opening_entry_id))?;
        if named == "open" {
            // Open, but still the ticket's OWN register: the plain-open path owes the same
            // register pairing the rescue arms enforce — no mixing configs and drawers.
            return self.same_register_open_session(s.opening_entry_id, s.pos_profile_id, s.company_id).await;
        }
        // Closed: honor the rescue only when it is an OPEN session on the SAME register the ticket
        // names — a rescue onto another register's drawer would move money between tills.
        if let Some(rescue) = s.rescue_opening_entry_id {
            return self.same_register_open_session(rescue, s.pos_profile_id, s.company_id).await;
        }
        Err(PosError::SessionClosedRescueRequired(s.opening_entry_id))
    }

    /// UPDATE-session resolution: an unchanged name must be open; a changed name is honored only
    /// when the ORIGINAL closed and the new session is open on the SAME register as the ticket.
    async fn resolve_session_for_update(
        &self,
        s: &NewSyncSale,
        existing: &crate::infrastructure::persistence::SyncLookupRow,
    ) -> Result<Uuid, PosError> {
        if s.opening_entry_id == existing.opening_entry_id {
            let st = self.openings
                .fetch_status(&self.db_pool, existing.opening_entry_id, s.company_id)
                .await?
                .ok_or(PosError::SessionNotFound(existing.opening_entry_id))?;
            if st != "open" {
                // The original closed between the create replay and this one: fall through to the
                // rescue rules below using the payload's rescue, not a silent re-attribution.
                if let Some(rescue) = s.rescue_opening_entry_id {
                    return self.same_register_open_session(rescue, existing.pos_profile_id, s.company_id).await;
                }
                return Err(PosError::SessionClosedRescueRequired(existing.opening_entry_id));
            }
            return Ok(existing.opening_entry_id);
        }
        // Different session name: only a CLOSED original + an OPEN target on the ticket's register.
        let orig = self.openings
            .fetch_status(&self.db_pool, existing.opening_entry_id, s.company_id)
            .await?
            .ok_or(PosError::SessionNotFound(existing.opening_entry_id))?;
        if orig == "open" {
            return Err(PosError::SyncSessionMismatch);
        }
        let target = s.rescue_opening_entry_id.unwrap_or(s.opening_entry_id);
        self.same_register_open_session(target, existing.pos_profile_id, s.company_id).await
    }

    /// The rescue must be an OPEN session on the ticket's own register — a rescue onto another
    /// register's drawer would move money between tills.
    async fn same_register_open_session(
        &self,
        session: Uuid,
        pos_profile_id: Uuid,
        company_id: Uuid,
    ) -> Result<Uuid, PosError> {
        let st = self.openings
            .fetch_status(&self.db_pool, session, company_id)
            .await?
            .ok_or(PosError::SessionNotFound(session))?;
        if st != "open" {
            return Err(PosError::SessionClosedRescueRequired(session));
        }
        let profile = self.openings
            .fetch_profile_id(&self.db_pool, session, company_id)
            .await?
            .ok_or(PosError::SessionNotFound(session))?;
        if profile != pos_profile_id {
            return Err(PosError::SessionRegisterMismatch);
        }
        Ok(session)
    }

    /// Every replayed tender method must be a live enum variant.
    async fn validate_tender_methods(&self, s: &NewSyncSale) -> Result<(), PosError> {
        let valid = self.payments.valid_methods(&self.db_pool).await?;
        for t in &s.tenders {
            if !valid.iter().any(|m| m == &t.method) {
                return Err(PosError::InvalidTenderMethod(t.method.clone()));
            }
        }
        Ok(())
    }
}
