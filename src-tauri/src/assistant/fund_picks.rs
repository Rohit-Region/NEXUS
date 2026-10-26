//! Fund suggestions: small-cap mutual funds ranked for a small monthly SIP.
//!
//! # Why small-cap funds
//!
//! For someone investing a few hundred rupees a month, a fund is how to own a
//! basket of growing smaller companies without picking them one by one: the
//! fund's research team does the picking, and a SIP starts at ₹100 to ₹500.
//!
//! # The model
//!
//! Every open, actively managed, direct-plan small-cap fund Groww lists with
//! at least three years of history is scored on eight factors, each 0 to 100:
//!
//! | Factor              | Weight | From                                                  |
//! |---------------------|--------|-------------------------------------------------------|
//! | Track record        | 25     | rank within the category over 3Y (half), 5Y, 1Y      |
//! | Risk-adjusted return| 20     | Sharpe, Sortino, alpha against its benchmark          |
//! | Money flow & size   | 15     | net inflows (AUM growth beyond NAV growth), AUM sweet spot |
//! | Steadiness          | 10     | standard deviation and beta, lower is better          |
//! | Cost                | 10     | expense ratio, lower is better                        |
//! | Portfolio           | 10     | holding count, top-10 concentration, cash level       |
//! | Manager             | 5      | tenure of the longest-serving current manager         |
//! | Small-ticket        | 5      | minimum SIP                                           |
//!
//! Ratios are ranked against the other funds (a percentile); the rest are
//! scored on fixed bands stated beside each rule. A factor with no data is
//! dropped and the rest reweighted.
//!
//! # Money flow
//!
//! No free source publishes a fund's monthly inflows. NEXUS records each
//! fund's AUM and NAV once a day when this screen loads, and once it holds
//! 25 days of history computes implied net flow: AUM growth not explained by
//! NAV growth. Until then the factor runs on fund size alone, and the screen
//! says how many days it has collected.
//!
//! # AUM sweet spot
//!
//! Small companies trade thinly. A very small fund (under ₹500 Cr) is fragile
//! and costly to run; a very large one (over ₹30,000 Cr) struggles to buy
//! meaningful stakes in small companies without moving their prices, and
//! drifts toward bigger names. Between those it scores in full.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::groww;

pub const W_TRACK: f64 = 25.0;
pub const W_RISK_ADJ: f64 = 20.0;
pub const W_FLOW: f64 = 15.0;
pub const W_STEADY: f64 = 10.0;
pub const W_COST: f64 = 10.0;
pub const W_PORTFOLIO: f64 = 10.0;
pub const W_MANAGER: f64 = 5.0;
pub const W_TICKET: f64 = 5.0;

/// Fund statistics are published daily at most.
const DETAILS_FOR: Duration = Duration::from_secs(12 * 3600);
const IN_FLIGHT: usize = 6;
/// History needed before implied flow is trusted.
const FLOW_MIN_DAYS: i64 = 25;
/// Snapshots older than this are dropped.
const HISTORY_DAYS: i64 = 400;

static DETAILS: Mutex<Option<(Instant, Vec<Value>)>> = Mutex::new(None);

/// One fund's AUM and NAV on one day.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub day: i64,
    pub aum: f64,
    pub nav: f64,
}

