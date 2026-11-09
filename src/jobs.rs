//! Background work: finish shows, plan due settlements, reconcile payments, send and track payouts.
//! Runs every 15 s in the web process (and once per call of `misfy-dropcard cycle`, e.g. from a cron job).
//! A Postgres advisory lock makes sure only one cycle runs at a time, even with several processes.

use std::time::Duration;

use tokio::time::MissedTickBehavior;
use uuid::Uuid;

use crate::{payments, payouts, AppState};

const LOCK_KEY: i64 = 77_001;

/// Runs one cycle. With `session`, only that visitor's shows are settled (the console's
/// "Run settlement now" button) and we wait briefly for the lock instead of skipping.
pub async fn run_cycle(st: &AppState, session: Option<Uuid>) -> anyhow::Result<()> {
    let mut conn = st.db.acquire().await?;
    let tries = if session.is_some() { 40 } else { 1 };
    let mut locked = false;
    for _ in 0..tries {
        locked = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)").bind(LOCK_KEY).fetch_one(&mut *conn).await?;
        if locked {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if !locked {
        return Ok(());
    }
    let result = cycle(st, session).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)").bind(LOCK_KEY).execute(&mut *conn).await;
    result
}

async fn cycle(st: &AppState, session: Option<Uuid>) -> anyhow::Result<()> {
    payouts::complete_shows(&st.db, session).await?;
    payouts::plan_due(&st.db, session).await?;
    if let Some(pp) = st.paypal.as_deref() {
        payments::reconcile(&st.db, pp).await?;
        payouts::submit_planned(&st.db, pp, session).await?;
        payouts::poll_pending(&st.db, pp).await?;
        if session.is_none() {
            payouts::retry_failed(&st.db, pp).await?;
        }
    }
    Ok(())
}

pub fn spawn(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = run_cycle(&st, None).await {
                tracing::warn!(error = %e, "background cycle failed");
            }
        }
    });
}
