//! What suits the user's own ULIP: its premiums, term and their retirement,
//! read against the funds' records.
//!
//! # The reasoning, written out
//!
//! - **Horizon decides the risk.** Money not needed for ten years or more can
//!   sit in equity and ride out falls; inside ten years of retirement it moves
//!   into debt a little each year (a glide path), so a crash just before
//!   retiring cannot take half of it.
//! - **"Best" means the best record for that horizon.** Funds are scored on
//!   long-run return, the typical and worst 1-year hold, how often a hold
//!   made money, and the charge. With decades to go return weighs most; near
//!   retirement steadiness does. Only funds with a three-year record are
//!   scored: a younger one says too little. A figure the source does not
//!   publish (Tata AIA gives no daily history, so no 1-year holds) counts as
//!   middling rather than as good or bad.
//! - **Premiums, not switches.** Premium allocation changes where new money
//!   goes; the money already in stays put unless a held fund has earned a
//!   switch. Half of new premiums goes to the two best-scored funds and half
//!   stays with the user's own picks while they build a record, so the plan
//!   is neither all-in on the past winner nor blind to it.
//!
//! None of this predicts returns. The projection is arithmetic at fixed
//! rates, after fund charges but before the policy's own charges.

use serde::{Deserialize, Serialize};

use super::mf_connector::civil_from_days;
use super::ulip_switch::{day_of, safest_debt, FundStats, Kind, SwitchReport, Verdict};

/// The policy's own terms and the holder's retirement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// yyyy-mm-dd, the first premium.
    pub started: String,
    pub monthly_premium: f64,
    pub pay_years: i64,
    pub term_years: i64,
    pub birth_year: i64,
    pub retire_age: i64,
}