/// Scheme code to its daily snapshots, oldest first. Stored by the caller.
pub type FlowHistory = HashMap<String, Vec<Snapshot>>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Factor {
    pub name: String,
    pub score: Option<f64>,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Weighted {
    pub name: String,
    pub pct: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FundPick {
    pub code: String,
    /// Groww page id, for a link to the fund.
    pub id: String,
    pub name: String,
    pub fund_house: Option<String>,
    pub score: f64,
    /// "Top pick", "Good", "Average" or "Weak".
    pub verdict: String,
    pub factors: Vec<Factor>,
    pub reasons: Vec<String>,
    pub risks: Vec<String>,
    /// The NAV Groww showed, as recorded by the pick journal.
    pub nav: Option<f64>,
    pub aum: Option<f64>,
    pub expense_ratio: Option<f64>,
    pub min_sip: Option<f64>,
    pub min_lumpsum: Option<f64>,
    pub ret_1y: Option<f64>,
    pub ret_3y: Option<f64>,
    pub ret_5y: Option<f64>,
    pub cat_ret_3y: Option<f64>,
    pub sharpe: Option<f64>,
    pub std_dev: Option<f64>,
    pub alpha: Option<f64>,
    pub manager: Option<String>,
    pub manager_years: Option<f64>,
    /// Implied net inflow over the recorded window, percent of AUM.
    pub flow_pct: Option<f64>,
    pub exit_load: Option<String>,
    pub top_holdings: Vec<Weighted>,
    pub top_sectors: Vec<Weighted>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FundPicks {
    /// Best score first.
    pub funds: Vec<FundPick>,
    /// Funds left out for having under three years of history.
    pub too_new: Vec<String>,
    /// Days of AUM history held; flows count from `FLOW_MIN_DAYS`.
    pub flow_days: i64,
    pub flow_min_days: i64,
    pub gaps: Vec<String>,
    pub fetched_at: i64,
}

/// Rank the funds, recording today's AUM snapshot into `history`.
pub fn picks(refresh: bool, now: i64, history: &mut FlowHistory) -> Result<FundPicks, String> {
    let (details, mut gaps) = load_details(refresh)?;
    let today = now / 86_400;

    let mut eligible = Vec::new();
    let mut too_new = Vec::new();
    for d in &details {
        let open = d.get("available_for_investment").and_then(|v| v.as_bool()).unwrap_or(false)
            && d.get("sip_allowed").and_then(|v| v.as_bool()).unwrap_or(false);
        let small_cap = str_at(d, "/sub_category").is_some_and(|s| s.eq_ignore_ascii_case("Small Cap"));
        if !open || !small_cap {
            continue;
        }
        if num_at(d, "/return_stats/0/rank3yr").is_none() {
            too_new.push(name_of(d));
            continue;
        }
        eligible.push(d);
    }
    if eligible.is_empty() {
        return Err("No small-cap fund data could be read. Is the network up?".to_string());
    }

    for d in &eligible {
        if let (Some(code), Some(aum), Some(nav)) = (str_at(d, "/scheme_code"), num_at(d, "/aum"), num_at(d, "/nav")) {
            let snaps = history.entry(code).or_default();
            snaps.retain(|s| s.day != today && s.day > today - HISTORY_DAYS);
            snaps.push(Snapshot { day: today, aum, nav });
            snaps.sort_by_key(|s| s.day);
        }
    }
    let flow_days = history
        .values()
        .filter_map(|s| Some(s.last()?.day - s.first()?.day))
        .max()
        .unwrap_or(0);

    let mut funds = score_all(&eligible, history, now);
    funds.sort_by(|a, b| b.score.total_cmp(&a.score));
    for (i, f) in funds.iter_mut().enumerate() {
        f.verdict = if i < 3 && f.score >= 55.0 {
            "Top pick"
        } else if f.score >= 60.0 {
            "Good"
        } else if f.score >= 45.0 {
            "Average"
        } else {
            "Weak"
        }
        .to_string();
    }
    if flow_days < FLOW_MIN_DAYS {
        gaps.push(format!(
            "money flow uses fund size only until {FLOW_MIN_DAYS} days of history are recorded ({flow_days} so far)"
        ));
    }

    Ok(FundPicks { funds, too_new, flow_days, flow_min_days: FLOW_MIN_DAYS, gaps, fetched_at: now })
}

fn str_at(v: &Value, p: &str) -> Option<String> {
    match v.pointer(p)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A number, whether Groww sent it as one or as a string (`expense_ratio` is
/// a string in the scheme body).
fn num_at(v: &Value, p: &str) -> Option<f64> {
    match v.pointer(p)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn name_of(d: &Value) -> String {
    str_at(d, "/scheme_name").or_else(|| str_at(d, "/fund_name")).unwrap_or_else(|| "(unnamed)".into())
}

/// Every candidate's Groww scheme body, from cache when fresh.
fn load_details(refresh: bool) -> Result<(Vec<Value>, Vec<String>), String> {
    if !refresh {
        if let Some((at, v)) = DETAILS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if at.elapsed() < DETAILS_FOR {
                return Ok((v.clone(), Vec::new()));
            }
        }
    }
    let candidates = candidates()?;
    let mut out = Vec::new();
    let mut failed = 0;
    for chunk in candidates.chunks(IN_FLIGHT) {
        let batch: Vec<Result<Value, String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|id| scope.spawn(move || groww::scheme(id)))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("Failed".into()))).collect()
        });
        for r in batch {
            match r {
                Ok(v) => out.push(v),
                Err(e) => {
                    failed += 1;
                    log::warn!("fund_picks: {e}");
                }
            }
        }
    }
    // The same fund can appear under two AMFI codes after a merger.
    let mut seen = std::collections::HashSet::new();
    out.retain(|v| seen.insert(str_at(v, "/search_id").unwrap_or_default()));

    let gaps = if failed > 0 { vec![format!("{failed} funds could not be read")] } else { Vec::new() };
    if !out.is_empty() {
        *DETAILS.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), out.clone()));
    }
    Ok((out, gaps))
}

