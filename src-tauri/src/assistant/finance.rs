//! The money math: goals and retirement projections.
//!
//! # Why this is in Rust, with tests
//!
//! These are the numbers a person makes decisions on, so they have to be right.
//! A compounding formula fat-fingered in a component is a wrong answer that
//! looks authoritative. Putting the formulas here, behind tests with worked
//! values, is what lets the UI show a projection without it being a guess.
//!
//! # What these are, and are not
//!
//! Projections under assumptions the user supplies: a return rate, an inflation
//! rate, a horizon. They compound those assumptions honestly. They are **not**
//! predictions, promises, or advice. A projection says "if money grew at the
//! rate you entered, here is where it lands"; it does not say that it will.
//! Everything that uses these shows the assumptions alongside the result.
//!
//! All rates are entered as percentages (8 means 8%). Money compounds monthly,
//! which is how SIPs and most Indian return quotes are framed.

use serde::{Deserialize, Serialize};

/// Monthly growth factor from an annual percentage rate.
fn monthly_rate(annual_pct: f64) -> f64 {
    (annual_pct / 100.0) / 12.0
}

/// What a lump sum grows to after `years` at `annual_pct`, compounded monthly.
pub fn future_value_lump(principal: f64, annual_pct: f64, years: f64) -> f64 {
    let i = monthly_rate(annual_pct);
    let m = years * 12.0;
    principal * (1.0 + i).powf(m)
}

/// What a monthly contribution grows to after `years`, compounded monthly.
///
/// Ordinary annuity: each contribution lands at the end of its month. At a zero
/// rate this is just `monthly * months`, which the formula would divide-by-zero
/// on, so that case is handled directly.
pub fn future_value_sip(monthly: f64, annual_pct: f64, years: f64) -> f64 {
    let i = monthly_rate(annual_pct);
    let m = years * 12.0;
    if i.abs() < 1e-12 {
        return monthly * m;
    }
    monthly * (((1.0 + i).powf(m) - 1.0) / i)
}

/// The monthly contribution needed to reach `target`, given a `current` corpus
/// that also grows. Zero when the current corpus alone already gets there.
pub fn required_monthly(target: f64, current: f64, annual_pct: f64, years: f64) -> f64 {
    let i = monthly_rate(annual_pct);
    let m = years * 12.0;
    let from_current = future_value_lump(current, annual_pct, years);
    let gap = target - from_current;
    if gap <= 0.0 {
        return 0.0;
    }
    if m <= 0.0 {
        return gap; // No time to compound: the gap is due now.
    }
    if i.abs() < 1e-12 {
        return gap / m;
    }
    gap * i / ((1.0 + i).powf(m) - 1.0)
}

/// A value stated in today's rupees, grown to its equivalent `years` out at
/// `inflation_pct`. Used to turn "what I spend now" into "what it will cost
/// then".
pub fn inflate(today: f64, inflation_pct: f64, years: f64) -> f64 {
    today * (1.0 + inflation_pct / 100.0).powf(years)
}

// -- Goals --------------------------------------------------------------------

/// A savings goal with its progress worked out.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalProgress {
    pub id: i64,
    pub name: String,
    pub target_amount: f64,
    pub current_amount: f64,
    pub target_date: String,
    pub expected_return: f64,
    /// 0..=100, clamped. How much of the target is already saved.
    pub percent: f64,
    /// Years until the target date, from today. Negative dates clamp to 0.
    pub years_left: f64,
    /// Monthly saving needed from now to hit the target by the date, given the
    /// current amount compounding. `None` when the date is unreadable.
    pub required_monthly: Option<f64>,
}

