//! Recurring money obligations: bills, EMIs, insurance, subscriptions, and the
//! fixed India tax dates.
//!
//! # What it computes
//!
//! The next time each thing is due, from an anchor date and a recurrence,
//! relative to today. So a monthly bill always shows its upcoming date without
//! anyone marking it paid: the next occurrence on or after today is derived,
//! not stored. India's statutory tax dates are fixed each year and are computed
//! the same way, as a built-in list, so they never need entering.
//!
//! # Why the date math is here, with tests
//!
//! "Due in 3 days" is only useful if it is right, and month-rollover and leap
//! years are exactly where hand-waved date code goes wrong. The arithmetic is
//! small, so it is written out and tested rather than pulling in a date crate.

use serde::Serialize;

/// A recurring obligation with its next due date worked out.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillView {
    pub id: i64,
    pub name: String,
    /// bill | emi | insurance | subscription.
    pub kind: String,
    pub amount: Option<f64>,
    pub recurrence: String,
    /// The next occurrence on or after today, YYYY-MM-DD.
    pub next_due: String,
    /// Whole days from today to that date. Negative only for a one-off already
    /// past; recurring items always roll forward to a non-negative value.
    pub days_until: i64,
}

/// A fixed statutory tax date.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaxDate {
    pub name: String,
    pub date: String,
    pub days_until: i64,
}

/// Work out a bill's next due date and days remaining from its anchor and
/// recurrence. `today_unix` is passed so the result is testable.
pub fn bill_view(
    id: i64,
    name: String,
    kind: String,
    amount: Option<f64>,
    recurrence: &str,
    anchor: &str,
    today_unix: i64,
) -> Option<BillView> {
    let today = unix_to_ymd(today_unix);
    let (ay, am, ad) = parse_ymd(anchor)?;
    let next = next_occurrence((ay, am, ad), recurrence, today);
    let next_str = fmt_ymd(next);
    let days_until = days_between(today, next);
    Some(BillView {
        id,
        name,
        kind,
        amount,
        recurrence: recurrence.to_string(),
        next_due: next_str,
        days_until,
    })
}

/// The next occurrence of an anchored, recurring date on or after `today`.
///
/// A one-off returns its anchor unchanged (even if past, so it can still be
/// shown as overdue). A recurring date is rolled forward by its period until it
/// reaches today, preserving the anchor's day-of-month (clamped to short
/// months, so the 31st becomes the 30th or the 28th where needed).
fn next_occurrence(anchor: (i64, i64, i64), recurrence: &str, today: (i64, i64, i64)) -> (i64, i64, i64) {
    let step = match recurrence {
        "monthly" => 1,
        "quarterly" => 3,
        "yearly" => 12,
        _ => return anchor, // "once" or unknown: the date is the date.
    };
    let mut d = anchor;
    // Guard the loop: at most a few decades of periods, never unbounded.
    for _ in 0..1_200 {
        if cmp(d, today) >= 0 {
            return d;
        }
        d = add_months(anchor, months_from(anchor, d) + step);
    }
    d
}

/// How many `step`-sized periods, in months, `current` is past `anchor`.
fn months_from(anchor: (i64, i64, i64), current: (i64, i64, i64)) -> i64 {
    (current.0 - anchor.0) * 12 + (current.1 - anchor.1)
}

/// Add `n` months to a date, keeping the anchor's day clamped to the target
/// month's length.
fn add_months(date: (i64, i64, i64), n: i64) -> (i64, i64, i64) {
    let (y, m, d) = date;
    let total = (m - 1) + n;
    let ny = y + total.div_euclid(12);
    let nm = total.rem_euclid(12) + 1;
    let nd = d.min(days_in_month(ny, nm));
    (ny, nm, nd)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 30,
    }
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn cmp(a: (i64, i64, i64), b: (i64, i64, i64)) -> i64 {
    if a == b {
        0
    } else if (a.0, a.1, a.2) < (b.0, b.1, b.2) {
        -1
    } else {
        1
    }
}