/// Groww page ids of the actively managed small-cap funds.
fn candidates() -> Result<Vec<String>, String> {
    let all = groww::category_funds("Small Cap")?;
    if all.is_empty() {
        return Err("Groww listed no small-cap funds.".to_string());
    }
    Ok(all.into_iter().filter(|(_, name)| is_active(name)).map(|(id, _)| id).collect())
}

/// Actively managed: not an index fund, ETF or fund-of-funds. Those track a
/// list rather than research companies, which is what the score rewards.
fn is_active(name: &str) -> bool {
    let n = name.to_lowercase();
    const PASSIVE: &[&str] = &["index", "etf", "fof", "fund of fund", "smallcap 250", "smallcap 50", "smallcap250"];
    !PASSIVE.iter().any(|x| n.contains(x))
}

/// Implied net flow in percent: AUM growth not explained by NAV growth.
pub fn implied_flow(snaps: &[Snapshot]) -> Option<f64> {
    let last = snaps.last()?;
    let first = snaps.iter().find(|s| last.day - s.day >= FLOW_MIN_DAYS)?;
    if first.aum <= 0.0 || first.nav <= 0.0 || last.nav <= 0.0 {
        return None;
    }
    Some(((last.aum / first.aum) / (last.nav / first.nav) - 1.0) * 100.0)
}

/// AUM in ₹ Cr, scored on the bands described in the module notes.
pub fn aum_band(aum: f64) -> f64 {
    match aum {
        a if a < 300.0 => 0.1,
        a if a < 500.0 => 0.4,
        a if a < 1_000.0 => 0.7,
        a if a <= 30_000.0 => 1.0,
        a if a <= 50_000.0 => 0.7,
        _ => 0.5,
    }
}

pub fn ticket_band(min_sip: f64) -> f64 {
    match min_sip {
        m if m <= 100.0 => 1.0,
        m if m <= 500.0 => 0.8,
        m if m <= 1_000.0 => 0.5,
        _ => 0.2,
    }
}

/// Each value's rank among the present ones, 0 (lowest) to 1 (highest).
fn percentiles(values: &[Option<f64>]) -> Vec<Option<f64>> {
    let mut present: Vec<f64> = values.iter().flatten().copied().collect();
    present.sort_by(|a, b| a.total_cmp(b));
    let n = present.len();
    values
        .iter()
        .map(|v| {
            v.map(|x| {
                if n < 2 {
                    return 0.5;
                }
                let below = present.partition_point(|p| *p < x);
                let equal = present[below..].partition_point(|p| *p <= x);
                (below as f64 + equal.saturating_sub(1) as f64 / 2.0) / (n - 1) as f64
            })
        })
        .collect()
}

fn blend(parts: &[(Option<f64>, f64)]) -> Option<f64> {
    let (sum, w) = parts
        .iter()
        .filter_map(|(v, w)| v.map(|v| (v * w, *w)))
        .fold((0.0, 0.0), |(s, t), (a, b)| (s + a, t + b));
    (w > 0.0).then(|| sum / w)
}

/// Category rank as a 0..1 score: first of N is 1, last is 0.
fn rank_score(d: &Value, key: &str) -> Option<f64> {
    let rank = num_at(d, &format!("/return_stats/0/rank{key}"))?;
    let count = num_at(d, &format!("/return_stats/0/rank_count{key}"))?;
    (count > 1.0).then(|| (1.0 - (rank - 1.0) / (count - 1.0)).clamp(0.0, 1.0))
}

struct Portfolio {
    count: usize,
    top10: f64,
    cash: f64,
    holdings: Vec<Weighted>,
    sectors: Vec<Weighted>,
}

