//! Double-entry ledger. Payments, fees and payouts are all written here, and every transaction
//! sums to zero — so "money in" and "money out" can always be reconciled from one table.

use anyhow::{ensure, Result};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Entry {
    pub account: &'static str,
    pub kind: &'static str,
    pub amount_cents: i64,
    pub memo: String,
}

impl Entry {
    pub fn new(account: &'static str, kind: &'static str, amount_cents: i64, memo: impl Into<String>) -> Self {
        Self { account, kind, amount_cents, memo: memo.into() }
    }
}

/// Writes one balanced transaction. Fails (and writes nothing) if the entries do not sum to zero.
pub async fn post(
    tx: &mut Transaction<'_, Postgres>,
    booking_id: Uuid,
    order_id: Option<Uuid>,
    payout_item_id: Option<Uuid>,
    entries: &[Entry],
) -> Result<Uuid> {
    let sum: i64 = entries.iter().map(|e| e.amount_cents).sum();
    ensure!(sum == 0, "unbalanced ledger transaction (sum {sum})");
    ensure!(!entries.is_empty(), "empty ledger transaction");
    let txn_id = Uuid::new_v4();
    for e in entries.iter().filter(|e| e.amount_cents != 0) {
        sqlx::query(
            "INSERT INTO ledger_entries (txn_id, booking_id, order_id, payout_item_id, kind, account, amount_cents, memo)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(txn_id)
        .bind(booking_id)
        .bind(order_id)
        .bind(payout_item_id)
        .bind(e.kind)
        .bind(e.account)
        .bind(e.amount_cents)
        .bind(&e.memo)
        .execute(&mut **tx)
        .await?;
    }
    Ok(txn_id)
}