impl Plan {
    pub fn check(&self, this_year: i64) -> Result<(), String> {
        if day_of(&self.started).is_none() {
            return Err("The start date must be a date (yyyy-mm-dd).".to_string());
        }
        if !(self.monthly_premium > 0.0 && self.monthly_premium.is_finite()) {
            return Err("The monthly premium must be above zero.".to_string());
        }
        if !(1..=40).contains(&self.pay_years) || !(1..=100).contains(&self.term_years) {
            return Err("Payment term is 1 to 40 years and policy term 1 to 100.".to_string());
        }
        if !(1930..=this_year - 18).contains(&self.birth_year) {
            return Err("Enter the year you were born.".to_string());
        }
        if !(40..=80).contains(&self.retire_age) {
            return Err("Retirement age is 40 to 80.".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlideStep {
    pub year: i64,
    pub age: i64,
    /// % of the policy in equity.
    pub equity: f64,
    pub note: String,
}

/// A fund scored for this horizon.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fit {
    pub label: String,
    /// 0 to 100, against the other scored funds.
    pub score: f64,
    pub rank: usize,
    pub why: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MixPart {
    pub label: String,
    /// % of each new premium.
    pub share: f64,
    pub held: bool,
    pub why: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scenario {
    pub label: String,
    /// % a year.
    pub rate: f64,
    /// (year, value at the end of it).
    pub points: Vec<(i64, f64)>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanAdvice {
    pub plan: Plan,
    pub age: i64,
    pub pay_until: String,
    pub premiums_left: i64,
    pub paid: f64,
    pub to_pay: f64,
    pub retire_year: i64,
    pub years_to_retire: i64,
    pub maturity_year: i64,
    pub stage: String,
    pub stage_note: String,
    pub equity_now: f64,
    pub glide: Vec<GlideStep>,
    pub fits: Vec<Fit>,
    pub best: Option<String>,
    pub best_why: String,
    pub mix: Vec<MixPart>,
    pub projection: Vec<Scenario>,
    /// (year, premiums paid by the end of it).
    pub paid_line: Vec<(i64, f64)>,
}

/// Inside this many years of retirement, money starts moving into debt.
const GLIDE_YEARS: i64 = 10;
/// Equity left at retirement, for growth through a long retirement.
const EQUITY_AT_RETIREMENT: f64 = 40.0;
pub const RATES: [(&str, f64); 3] = [("Cautious", 8.0), ("Middle", 10.0), ("Strong", 12.0)];

/// Equity share for a number of years to retirement.
pub fn equity_for(years_left: i64) -> f64 {
    if years_left >= GLIDE_YEARS {
        100.0
    } else {
        let f = years_left.max(0) as f64 / GLIDE_YEARS as f64;
        EQUITY_AT_RETIREMENT + (100.0 - EQUITY_AT_RETIREMENT) * f
    }
}

/// Months from the start to `today`, counting the first premium.
fn premiums_paid(plan: &Plan, today: i64) -> i64 {
    let (Some(start), total) = (day_of(&plan.started), plan.pay_years * 12) else { return 0 };
    if today < start {
        return 0;
    }
    let (sy, sm, sd) = civil_from_days(start);
    let (ty, tm, td) = civil_from_days(today);
    let months = (ty - sy) * 12 + (tm as i64 - sm as i64) - i64::from(td < sd);
    (months + 1).clamp(0, total)
}

/// Weights for (long-run return, typical hold, share of holds that made
/// money, worst hold, charge) at this horizon.
fn weights(years_left: i64) -> [f64; 5] {
    if years_left >= 15 {
        [0.40, 0.20, 0.15, 0.15, 0.10]
    } else if years_left >= 8 {
        [0.30, 0.20, 0.20, 0.20, 0.10]
    } else {
        [0.20, 0.15, 0.25, 0.30, 0.10]
    }
}

/// Equity funds with a long enough record, scored for the horizon.
pub fn score(funds: &[FundStats], years_left: i64) -> Vec<Fit> {
    let eligible: Vec<(&FundStats, [Option<f64>; 5])> = funds
        .iter()
        .filter(|f| f.kind == Kind::Equity)
        .filter_map(|f| {
            let ret = f.cagr5y.or(f.cagr3y)?;
            let r = f.rolling.as_ref();
            Some((f, [Some(ret), r.map(|r| r.median), r.map(|r| r.positive), r.map(|r| r.worst), f.fmc.map(|c| -c)]))
        })
        .collect();
    let w = weights(years_left);
    let range = |i: usize| {
        let xs = eligible.iter().filter_map(|(_, v)| v[i]);
        (xs.clone().fold(f64::INFINITY, f64::min), xs.fold(f64::NEG_INFINITY, f64::max))
    };
    let ranges: Vec<(f64, f64)> = (0..5).map(range).collect();
    let mut fits: Vec<(f64, &FundStats)> = eligible
        .iter()
        .map(|(f, v)| {
            let s: f64 = (0..5)
                .map(|i| {
                    let (lo, hi) = ranges[i];
                    let norm = match v[i] {
                        Some(x) if hi > lo => (x - lo) / (hi - lo),
                        Some(_) => 1.0,
                        None => 0.5,
                    };
                    norm * w[i]
                })
                .sum();
            (s * 100.0, *f)
        })
        .collect();
    fits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    fits.iter()
        .enumerate()
        .map(|(i, (s, f))| {
            let (ret, span) = match f.cagr5y {
                Some(c) => (c, "5"),
                None => (f.cagr3y.unwrap_or_default(), "3"),
            };
            let mut why = format!("{ret:.1}% a year over {span} years");
            if let Some(r) = &f.rolling {
                why += &format!("; {:.0}% of 1-year holds made money, typically {:+.1}%, worst {:+.1}%", r.positive, r.median, r.worst);
            }
            if let Some(b) = f.bench_3y {
                why += &format!("; its benchmark {b:.1}% a year over 3 years");
            }
            if let Some(c) = f.fmc {
                why += &format!("; charge {c:.2}%");
            }
            Fit { label: f.label.clone(), score: *s, rank: i + 1, why: why + "." }
        })
        .collect()
}

pub fn advise(plan: &Plan, report: &SwitchReport, value: f64, today: i64) -> PlanAdvice {
    let (this_year, _, _) = civil_from_days(today);
    let age = this_year - plan.birth_year;
    let retire_year = plan.birth_year + plan.retire_age;
    let years_to_retire = retire_year - this_year;
    let start = day_of(&plan.started).unwrap_or(today);
    let (sy, sm, sd) = civil_from_days(start);
    let pay_until = format!("{:04}-{sm:02}-{sd:02}", sy + plan.pay_years);
    let paid_count = premiums_paid(plan, today);
    let premiums_left = plan.pay_years * 12 - paid_count;
    let paid = paid_count as f64 * plan.monthly_premium;
    let maturity_year = sy + plan.term_years;
    let equity_now = equity_for(years_to_retire);

    let debt_fund = safest_debt(&report.funds).map_or_else(|| "a debt fund".to_string(), |f| f.label.clone());

    let (stage, stage_note) = if years_to_retire >= GLIDE_YEARS {
        (
            "Growth".to_string(),
            format!(
                "{years_to_retire} years to retirement at {}, and premiums run only until {}. Every rupee here has decades to grow, so all of it belongs in equity; a fall of 15 to 20% in a bad year is time to wait out, not a reason to move.",
                plan.retire_age,
                &pay_until[..4]
            ),
        )
    } else if years_to_retire > 0 {
        (
            "Glide".to_string(),
            format!("{years_to_retire} years to retirement: move a slice into {debt_fund} each year so a late crash can't undo decades of growth."),
        )
    } else {
        ("Retirement".to_string(), format!("Keep about {EQUITY_AT_RETIREMENT:.0}% in equity for growth through retirement and the rest in {debt_fund}."))
    };

    let glide_year = retire_year - GLIDE_YEARS;
    let mut glide = vec![GlideStep {
        year: this_year,
        age,
        equity: equity_now,
        note: if years_to_retire >= GLIDE_YEARS { "All equity".to_string() } else { "Gliding".to_string() },
    }];
    if glide_year > this_year {
        glide.push(GlideStep {
            year: glide_year,
            age: plan.retire_age - GLIDE_YEARS,
            equity: 100.0,
            note: format!("Start moving about 6% a year into {debt_fund}"),
        });
    }
    if retire_year - 5 > this_year {
        glide.push(GlideStep { year: retire_year - 5, age: plan.retire_age - 5, equity: equity_for(5), note: "Halfway into safety".to_string() });
    }
    if retire_year > this_year {
        glide.push(GlideStep {
            year: retire_year,
            age: plan.retire_age,
            equity: EQUITY_AT_RETIREMENT,
            note: format!("{:.0}% safe, for withdrawals", 100.0 - EQUITY_AT_RETIREMENT),
        });
    }

    let fits = score(&report.funds, years_to_retire);
    let horizon = if years_to_retire >= 15 {
        "a horizon of decades, so long-run return weighs most"
    } else if years_to_retire >= 8 {
        "a medium horizon, so return and steadiness weigh about equally"
    } else {
        "a short horizon, so steadiness and the worst year weigh most"
    };
    let best = fits.first().map(|f| f.label.clone());
    let best_why = match fits.first() {
        Some(f) if fits.len() == 1 => {
            format!("The only fund in the plan with three years of record, so the only one that can be judged for {horizon}. {}", f.why)
        }
        Some(f) => format!("Scored highest of {} funds with a long enough record, for {horizon}. {}", fits.len(), f.why),
        None => "No fund in the plan has three years of record yet, so none can be called best; your own picks are as good a bet as any.".to_string(),
    };

    let mix = mix(report, &fits, equity_now, &debt_fund, premiums_left > 0);

    let mut projection = Vec::new();
    for (label, rate) in RATES {
        let monthly = (1.0 + rate / 100.0).powf(1.0 / 12.0);
        let mut v = value;
        let mut left = premiums_left;
        let mut points = vec![(this_year, value)];
        for year in this_year + 1..=retire_year.max(this_year + 1) {
            for _ in 0..12 {
                v *= monthly;
                if left > 0 {
                    v += plan.monthly_premium;
                    left -= 1;
                }
            }
            points.push((year, v));
        }
        projection.push(Scenario { label: label.to_string(), rate, points });
    }
    let paid_line = {
        let mut total = paid;
        let mut left = premiums_left;
        let mut out = vec![(this_year, paid)];
        for year in this_year + 1..=retire_year.max(this_year + 1) {
            let n = left.min(12);
            total += n as f64 * plan.monthly_premium;
            left -= n;
            out.push((year, total));
        }
        out
    };

    PlanAdvice {
        plan: plan.clone(),
        age,
        pay_until,
        premiums_left,
        paid,
        to_pay: premiums_left as f64 * plan.monthly_premium,
        retire_year,
        years_to_retire,
        maturity_year,
        stage,
        stage_note,
        equity_now,
        glide,
        fits,
        best,
        best_why,
        mix,
        projection,
        paid_line,
    }
}

/// How each new premium could be split.
fn mix(report: &SwitchReport, fits: &[Fit], equity_now: f64, debt_fund: &str, paying: bool) -> Vec<MixPart> {
    if !paying {
        return Vec::new();
    }
    let review = report.review_on.clone().unwrap_or_default();
    let keep: Vec<&str> = report.held.iter().filter(|h| h.verdict != Verdict::Switch).map(|h| h.label.as_str()).collect();
    let top: Vec<&Fit> = fits.iter().take(2).collect();
    let equity = equity_now;
    let mut parts: Vec<MixPart> = Vec::new();
    let mut add = |label: &str, share: f64, why: String| {
        if share <= 0.0 {
            return;
        }
        let held = report.held.iter().any(|h| h.label == label);
        match parts.iter_mut().find(|p| p.label == label) {
            Some(p) => p.share += share,
            None => parts.push(MixPart { label: label.to_string(), share, held, why }),
        }
    };
    let (proven_share, own_share) = match (top.is_empty(), keep.is_empty()) {
        (true, _) => (0.0, equity),
        (false, true) => (equity, 0.0),
        (false, false) => (equity / 2.0, equity / 2.0),
    };
    for (i, f) in top.iter().enumerate() {
        let why = if i == 0 {
            "Best record for your horizon (rank 1). Proven over years, not months.".to_string()
        } else {
            "Second-best record, with a different strategy, so one style's bad patch hurts less.".to_string()
        };
        add(&f.label, proven_share / top.len() as f64, why);
    }
    for label in &keep {
        add(
            label,
            own_share / keep.len() as f64,
            format!("Your pick; keep it while it builds a record, then judge it on {review}."),
        );
    }
    add(debt_fund, 100.0 - equity, "The safe slice for your stage.".to_string());
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::ulip_switch::{HeldFund, Rolling};

    fn fund(label: &str, cagr5: f64, positive: f64, worst: f64) -> FundStats {
        FundStats {
            label: label.into(),
            kind: Kind::Equity,
            fmc: Some(1.25),
            nav: 10.0,
            nav_date: "2026-11-03".into(),
            cagr3y: Some(cagr5),
            cagr5y: Some(cagr5),
            rolling: Some(Rolling { holds: 200, positive, median: cagr5, worst, best: 40.0 }),
            ..Default::default()
        }
    }

    fn plan() -> Plan {
        Plan { started: "2026-10-31".into(), monthly_premium: 5000.0, pay_years: 10, term_years: 75, birth_year: 2001, retire_age: 60 }
    }

    fn report(funds: Vec<FundStats>, held: &[(&str, Verdict)]) -> SwitchReport {
        SwitchReport {
            insurer: "PNB MetLife".into(),
            as_of: "2026-11-03".into(),
            funds,
            held: held
                .iter()
                .map(|(l, v)| HeldFund { label: l.to_string(), units: 1.0, value: 10.0, weight: 50.0, verdict: *v, reason: String::new() })
                .collect(),
            headline: String::new(),
            points: vec![],
            alternatives: vec![],
            ideas: vec![],
            review_on: Some("2027-10-30".into()),
            failed: vec![],
            plan: None,
        }
    }

    /// Run with `cargo test -- --ignored` to see the advice on today's NAVs.
    #[test]
    #[ignore = "network"]
    fn advice_on_live_navs() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let held = [("Momentum Quality Index Fund".to_string(), 240.53), ("Value Fund".to_string(), 222.82)];
        let group = crate::assistant::portfolio_chat::PNB_METLIFE_GROUP;
        let r = crate::assistant::ulip_switch::build(group, &held, &[], &[], false, now).unwrap();
        let value = r.held.iter().map(|h| h.value).sum();
        let a = advise(&plan(), &r, value, now / 86_400);
        println!("{}", serde_json::to_string_pretty(&a).unwrap());
    }

    #[test]
    fn the_glide_path_is_all_equity_until_ten_years_out() {
        assert_eq!(equity_for(34), 100.0);
        assert_eq!(equity_for(10), 100.0);
        assert_eq!(equity_for(5), 70.0);
        assert_eq!(equity_for(0), 40.0);
        assert_eq!(equity_for(-3), 40.0);
    }

    #[test]
    fn premiums_paid_count_the_first_month() {
        let p = plan();
        assert_eq!(premiums_paid(&p, day_of("2026-11-04").unwrap()), 1);
        assert_eq!(premiums_paid(&p, day_of("2026-11-30").unwrap()), 1, "the 30th comes before the 31st");
        assert_eq!(premiums_paid(&p, day_of("2027-10-31").unwrap()), 13);
        assert_eq!(premiums_paid(&p, day_of("2040-01-01").unwrap()), 120, "capped at the payment term");
    }

    #[test]
    fn a_long_horizon_favours_return_and_a_short_one_steadiness() {
        let funds = vec![fund("Racer", 18.0, 80.0, -15.0), fund("Steady", 11.0, 100.0, 2.0)];
        assert_eq!(score(&funds, 34)[0].label, "Racer");
        assert_eq!(score(&funds, 4)[0].label, "Steady");
    }

    #[test]
    fn young_funds_are_not_scored() {
        let mut young = fund("Young", 30.0, 100.0, 5.0);
        (young.cagr3y, young.cagr5y) = (None, None);
        assert!(score(&[young, fund("Old", 10.0, 80.0, -5.0)], 34).iter().all(|f| f.label != "Young"));
    }

    #[test]
    fn a_fund_without_daily_history_is_scored_on_what_is_published() {
        let mut factsheet_only = fund("Factsheet", 13.5, 0.0, 0.0);
        (factsheet_only.rolling, factsheet_only.fmc) = (None, None);
        let fits = score(&[factsheet_only], 34);
        assert_eq!(fits.len(), 1);
        assert!(!fits[0].why.contains("holds"), "{}", fits[0].why);
    }

    #[test]
    fn half_of_new_premiums_go_to_the_best_records_and_half_to_your_picks() {
        let r = report(
            vec![fund("A", 17.0, 90.0, -8.0), fund("B", 13.0, 87.0, -9.0), fund("C", 9.0, 60.0, -13.0)],
            &[("Value Fund", Verdict::Keep), ("Momentum Quality Index Fund", Verdict::TooNew)],
        );
        let a = advise(&plan(), &r, 4833.0, day_of("2026-11-04").unwrap());
        assert_eq!(a.age, 25);
        assert_eq!(a.years_to_retire, 35);
        assert_eq!(a.best.as_deref(), Some("A"));
        let shares: Vec<(&str, f64)> = a.mix.iter().map(|m| (m.label.as_str(), m.share)).collect();
        assert_eq!(shares, vec![("A", 25.0), ("B", 25.0), ("Value Fund", 25.0), ("Momentum Quality Index Fund", 25.0)]);
        assert_eq!(a.premiums_left, 119);
        assert_eq!(a.pay_until, "2036-10-31");
        assert_eq!(a.maturity_year, 2101);
    }

    #[test]
    fn a_fund_that_earned_a_switch_gets_no_new_premiums() {
        let r = report(vec![fund("A", 17.0, 90.0, -8.0), fund("B", 13.0, 87.0, -9.0)], &[("Laggard", Verdict::Switch)]);
        let a = advise(&plan(), &r, 0.0, day_of("2026-11-04").unwrap());
        assert!(a.mix.iter().all(|m| m.label != "Laggard"));
        assert_eq!(a.mix.iter().map(|m| m.share).sum::<f64>(), 100.0);
    }

    #[test]
    fn the_projection_adds_premiums_only_while_they_are_due() {
        let r = report(vec![], &[]);
        let a = advise(&plan(), &r, 5000.0, day_of("2026-11-04").unwrap());
        let paid = a.paid_line.last().unwrap().1;
        assert_eq!(paid, 600_000.0, "120 premiums of 5,000");
        let middle = &a.projection[1];
        assert_eq!(middle.points.last().unwrap().0, 2061);
        assert!(middle.points.last().unwrap().1 > paid * 5.0, "35 years at 10% grows well past what was paid");
    }
}
