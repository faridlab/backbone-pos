//! The server-owned ticket computation (hand-authored, user-owned).
//!
//! An `impl PosWriteService` chunk over the vocabulary in [`super::pos_write_service`]. This is the
//! ONE place a ticket's money is derived — `ring_sale`, `ring_sale_priced`, and the offline
//! `sync_from_ui` all route through [`Self::compute_ticket`], so an online ticket and its offline
//! replay can never price differently.
//!
//! The contract, in order:
//!
//! 1. **Client inputs are inputs only.** `unit_price`/`quantity`/`discount_amount` (and the promo
//!    pricer's resolved nets) feed the computation; every total is derived here. No client-supplied
//!    tax, grand total, or rounding step is ever read.
//! 2. **Tax is document-grade.** The register's `tax_template_ids` (NOT the retired flat `tax_rate`
//!    column) are expanded against the lines and resolved through the `PosTaxComputePort`. A register
//!    with no templates refuses the ring outright — a zero-rated template is how a non-PKP register
//!    expresses itself. Zero Cargo edge to the tax module lives here: the port is the only resolver.
//! 3. **The port's nets win.** `PosTaxComputeResult::net_amounts` OVERWRITES POS's own per-line
//!    rounding — a globally-rounding tax policy redistributes per-line cents so the journal balances,
//!    and adopting those nets is what keeps Σ lines == header net.
//! 4. **Cash rounding is register config.** `cash_rounding_strategy` + `cash_rounding_unit` on the
//!    profile decide the pay-to total (IDR receipt rounding); the client no longer sends a step.
//!
//! Per the module's 4-layer rule this file holds no SQL — the register-config read lives on
//! `PosProfileRepository`.

use rust_decimal::Decimal;
use uuid::Uuid;

use crate::infrastructure::persistence::TaxConfigRow;

use super::pos_ports::{PosTaxComputePort, PosTaxComputeRequest, PosTaxDocumentType, PosTaxLineIn};
use super::pos_write_service::{money, round_to, PosError, PosWriteService, TicketTotals};

/// One line handed to the compute core — the neutral input shape shared by the online ring
/// (`NewSaleLine`) and the offline replay (`SyncSaleLine`).
pub struct ComputeLineIn {
    pub item_id: Uuid,
    pub revenue_account_id: Option<Uuid>,
    pub description: Option<String>,
    /// Course grouping (1 = starter, 2 = main, …) — a kitchen-routing label carried verbatim; it has
    /// no money semantics and never influences the compute.
    pub course: Option<i32>,
    pub quantity: Decimal,
    pub unit_price: Decimal,
    pub discount_amount: Decimal,
}

/// One line as the compute core resolved it: the client's inputs + the SERVER-adopted net.
pub struct ComputedLine {
    pub item_id: Uuid,
    pub revenue_account_id: Option<Uuid>,
    pub description: Option<String>,
    pub course: Option<i32>,
    pub quantity: Decimal,
    pub unit_price: Decimal,
    pub discount_amount: Decimal,
    /// The line's tax-excluded net AFTER the tax compute's redistribution — the only net that is
    /// ever persisted (POS's own rounding is overwritten by the port result, by contract).
    pub net_amount: Decimal,
}

/// A fully priced ticket: per-line adopted nets + the header money derived from them. The
/// `TicketTotals` half (paid/change included) is finished once tenders are known — see
/// [`Self::totals_with_tenders`].
pub struct ComputedTicket {
    pub lines: Vec<ComputedLine>,
    pub net_total: Decimal,
    pub tax_total: Decimal,
    pub grand_total: Decimal,
    pub rounding_adjustment: Decimal,
    pub rounded_total: Decimal,
}