/// Work out a goal's progress from its stored fields and today's date.
///
/// `today_unix` is passed in rather than read from the clock so the result is
/// testable: the same goal and date always compute the same.
pub fn goal_progress(
    id: i64,
    name: String,
    target_amount: f64,
    current_amount: f64,
    target_date: &str,
    expected_return: f64,
    today_unix: i64,
) -> GoalProgress {
    let percent = if target_amount > 0.0 {
        (current_amount / target_amount * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    let years_left = years_until(target_date, today_unix);
    let required_monthly = years_left.map(|yrs| {
        required_monthly(target_amount, current_amount, expected_return, yrs.max(0.0))
    });

    GoalProgress {
        id,
        name,
        target_amount,
        current_amount,
        target_date: target_date.to_string(),
        expected_return,
        percent,
        years_left: years_left.unwrap_or(0.0).max(0.0),
        required_monthly,
    }
}

/// Years from `today_unix` to a `YYYY-MM-DD` date, or `None` if unparseable.
/// Approximate (365.25 days/year), which is plenty for a savings horizon.
fn years_until(date: &str, today_unix: i64) -> Option<f64> {
    let target = parse_ymd_to_unix(date)?;
    Some((target - today_unix) as f64 / (365.25 * 86_400.0))
}

/// A `YYYY-MM-DD` date to unix seconds at UTC midnight. Hand-rolled so no date
/// crate is pulled in for one parse.
fn parse_ymd_to_unix(date: &str) -> Option<i64> {
    let mut parts = date.trim().split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days from the Unix epoch to year/month/day, via a days-from-civil
    // algorithm (Howard Hinnant's), which handles leap years exactly.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400)
}

// -- Retirement ---------------------------------------------------------------

/// Inputs for a retirement projection. All rates are percentages.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetirementInput {
    pub current_age: f64,
    pub retirement_age: f64,
    pub current_corpus: f64,
    pub monthly_sip: f64,
    pub expected_return: f64,
    pub inflation: f64,
    /// Monthly expense in today's rupees, used to size the target corpus.
    pub monthly_expense_today: f64,
}

/// The result of a retirement projection.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetirementProjection {
    pub years: f64,
    /// What the current corpus plus the SIP grows to by retirement.
    pub projected_corpus: f64,
    /// The corpus needed: inflated annual expenses times 25 (the 4% rule).
    pub target_corpus: f64,
    pub on_track: bool,
    /// How far short, zero when on track.
    pub shortfall: f64,
    /// Total monthly saving needed to hit the target (not the extra).
    pub required_monthly: f64,
    /// Extra monthly saving over the current SIP to get on track, zero if none.
    pub extra_monthly: f64,
}

/// The withdrawal multiple behind the target: 25x annual expenses, i.e. the
/// 4% safe-withdrawal rule. A round, widely-used rule of thumb, not a law; it
/// is named in the UI so the user can judge it.
const WITHDRAWAL_MULTIPLE: f64 = 25.0;