fn portfolio(d: &Value) -> Option<Portfolio> {
    let list = d.get("holdings")?.as_array()?;
    let mut equity: Vec<(String, String, f64)> = Vec::new();
    let mut cash = 0.0;
    for h in list {
        let pct = h.get("corpus_per").and_then(|x| x.as_f64()).unwrap_or(0.0);
        if h.get("nature_name").and_then(|x| x.as_str()) == Some("EQUITY") {
            equity.push((
                str_at(h, "/company_name").unwrap_or_default(),
                str_at(h, "/sector_name").unwrap_or_else(|| "Other".into()),
                pct,
            ));
        } else {
            cash += pct;
        }
    }
    if equity.is_empty() {
        return None;
    }
    equity.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut sectors: HashMap<String, f64> = HashMap::new();
    for (_, s, p) in &equity {
        *sectors.entry(s.clone()).or_default() += p;
    }
    let mut sectors: Vec<Weighted> = sectors.into_iter().map(|(name, pct)| Weighted { name, pct }).collect();
    sectors.sort_by(|a, b| b.pct.total_cmp(&a.pct));
    sectors.truncate(3);
    Some(Portfolio {
        count: equity.len(),
        top10: equity.iter().take(10).map(|e| e.2).sum(),
        cash,
        holdings: equity.iter().take(5).map(|(n, _, p)| Weighted { name: n.clone(), pct: *p }).collect(),
        sectors,
    })
}