/// The upcoming India statutory tax dates, from today, soonest first.
///
/// Advance-tax instalments (15 Jun / 15 Sep / 15 Dec / 15 Mar) and the usual
/// individual ITR deadline (31 Jul). Fixed each year, so they are generated for
/// this year and next and filtered to those still ahead. These are the common
/// dates, not tailored tax advice; a given year's extended deadline or a
/// taxpayer's own situation can differ.
pub fn upcoming_tax_dates(today_unix: i64, count: usize) -> Vec<TaxDate> {
    let today = unix_to_ymd(today_unix);
    let year = today.0;
    // (month, day, label)
    let fixtures = [
        (6, 15, "Advance tax (Q1)"),
        (9, 15, "Advance tax (Q2)"),
        (12, 15, "Advance tax (Q3)"),
        (3, 15, "Advance tax (Q4)"),
        (7, 31, "Income-tax return (ITR)"),
    ];
    let mut out: Vec<TaxDate> = Vec::new();
    for y in [year, year + 1] {
        for (m, d, label) in fixtures {
            let date = (y, m as i64, d as i64);
            if cmp(date, today) >= 0 {
                out.push(TaxDate {
                    name: label.to_string(),
                    date: fmt_ymd(date),
                    days_until: days_between(today, date),
                });
            }
        }
    }
    out.sort_by_key(|t| t.days_until);
    out.truncate(count);
    out
}

// -- Date helpers (civil calendar, no crate) ----------------------------------

fn parse_ymd(s: &str) -> Option<(i64, i64, i64)> {
    let mut p = s.trim().split('-');
    let y: i64 = p.next()?.parse().ok()?;
    let m: i64 = p.next()?.parse().ok()?;
    let d: i64 = p.next()?.parse().ok()?;
    if (1..=12).contains(&m) && (1..=31).contains(&d) {
        Some((y, m, d))
    } else {
        None
    }
}

fn fmt_ymd(date: (i64, i64, i64)) -> String {
    format!("{:04}-{:02}-{:02}", date.0, date.1, date.2)
}

/// Days from the Unix epoch for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_between(a: (i64, i64, i64), b: (i64, i64, i64)) -> i64 {
    days_from_civil(b.0, b.1, b.2) - days_from_civil(a.0, a.1, a.2)
}

fn unix_to_ymd(unix: i64) -> (i64, i64, i64) {
    let days = unix.div_euclid(86_400) + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fixed "today" = 2026-10-22, so every test is deterministic.
    fn today() -> i64 {
        days_from_civil(2026, 10, 22) * 86_400
    }

    #[test]
    fn a_monthly_bill_rolls_to_the_next_occurrence() {
        // Anchor the 5th; today is the 22nd, so next is next month's 5th.
        let v = bill_view(1, "Rent".into(), "bill".into(), Some(25000.0), "monthly", "2026-01-05", today()).unwrap();
        assert_eq!(v.next_due, "2026-11-05");
        assert_eq!(v.days_until, 14);
    }

    #[test]
    fn a_monthly_bill_due_later_this_month_stays_this_month() {
        let v = bill_view(1, "Card".into(), "bill".into(), None, "monthly", "2026-01-28", today()).unwrap();
        assert_eq!(v.next_due, "2026-10-28");
        assert_eq!(v.days_until, 6);
    }

    #[test]
    fn a_short_month_clamps_the_day() {
        // 31st anchored, rolled into November, which has 30 days.
        let v = bill_view(1, "X".into(), "emi".into(), None, "monthly", "2026-08-31", today()).unwrap();
        assert_eq!(v.next_due, "2026-10-31");
        // And into February of a non-leap-adjacent year clamps to 28/29.
        let feb = add_months((2026, 8, 31), 6);
        assert_eq!(feb, (2027, 2, 28));
    }

    #[test]
    fn a_yearly_insurance_premium_finds_the_next_year() {
        // Anchor in March; today is October, so next is next March.
        let v = bill_view(1, "LIC".into(), "insurance".into(), Some(18000.0), "yearly", "2020-03-10", today()).unwrap();
        assert_eq!(v.next_due, "2027-03-10");
    }

    #[test]
    fn a_one_off_keeps_its_date_even_if_past() {
        let v = bill_view(1, "Once".into(), "bill".into(), None, "once", "2026-01-01", today()).unwrap();
        assert_eq!(v.next_due, "2026-01-01");
        assert!(v.days_until < 0);
    }

    #[test]
    fn tax_dates_are_upcoming_and_sorted() {
        let dates = upcoming_tax_dates(today(), 5);
        assert!(!dates.is_empty());
        // All ahead of today, ascending.
        assert!(dates.iter().all(|t| t.days_until >= 0));
        for w in dates.windows(2) {
            assert!(w[0].days_until <= w[1].days_until);
        }
        // From 2026-10-22 the next is 15 Dec (advance tax Q3).
        assert_eq!(dates[0].date, "2026-12-15");
    }

    #[test]
    fn days_between_known_dates_is_exact() {
        assert_eq!(days_between((2026, 10, 22), (2026, 10, 23)), 1);
        assert_eq!(days_between((2024, 2, 28), (2024, 3, 1)), 2); // leap year
    }
}
