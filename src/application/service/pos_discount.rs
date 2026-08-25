//! Order-level discount resolution + folding (hand-authored, user-owned).
//!
//! An `impl PosWriteService` chunk over the vocabulary in [`super::pos_write_service`]. The whole
//! point of this file is the trust posture: a client ringing a ticket names a discount by ID and
//! nothing else. The percentage applied is ALWAYS the one on the caller's tenant-scoped master row,
//! the register must have discounts enabled (`pos_profiles.allow_discount`), and the fold happens
//! server-side into the per-line `discount_amount` BEFORE the tax compute — so tax sees post-discount
//! nets and a client-authored discount value can never reach a total.
//!
//! Per the module's 4-layer rule this file holds no SQL — the master + register reads live on
//! `PosDiscountRepository` / `PosProfileRepository`.

use rust_decimal::Decimal;
use uuid::Uuid;

use crate::infrastructure::persistence::DiscountRow;

use super::pos_compute::ComputeLineIn;
use super::pos_write_service::{money, PosError, PosWriteService};

impl PosWriteService {
    /// Resolve the order-level discount a ring/sync named. `Ok(None)` = no discount was named and
    /// none applies. Refusals are typed:
    /// - register not found → [`PosError::ProfileNotFound`];
    /// - register has discounts off → [`PosError::DiscountNotAllowed`];
    /// - master id unknown in this tenant → [`PosError::DiscountNotFound`];
    /// - master percentage not a sane fraction → [`PosError::DiscountInvalid`].
    pub(super) async fn resolve_order_discount(
        &self,
        company_id: Uuid,
        pos_profile_id: Uuid,
        discount_id: Option<Uuid>,
    ) -> Result<Option<DiscountRow>, PosError> {
        let Some(discount_id) = discount_id else { return Ok(None) };
        // Register gate: `allow_discount` rides the same config read the compute uses (one extra
        // scoped round trip on a ring that named a discount — cheap, and keeps a single source for
        // "does this register exist").
        let cfg = self
            .profiles
            .fetch_tax_config(&self.db_pool, pos_profile_id, company_id)
            .await?
            .ok_or(PosError::ProfileNotFound(pos_profile_id))?;
        if !cfg.allow_discount {
            return Err(PosError::DiscountNotAllowed);
        }
        let d = self
            .discounts
            .fetch_discount(&self.db_pool, discount_id, company_id)
            .await?
            .ok_or(PosError::DiscountNotFound(discount_id))?;
        if d.percentage < Decimal::ZERO || d.percentage > Decimal::ONE {
            // The column CHECK only bounds the master at storage time; this is the read-side fence
            // for rows that predate it or arrived another way.
            return Err(PosError::DiscountInvalid("percentage must be between 0 and 1"));
        }
        Ok(Some(d))
    }

    /// Fold a resolved order-level discount into the lines, pro-rata by line gross, per-line
    /// money-rounded, BEFORE the tax compute runs (the caller mutates its `ComputeLineIn`s in place
    /// and then prices the ticket — tax therefore sees post-discount nets). Returns the total folded.
    ///
    /// The order discount is DEFINED as the sum of the per-line rounded shares — the same
    /// per-line-rounding discipline the document-grade tax compute applies — so Σ lines == header
    /// net holds without a penny-jar remainder line.
    pub(super) fn fold_order_discount(lines: &mut [ComputeLineIn], pct: Decimal) -> Result<Decimal, PosError> {
        let mut folded = Decimal::ZERO;
        for l in lines.iter_mut() {
            let gross = l.quantity * l.unit_price;
            let share = money(gross * pct);
            let new_discount = l.discount_amount + share;
            if new_discount > gross {
                // Line discount + order share over-discounts the line. Refuse rather than clamp: a
                // silent clamp would let a combination of discounts price a line below its tax base.
                return Err(PosError::DiscountInvalid("combined discounts exceed line gross"));
            }
            l.discount_amount = new_discount;
            folded += share;
        }
        Ok(folded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(q: i64, price: i64, disc: i64) -> ComputeLineIn {
        ComputeLineIn {
            item_id: Uuid::new_v4(),
            revenue_account_id: None,
            description: None,
            course: None,
            quantity: Decimal::from(q),
            unit_price: Decimal::from(price),
            discount_amount: Decimal::from(disc),
        }
    }

    #[test]
    fn folds_pro_rata_by_line_gross() {
        let mut lines = vec![line(2, 25_000, 0), line(1, 50_000, 0)];
        let folded = PosWriteService::fold_order_discount(&mut lines, Decimal::new(1, 1)).unwrap();
        // 10% of 50,000 and of 50,000 = 10,000 total.
        assert_eq!(folded, Decimal::from(10_000));
        assert_eq!(lines[0].discount_amount, Decimal::from(5_000));
        assert_eq!(lines[1].discount_amount, Decimal::from(5_000));
    }

    #[test]
    fn folds_onto_existing_line_discount() {
        let mut lines = vec![line(1, 100, 10)];
        // 12.5% of gross 100 = 12.50 folded on top of the existing 10.
        let folded = PosWriteService::fold_order_discount(&mut lines, Decimal::new(125, 3)).unwrap();
        assert_eq!(folded, Decimal::new(125, 1)); // 12.5
        assert_eq!(lines[0].discount_amount, Decimal::new(225, 1)); // 10 + 12.5
    }

    #[test]
    fn refuses_when_combined_discounts_exceed_gross() {
        let mut lines = vec![line(1, 100, 90)];
        assert!(matches!(
            PosWriteService::fold_order_discount(&mut lines, Decimal::new(2, 1)), // 20% share = 20 > 10 headroom
            Err(PosError::DiscountInvalid(_))
        ));
    }

    #[test]
    fn rounds_each_line_share_to_money() {
        let mut lines = vec![line(3, 33, 0)]; // gross 99; 10% = 9.9
        let folded = PosWriteService::fold_order_discount(&mut lines, Decimal::new(1, 1)).unwrap();
        assert_eq!(folded, Decimal::new(990, 2));
        assert_eq!(lines[0].discount_amount, Decimal::new(990, 2));
    }
}
