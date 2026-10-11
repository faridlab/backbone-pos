// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

// The hand-written POS SQL's parameter/projection types. Their repositories are each declared
// `user_owned` in metaphor.codegen.yaml; the param structs mirror COLUMNS, not entities.
pub use pos_cash_movement_repository::NewCashMovementRow;
pub use pos_closing_entry_repository::NewClosingEntryRow;
pub use pos_invoice_item_repository::{
    NewInvoiceItemRow, QuantityLineRow, ReceiptLineRow, RevenueLineRow,
};
pub use pos_invoice_repository::{
    NewDraftInvoiceRow, NewReturnInvoiceRow, PaidStateRow, ReceiptHeaderRow, RecognitionRow,
    ReturnSourceRow, SyncLookupRow, TenderHeaderRow,
};
pub use pos_opening_entry_repository::NewOpeningEntryRow;
pub use pos_payment_repository::{MethodTotalRow, NewTenderRow, ReceiptTenderRow};
pub use pos_profile_repository::TaxConfigRow;
// Sibling files adding hand-written methods to GENERATED repositories (the module's sanctioned
// custom-code convention; each sibling is user_owned in metaphor.codegen.yaml).
mod pos_discount_repository_custom;
mod pos_table_repository_custom;
pub use pos_discount_repository_custom::DiscountRow;
pub use pos_opening_entry_repository::StaleSessionRow;
pub use pos_table_repository_custom::TableRow;