pub fn project_retirement(input: &RetirementInput) -> RetirementProjection {
    let years = (input.retirement_age - input.current_age).max(0.0);

    let projected_corpus = future_value_lump(input.current_corpus, input.expected_return, years)
        + future_value_sip(input.monthly_sip, input.expected_return, years);

    let annual_expense_then = inflate(input.monthly_expense_today * 12.0, input.inflation, years);
    let target_corpus = annual_expense_then * WITHDRAWAL_MULTIPLE;

    let on_track = projected_corpus >= target_corpus;
    let shortfall = (target_corpus - projected_corpus).max(0.0);
    let required_monthly =
        required_monthly(target_corpus, input.current_corpus, input.expected_return, years);
    let extra_monthly = (required_monthly - input.monthly_sip).max(0.0);

    RetirementProjection {
        years,
        projected_corpus,
        target_corpus,
        on_track,
        shortfall,
        required_monthly,
        extra_monthly,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn a_lump_sum_compounds_monthly() {
        // 100000 at 12% for 1 year, compounded monthly: 100000*(1.01)^12.
        assert!(close(future_value_lump(100_000.0, 12.0, 1.0), 112_682.5, 1.0));
    }

    #[test]
    fn a_sip_at_zero_rate_is_just_the_sum_of_contributions() {
        assert!(close(future_value_sip(5_000.0, 0.0, 2.0), 120_000.0, 0.01));
    }

    #[test]
    fn a_sip_compounds() {
        // 10000/month at 12% for 1 year, ordinary annuity.
        // 10000 * ((1.01^12 - 1)/0.01) = 10000 * 12.6825 = 126825.
        assert!(close(future_value_sip(10_000.0, 12.0, 1.0), 126_825.0, 5.0));
    }

    #[test]
    fn required_monthly_is_zero_when_the_current_corpus_already_gets_there() {
        // 1,000,000 now at 10% easily exceeds a 500,000 target in 10 years.
        assert_eq!(required_monthly(500_000.0, 1_000_000.0, 10.0, 10.0), 0.0);
    }

    #[test]
    fn required_monthly_funds_the_gap() {
        // With no current corpus, the required SIP must itself grow to target.
        let req = required_monthly(126_825.0, 0.0, 12.0, 1.0);
        assert!(close(req, 10_000.0, 1.0), "got {req}");
    }

    #[test]
    fn inflation_grows_a_todays_number() {
        // 100 at 6% for 1 year is 106.
        assert!(close(inflate(100.0, 6.0, 1.0), 106.0, 0.001));
    }

    #[test]
    fn a_retirement_projection_sizes_the_target_and_the_gap() {
        let input = RetirementInput {
            current_age: 30.0,
            retirement_age: 60.0,
            current_corpus: 500_000.0,
            monthly_sip: 20_000.0,
            expected_return: 10.0,
            inflation: 6.0,
            monthly_expense_today: 50_000.0,
        };
        let p = project_retirement(&input);
        assert_eq!(p.years, 30.0);
        // Target: 50k*12 inflated 6% over 30y, times 25.
        let expected_target = inflate(600_000.0, 6.0, 30.0) * 25.0;
        assert!(close(p.target_corpus, expected_target, 1.0));
        // Projection and shortfall are internally consistent.
        assert!(p.projected_corpus > 0.0);
        if p.on_track {
            assert_eq!(p.shortfall, 0.0);
            assert_eq!(p.extra_monthly, 0.0);
        } else {
            assert!(p.shortfall > 0.0);
        }
    }

    #[test]
    fn goal_progress_clamps_percent_and_computes_a_required_saving() {
        // Target date ten years out from a fixed "today".
        let today = 1_760_000_000; // ~Oct 2025
        let ten_years = today + (10.0 * 365.25 * 86_400.0) as i64;
        let date = unix_to_ymd(ten_years);
        let g = goal_progress(1, "House".into(), 2_000_000.0, 500_000.0, &date, 9.0, today);
        assert!(close(g.percent, 25.0, 0.5));
        assert!(g.years_left > 9.5 && g.years_left < 10.5, "{}", g.years_left);
        assert!(g.required_monthly.unwrap() > 0.0);
    }

    #[test]
    fn an_already_met_goal_needs_nothing_and_caps_at_full() {
        let today = 1_760_000_000;
        let date = unix_to_ymd(today + (5.0 * 365.25 * 86_400.0) as i64);
        let g = goal_progress(1, "Fund".into(), 100_000.0, 150_000.0, &date, 8.0, today);
        assert_eq!(g.percent, 100.0);
        assert_eq!(g.required_monthly.unwrap(), 0.0);
    }

    #[test]
    fn a_bad_date_does_not_panic() {
        let g = goal_progress(1, "X".into(), 1000.0, 0.0, "not-a-date", 8.0, 1_760_000_000);
        assert_eq!(g.required_monthly, None);
    }

    /// Round-trips through the civil-date math, so the date test above is
    /// trustworthy.
    fn unix_to_ymd(unix: i64) -> String {
        let days = unix / 86_400 + 719_468;
        let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
        let doe = days - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        format!("{y:04}-{m:02}-{d:02}")
    }

    #[test]
    fn the_civil_date_parse_matches_known_dates() {
        // 1970-01-01 is epoch 0; 2000-01-01 is 946684800.
        assert_eq!(parse_ymd_to_unix("1970-01-01"), Some(0));
        assert_eq!(parse_ymd_to_unix("2000-01-01"), Some(946_684_800));
    }
}