impl PosWriteService {
    /// Derive a ticket's money from its inputs. See the module doc for the four-part contract.
    ///
    /// `document_type` tells the tax compute whether the document is a sale or a refund (the
    /// repartition family differs); `on_date` is the ticket's posting date.
    pub(super) async fn compute_ticket(
        &self,
        company_id: Uuid,
        pos_profile_id: Uuid,
        on_date: chrono::NaiveDate,
        document_type: PosTaxDocumentType,
        lines: Vec<ComputeLineIn>,
        tax: &dyn PosTaxComputePort,
    ) -> Result<ComputedTicket, PosError> {
        if lines.is_empty() {
            return Err(PosError::EmptyDocument);
        }
        // Register config: templates + rounding. Absent profile = 404; present-but-unconfigured
        // templates = typed refusal (the register must be explicitly tax-configured — a zero-rated
        // template is the non-PKP expression, so NULL/empty cannot silently mean "no tax").
        let cfg: TaxConfigRow = self
            .profiles
            .fetch_tax_config(&self.db_pool, pos_profile_id, company_id)
            .await?
            .ok_or(PosError::ProfileNotFound(pos_profile_id))?;
        let templates = parse_template_ids(cfg.tax_template_ids.as_ref())
            .ok_or(PosError::ProfileTaxTemplatesMissing(pos_profile_id))?;
        if templates.is_empty() {
            return Err(PosError::ProfileTaxTemplatesMissing(pos_profile_id));
        }

        // POS's own first-pass nets (money-rounded). These feed the tax compute; the port's
        // redistribution OVERWRITES them per line below.
        let mut first_pass: Vec<(ComputeLineIn, Decimal)> = Vec::with_capacity(lines.len());
        for l in lines {
            if l.quantity < Decimal::ZERO || l.unit_price < Decimal::ZERO || l.discount_amount < Decimal::ZERO {
                return Err(PosError::NegativeAmount);
            }
            let gross = money(l.quantity * l.unit_price);
            let net = gross - money(l.discount_amount);
            if net < Decimal::ZERO {
                return Err(PosError::NegativeAmount);
            }
            first_pass.push((l, net));
        }

        // Expand (line x template) into the compute request. Each line carries a correlation ref the
        // result keys its per-line nets and components back on.
        let refs: Vec<Uuid> = (0..first_pass.len()).map(|_| Uuid::new_v4()).collect();
        let tax_lines: Vec<PosTaxLineIn> = first_pass
            .iter()
            .zip(&refs)
            .flat_map(|((_, net), r)| {
                templates
                    .iter()
                    .map(|t| PosTaxLineIn {
                        line_ref: *r,
                        template_id: *t,
                        net_amount: *net,
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let result = tax
            .compute_document(&PosTaxComputeRequest {
                company_id,
                document_type,
                on_date,
                lines: tax_lines,
            })
            .await
            .map_err(|e| PosError::TaxRejected { code: e.code, message: e.message })?;

        // Adopt the port's per-line nets — the OVERWRITE half of the contract. A line the result
        // omitted keeps POS's first pass (the port only redistributes what it was asked to).
        let mut adopted: Vec<Decimal> = first_pass.iter().map(|(_, n)| *n).collect();
        for (r, net) in &result.net_amounts {
            if let Some(pos) = refs.iter().position(|x| x == r) {
                adopted[pos] = *net;
            }
        }

        let mut computed = Vec::with_capacity(first_pass.len());
        let mut net_total = Decimal::ZERO;
        for ((l, _), net) in first_pass.into_iter().zip(adopted) {
            net_total += net;
            computed.push(ComputedLine {
                item_id: l.item_id,
                revenue_account_id: l.revenue_account_id,
                description: l.description,
                course: l.course,
                quantity: l.quantity,
                unit_price: l.unit_price,
                discount_amount: money(l.discount_amount),
                net_amount: net,
            });
        }
        let net_total = money(net_total);
        // Tax total = Σ signed components (the same components the implementer books), so the header
        // can never disagree with the per-line tax split that reaches the GL.
        let mut tax_total = Decimal::ZERO;
        for c in &result.components {
            tax_total += c.tax_amount;
        }
        let tax_total = money(tax_total);
        let grand = net_total + tax_total;
        // Register-config cash rounding: `none` (or a zero unit) still money-rounds to 2dp; `half_up`
        // steps to the configured unit (e.g. 100 for IDR receipts).
        let step = match cfg.rounding_strategy.as_str() {
            "half_up" if cfg.rounding_unit > Decimal::ZERO => cfg.rounding_unit,
            _ => Decimal::ZERO,
        };
        let rounded = round_to(grand, step);
        let rounding_adjustment = rounded - grand;
        Ok(ComputedTicket {
            lines: computed,
            net_total,
            tax_total,
            grand_total: grand,
            rounding_adjustment,
            rounded_total: rounded,
        })
    }

    /// Finish a [`TicketTotals`] once the tenders are known: the pay-to total is the ROUNDED total,
    /// overpayment reads as change due. Shared by the ring path's tender step and the offline replay.
    pub(super) fn totals_with_tenders(t: &ComputedTicket, paid_total: Decimal) -> TicketTotals {
        let paid_total = money(paid_total);
        let change_due = if paid_total > t.rounded_total {
            paid_total - t.rounded_total
        } else {
            Decimal::ZERO
        };
        TicketTotals {
            net_total: t.net_total,
            tax_total: t.tax_total,
            grand_total: t.grand_total,
            rounding_adjustment: t.rounding_adjustment,
            rounded_total: t.rounded_total,
            paid_total,
            change_due,
        }
    }
}

/// Parse the profile's `tax_template_ids` JSON into template ids. Accepts an array of uuid strings
/// (or of `{id: ...}` objects, tolerantly); `None` = not JSON-shaped at all — the caller treats that
/// exactly like NULL (unconfigured register, typed refusal).
pub(super) fn parse_template_ids(v: Option<&serde_json::Value>) -> Option<Vec<Uuid>> {
    let arr = v?.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for e in arr {
        let id = match e {
            serde_json::Value::String(s) => Uuid::parse_str(s).ok(),
            serde_json::Value::Object(o) => {
                o.get("id").and_then(|i| i.as_str()).and_then(|s| Uuid::parse_str(s).ok())
            }
            _ => None,
        };
        // A malformed entry is a misconfigured register, not a partial tax application — refuse all.
        out.push(id?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parsing_accepts_strings_and_objects() {
        let strings = serde_json::json!(["11111111-1111-1111-1111-111111111111"]);
        assert_eq!(
            parse_template_ids(Some(&strings)).unwrap(),
            vec![Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()]
        );
        let objects = serde_json::json!([{ "id": "22222222-2222-2222-2222-222222222222" }]);
        assert_eq!(parse_template_ids(Some(&objects)).unwrap().len(), 1);
    }

    #[test]
    fn template_parsing_refuses_null_empty_and_malformed() {
        assert_eq!(parse_template_ids(None), None); // NULL is unshaped -> caller refuses
        assert_eq!(parse_template_ids(Some(&serde_json::json!([]))), Some(vec![]));
        // A malformed entry refuses the whole set (None), never a partial application.
        assert_eq!(parse_template_ids(Some(&serde_json::json!(["not-a-uuid"]))), None);
        assert_eq!(parse_template_ids(Some(&serde_json::json!("not-an-array"))), None);
    }

    #[test]
    fn totals_finish_change_due_on_overpayment() {
        let t = ComputedTicket {
            lines: vec![],
            net_total: Decimal::from(1000),
            tax_total: Decimal::from(110),
            grand_total: Decimal::from(1110),
            rounding_adjustment: Decimal::from(-10),
            rounded_total: Decimal::from(1100),
        };
        let totals = PosWriteService::totals_with_tenders(&t, Decimal::from(1500));
        assert_eq!(totals.change_due, Decimal::from(400));
        assert_eq!(totals.paid_total, Decimal::from(1500));
        // Underpayment carries no change.
        let under = PosWriteService::totals_with_tenders(&t, Decimal::from(500));
        assert_eq!(under.change_due, Decimal::ZERO);
    }
}