fn score_all(funds: &[&Value], history: &FlowHistory, now: i64) -> Vec<FundPick> {
    let rs = |k: &str| funds.iter().map(|d| num_at(d, &format!("/return_stats/0/{k}"))).collect::<Vec<_>>();
    let inv = |v: Vec<Option<f64>>| v.into_iter().map(|x| x.map(|y| -y)).collect::<Vec<_>>();
    let sharpe = percentiles(&rs("sharpe_ratio"));
    let sortino = percentiles(&rs("sortino_ratio"));
    let alpha = percentiles(&rs("alpha"));
    let low_sd = percentiles(&inv(rs("standard_deviation")));
    let low_beta = percentiles(&inv(rs("beta")));
    let low_cost = percentiles(&inv(funds.iter().map(|d| num_at(d, "/expense_ratio")).collect()));
    let flows: Vec<Option<f64>> = funds
        .iter()
        .map(|d| str_at(d, "/scheme_code").and_then(|c| history.get(&c)).and_then(|s| implied_flow(s)))
        .collect();
    let flow_rank = percentiles(&flows);
    let low_conc = percentiles(&inv(funds.iter().map(|d| portfolio(d).map(|p| p.top10)).collect()));

    funds
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let stat = |k: &str| num_at(d, &format!("/return_stats/0/{k}"));
            let aum = num_at(d, "/aum");
            let expense = num_at(d, "/expense_ratio");
            let min_sip = num_at(d, "/min_sip_investment");
            let port = portfolio(d);

            // Longest-serving current manager.
            let manager = d
                .get("fund_manager_details")
                .and_then(|m| m.as_array())
                .and_then(|ms| {
                    ms.iter()
                        .filter_map(|m| {
                            let from = super::news_desk::parse_date(m.get("date_from")?.as_str()?)?;
                            Some((str_at(m, "/person_name")?, (now - from) as f64 / (365.25 * 86_400.0)))
                        })
                        .max_by(|a, b| a.1.total_cmp(&b.1))
                });

            let track = blend(&[(rank_score(d, "3yr"), 0.5), (rank_score(d, "5yr"), 0.3), (rank_score(d, "1yr"), 0.2)]);
            let risk_adj = blend(&[(sharpe[i], 0.4), (sortino[i], 0.3), (alpha[i], 0.3)]);
            let flow = blend(&[(flow_rank[i], 0.6), (aum.map(aum_band), 0.4)]);
            let steady = blend(&[(low_sd[i], 0.6), (low_beta[i], 0.4)]);
            let portfolio_s = port.as_ref().map(|p| {
                let count = match p.count {
                    c if c < 25 => 0.3,
                    c if c < 40 => 0.7,
                    c if c <= 150 => 1.0,
                    _ => 0.7,
                };
                let cash = if (2.0..=12.0).contains(&p.cash) { 1.0 } else if p.cash <= 20.0 { 0.5 } else { 0.2 };
                blend(&[(Some(count), 0.35), (low_conc[i], 0.35), (Some(cash), 0.3)]).unwrap_or(0.5)
            });
            let manager_s = manager.as_ref().map(|(_, y)| (y / 5.0).min(1.0));

            let factors = vec![
                Factor { name: "Track record".into(), score: track, weight: W_TRACK },
                Factor { name: "Risk-adjusted".into(), score: risk_adj, weight: W_RISK_ADJ },
                Factor { name: "Money flow".into(), score: flow, weight: W_FLOW },
                Factor { name: "Steadiness".into(), score: steady, weight: W_STEADY },
                Factor { name: "Cost".into(), score: low_cost[i], weight: W_COST },
                Factor { name: "Portfolio".into(), score: portfolio_s, weight: W_PORTFOLIO },
                Factor { name: "Manager".into(), score: manager_s, weight: W_MANAGER },
                Factor { name: "Small-ticket".into(), score: min_sip.map(ticket_band), weight: W_TICKET },
            ]
            .into_iter()
            .map(|f| Factor { score: f.score.map(|s| s * 100.0), ..f })
            .collect::<Vec<_>>();
            let score = blend(&factors.iter().map(|f| (f.score, f.weight)).collect::<Vec<_>>()).unwrap_or(0.0);

            let mut reasons = Vec::new();
            let mut risks = Vec::new();
            if let (Some(r), Some(c)) = (stat("rank3yr"), stat("rank_count3yr")) {
                if r <= (c / 4.0).ceil() {
                    reasons.push(format!("Ranked {r:.0} of {c:.0} small-cap funds over 3 years"));
                } else if r > c * 0.75 {
                    risks.push(format!("Ranked {r:.0} of {c:.0} over 3 years"));
                }
            }
            if let (Some(f3), Some(c3)) = (stat("return3y"), stat("cat_return3y")) {
                if f3 > c3 + 2.0 {
                    reasons.push(format!("{f3:.1}% a year over 3 years vs {c3:.1}% category average"));
                } else if f3 < c3 - 2.0 {
                    risks.push(format!("{f3:.1}% a year over 3 years, below the {c3:.1}% category average"));
                }
            }
            if let Some(a) = stat("alpha").filter(|a| *a > 3.0) {
                reasons.push(format!("Beat its benchmark by {a:.1}% a year (alpha)"));
            }
            if sharpe[i].is_some_and(|p| p >= 0.75) {
                reasons.push(format!("Top-quartile return per unit of risk (Sharpe {:.2})", stat("sharpe_ratio").unwrap_or(0.0)));
            }
            if low_sd[i].is_some_and(|p| p >= 0.75) {
                reasons.push("Among the least volatile small-cap funds".into());
            } else if low_sd[i].is_some_and(|p| p <= 0.2) {
                risks.push(format!("Among the most volatile: {:.1}% standard deviation", stat("standard_deviation").unwrap_or(0.0)));
            }
            if let Some(f) = flows[i] {
                if f > 2.0 {
                    reasons.push(format!("Net inflows of {f:.1}% of AUM in the recorded window"));
                } else if f < -2.0 {
                    risks.push(format!("Net outflows of {:.1}% of AUM in the recorded window", -f));
                }
            }
            if let Some(a) = aum {
                if a > 30_000.0 {
                    risks.push(format!("Very large for small caps (₹{:.0}k Cr): harder to stay nimble", a / 1000.0));
                } else if a < 500.0 {
                    risks.push(format!("Small fund (₹{a:.0} Cr): less proven, higher cost to run"));
                }
            }
            if let Some(e) = expense {
                if e <= 0.7 {
                    reasons.push(format!("Low cost: {e:.2}% expense ratio"));
                } else if e >= 1.2 {
                    risks.push(format!("High cost: {e:.2}% expense ratio"));
                }
            }
            if let Some(p) = &port {
                if p.cash > 15.0 {
                    risks.push(format!("{:.0}% in cash: not fully invested", p.cash));
                }
                if p.top10 > 40.0 {
                    risks.push(format!("Concentrated: top 10 holdings are {:.0}% of the fund", p.top10));
                }
            }
            if let Some((name, y)) = &manager {
                if *y < 1.5 {
                    risks.push(format!("Manager {name} has run it only {y:.1} years"));
                }
            }
            if let Some(m) = min_sip.filter(|m| *m <= 500.0) {
                reasons.push(format!("SIP from ₹{m:.0} a month"));
            }

            let (top_holdings, top_sectors) = port.map(|p| (p.holdings, p.sectors)).unwrap_or_default();
            FundPick {
                code: str_at(d, "/scheme_code").unwrap_or_default(),
                id: str_at(d, "/search_id").unwrap_or_default(),
                name: name_of(d),
                fund_house: str_at(d, "/fund_house"),
                score,
                verdict: String::new(),
                factors,
                reasons,
                risks,
                nav: num_at(d, "/nav"),
                aum,
                expense_ratio: expense,
                min_sip,
                min_lumpsum: num_at(d, "/min_investment_amount"),
                ret_1y: stat("return1y"),
                ret_3y: stat("return3y"),
                ret_5y: stat("return5y"),
                cat_ret_3y: stat("cat_return3y"),
                sharpe: stat("sharpe_ratio"),
                std_dev: stat("standard_deviation"),
                alpha: stat("alpha"),
                manager_years: manager.as_ref().map(|m| m.1),
                manager: manager.map(|m| m.0),
                flow_pct: flows[i],
                exit_load: str_at(d, "/exit_load"),
                top_holdings,
                top_sectors,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_actively_managed_funds_are_candidates() {
        assert!(is_active("Quant Small Cap Fund Direct Plan Growth"));
        assert!(is_active("SBI Small Cap Fund Direct Growth"));
        for no in [
            "Nippon India Nifty Smallcap 250 Index Fund Direct Growth",
            "Mirae Asset Nifty Smallcap 250 Momentum Quality 100 ETF FoF Direct Growth",
            "WhiteOak Capital Diversified Equity Small Cap Active FOF Direct Growth",
            "Axis Nifty Smallcap 50 Index Fund Direct Growth",
        ] {
            assert!(!is_active(no), "{no}");
        }
    }

    #[test]
    fn implied_flow_strips_out_market_movement() {
        // AUM up 10% while NAV rose 10%: no new money.
        let flat = [Snapshot { day: 0, aum: 100.0, nav: 10.0 }, Snapshot { day: 30, aum: 110.0, nav: 11.0 }];
        assert!(implied_flow(&flat).unwrap().abs() < 1e-9);
        // AUM up 21% while NAV rose 10%: 10% new money.
        let inflow = [Snapshot { day: 0, aum: 100.0, nav: 10.0 }, Snapshot { day: 30, aum: 121.0, nav: 11.0 }];
        assert!((implied_flow(&inflow).unwrap() - 10.0).abs() < 1e-9);
        // Too short a window says nothing.
        let short = [Snapshot { day: 0, aum: 100.0, nav: 10.0 }, Snapshot { day: 10, aum: 150.0, nav: 10.0 }];
        assert_eq!(implied_flow(&short), None);
    }

    #[test]
    fn bands_favour_mid_sized_cheap_to_enter_funds() {
        assert!(aum_band(200.0) < aum_band(800.0));
        assert_eq!(aum_band(10_000.0), 1.0);
        assert!(aum_band(60_000.0) < aum_band(10_000.0));
        assert_eq!(ticket_band(100.0), 1.0);
        assert!(ticket_band(5_000.0) < ticket_band(500.0));
    }

    #[test]
    fn category_rank_maps_first_to_one_and_last_to_zero() {
        let d: Value = serde_json::from_str(r#"{"return_stats":[{"rank3yr":1,"rank_count3yr":34,"rank1yr":46,"rank_count1yr":46}]}"#).unwrap();
        assert_eq!(rank_score(&d, "3yr"), Some(1.0));
        assert_eq!(rank_score(&d, "1yr"), Some(0.0));
        assert_eq!(rank_score(&d, "5yr"), None);
    }

    /// Live, ignored by default. `cargo test -- --ignored --nocapture live_funds`.
    #[test]
    #[ignore]
    fn live_funds() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let mut h = FlowHistory::new();
        let p = picks(true, now, &mut h).expect("funds");
        println!("{} funds, too new {:?}, gaps {:?}", p.funds.len(), p.too_new, p.gaps);
        for f in &p.funds {
            println!(
                "{:5.1} {:9} {:55} sip {:?} aum {:?} {:?}",
                f.score,
                f.verdict,
                f.name,
                f.min_sip,
                f.aum,
                f.factors.iter().map(|x| x.score.map(|s| s.round())).collect::<Vec<_>>()
            );
        }
        if let Some(f) = p.funds.first() {
            println!("+ {:?}\n- {:?}\n{:?}", f.reasons, f.risks, f.top_sectors);
        }
    }
}
