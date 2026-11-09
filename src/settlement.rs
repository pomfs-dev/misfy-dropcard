//! Settlement rule. The same arithmetic is used by the sample-data generator in
//! `migrations/0002_sample_data.sql`, so sample and live settlements always agree.
//!
//! * Money in for a show = captured PayPal payments minus PayPal fees minus refunds,
//!   kept separately for the deposit and for ticket sales.
//! * The platform fee is taken at settlement time: `round_half_up(amount * fee_bps / 10_000)`,
//!   separately from the deposit and from ticket sales.
//! * The deposit (after the fee) goes to the venue.
//! * Ticket sales (after the fee) are shared: the artist gets `floor(x * artist_bps)`,
//!   the promoter `floor(x * promoter_bps)`, and the venue gets the rest (so no cent is lost).
//!   With no artist on the bill, the artist share stays with the venue.

use serde::Serialize;

#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub platform_fee_bps: i64,
    pub venue_bps: i64,
    pub artist_bps: i64,
    pub promoter_bps: i64,
    pub has_artist: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Split {
    pub deposit_net: i64,
    pub tickets_net: i64,
    pub platform_fee: i64,
    pub venue: i64,
    pub artist: i64,
    pub promoter: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SplitError {
    #[error("refunds exceed payments for this show (deposit {deposit_net}, tickets {tickets_net})")]
    NegativeNet { deposit_net: i64, tickets_net: i64 },
    #[error("split must add up to 10000 basis points")]
    InvalidRule,
}

fn round_half_up_bps(amount: i64, bps: i64) -> i64 {
    (amount * bps + 5_000) / 10_000
}

fn floor_bps(amount: i64, bps: i64) -> i64 {
    amount * bps / 10_000
}

pub fn compute(deposit_net: i64, tickets_net: i64, rule: Rule) -> Result<Split, SplitError> {
    if deposit_net < 0 || tickets_net < 0 {
        return Err(SplitError::NegativeNet { deposit_net, tickets_net });
    }
    if rule.venue_bps + rule.artist_bps + rule.promoter_bps != 10_000
        || !(0..=3_000).contains(&rule.platform_fee_bps)
    {
        return Err(SplitError::InvalidRule);
    }
    let deposit_fee = round_half_up_bps(deposit_net, rule.platform_fee_bps);
    let tickets_fee = round_half_up_bps(tickets_net, rule.platform_fee_bps);
    let deposit_after = deposit_net - deposit_fee;
    let tickets_after = tickets_net - tickets_fee;

    let artist = if rule.has_artist { floor_bps(tickets_after, rule.artist_bps) } else { 0 };
    let promoter = floor_bps(tickets_after, rule.promoter_bps);
    let venue = deposit_after + (tickets_after - artist - promoter);

    Ok(Split {
        deposit_net,
        tickets_net,
        platform_fee: deposit_fee + tickets_fee,
        venue,
        artist,
        promoter,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: Rule = Rule {
        platform_fee_bps: 500,
        venue_bps: 3_000,
        artist_bps: 5_000,
        promoter_bps: 2_000,
        has_artist: true,
    };

    #[test]
    fn worked_example() {
        // $720 deposit, $1,000 tickets (both already net of PayPal fees)
        let s = compute(72_000, 100_000, RULE).unwrap();
        assert_eq!(s.platform_fee, 3_600 + 5_000);
        assert_eq!(s.artist, 47_500);
        assert_eq!(s.promoter, 19_000);
        assert_eq!(s.venue, 68_400 + 28_500);
    }

    #[test]
    fn remainder_cents_go_to_the_venue() {
        // tickets 101 -> fee round(5.05) = 5 -> 96; artist floor(48.0) = 48, promoter floor(19.2) = 19
        let s = compute(0, 101, RULE).unwrap();
        assert_eq!((s.platform_fee, s.artist, s.promoter, s.venue), (5, 48, 19, 29));
    }

    #[test]
    fn no_artist_share_stays_with_venue() {
        let s = compute(0, 10_000, Rule { has_artist: false, ..RULE }).unwrap();
        assert_eq!(s.artist, 0);
        assert_eq!(s.venue + s.promoter + s.platform_fee, 10_000);
    }

    #[test]
    fn every_cent_is_accounted_for() {
        for deposit in [0_i64, 1, 99, 72_000, 123_457] {
            for tickets in [0_i64, 1, 7, 101, 99_999, 1_234_567] {
                let s = compute(deposit, tickets, RULE).unwrap();
                assert_eq!(s.venue + s.artist + s.promoter + s.platform_fee, deposit + tickets);
                assert!(s.venue >= 0 && s.artist >= 0 && s.promoter >= 0 && s.platform_fee >= 0);
            }
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(compute(-1, 0, RULE), Err(SplitError::NegativeNet { .. })));
        assert_eq!(compute(0, 0, Rule { artist_bps: 6_000, ..RULE }), Err(SplitError::InvalidRule));
    }
}
