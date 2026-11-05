//! A ULIP policy's switch report: how every fund the policy can switch into
//! has done, how the user's own funds compare, and whether a switch is worth
//! making.
//!
//! # Sources
//!
//! - PNB MetLife: the insurer's own "Track fund NAV" chart, which posts a
//!   fund's name and a date range to `/bin/getFundValue` and gets every daily
//!   NAV in it back. It matches the policy page and carries new funds that
//!   Morningstar does not list yet.
//! - Tata AIA publishes no NAV history, only a monthly factsheet of returns
//!   (see `tata_factsheet`). Its funds are judged on that, priced daily from
//!   Morningstar, and those daily prices are logged so that volatility and
//!   correlation fill in over time.
//!
//! # What the suggestion is
//!
//! A written-out rule over past returns, not a forecast. A held fund is
//! compared with the median of the funds of its kind (equity with equity):
//! under a year of prices is too new to judge; trailing by more than 5 points
//! over a year is worth watching; trailing over 3 years as well is worth a
//! switch. Past returns do not predict future ones, which is why one bad year
//! alone never triggers a switch.

use std::sync::Mutex;

use serde::Serialize;

use super::http::{send, HttpError, Request};
use super::mf_connector::civil_from_days;
use super::portfolio_chat::{PNB_METLIFE_GROUP, TATA_AIA_GROUP};
use super::spend::days_from_civil;
use super::tata_factsheet;

const URL: &str = "https://www.pnbmetlife.com/bin/getFundValue";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Equity,
    Balanced,
    Debt,
}

pub struct Fund {
    /// The key its source knows the fund by: PNB MetLife's chart name, or
    /// the tail of a Tata AIA SFIN ("MUP110").
    pub name: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    /// Fund management charge, % a year, when published. NAVs are already
    /// net of it.
    pub fmc: Option<f64>,
    /// Morningstar India's name for it, where it prices the fund daily.
    pub morningstar: Option<&'static str>,
}

const fn fund(name: &'static str, label: &'static str, kind: Kind, fmc: f64) -> Fund {
    Fund { name, label, kind, fmc: Some(fmc), morningstar: None }
}

const fn tata(name: &'static str, label: &'static str, kind: Kind, morningstar: Option<&'static str>) -> Fund {
    Fund { name, label, kind, fmc: None, morningstar }
}

/// A ULIP policy whose funds the report covers.
pub struct Policy {
    pub group: &'static str,
    pub insurer: &'static str,
    pub funds: &'static [Fund],
    /// What the policy allows for switching, as the report says it.
    pub switching: &'static str,
}

pub const POLICIES: &[Policy] = &[
    Policy {
        group: PNB_METLIFE_GROUP,
        insurer: "PNB MetLife",
        funds: PNB_FUNDS,
        switching: "Switches are free and unlimited in this plan, so waiting for a fund to show a real record costs nothing.",
    },
    Policy {
        group: TATA_AIA_GROUP,
        insurer: "Tata AIA",
        funds: TATA_FUNDS,
        switching: "Check the policy document for how many free switches a year it allows before using one; waiting for a fund to show a real record costs nothing.",
    },
];

pub fn policy(group: &str) -> Option<&'static Policy> {
    POLICIES.iter().find(|p| p.group == group)
}

/// The funds the Tata AIA policy holds or can switch into, keyed by SFIN.
/// Momentum Value 50 Index Fund is held but not yet in the factsheet.
pub const TATA_FUNDS: &[Fund] = &[
    tata("FEP110", "Future Equity Pension Fund", Kind::Equity, Some("Tata AIA Life - Future Equity Pension Fund")),
    tata("IPF110", "Income Pension Fund", Kind::Debt, None),
    tata("NAP110", "Alpha 50 Index Pension Fund", Kind::Equity, Some("Tata AIG Life Alpha 50 Index Pension Fund")),
    tata(
        "MQP110",
        "Multicap Momentum Quality Index Pension Fund",
        Kind::Equity,
        Some("Tata AIG Life Multicap Momentum Quality Index Pension Fund"),
    ),
    tata(
        "TBP110",
        "Life Tax Bonanza Consumption Pension Fund",
        Kind::Equity,
        Some("Tata AIG Life - Tax Bonanza Consumption Pension Fund"),
    ),
    tata("TAP110", "Top 200 Alpha 30 Index Pension Fund", Kind::Equity, Some("Tata AIA Top 200 Alpha 30 Index Pension Fund")),
    tata("SLP110", "Sector Leaders Index Pension Fund", Kind::Equity, Some("Tata AIA Sector Leaders Index Pension Fund")),
    tata(
        "MUP110",
        "Multicap Opportunities Pension Fund",
        Kind::Equity,
        Some("Tata AIG Life-India MULTICAP OPPORTUNITIES PENSION FUND"),
    ),
    tata(
        "ENVALINDXP110",
        "Enhanced Value Index Pension Fund",
        Kind::Equity,
        Some("Tata AIG Life-India ENHANCED VALUE INDEX PENSION FUND"),
    ),
    tata("LARMIDINNP110", "Large Mid Cap Innovation Pension Fund", Kind::Equity, None),
    tata("DIVLEINDXP110", "Dividend Leaders Index Pension Fund", Kind::Equity, None),
    tata("MVF", "Momentum Value 50 Index Fund", Kind::Equity, None),
];

/// The funds the PNB MetLife policy can switch between, with their charges,
/// from the Smart GEM brochure (UIN 117L139V01). Labels are the policy page's.
pub const PNB_FUNDS: &[Fund] = &[
    fund("Protector Fund II", "Protector II", Kind::Debt, 1.00),
    fund("Bond Opportunities Fund", "Bond Opportunities Fund", Kind::Debt, 1.00),
    fund("Liquid Fund", "Liquid Fund", Kind::Debt, 1.00),
    fund("Balancer  Fund II", "Balancer II", Kind::Balanced, 1.15),
    fund("Balanced Opportunities Fund", "Balanced Opportunities Fund", Kind::Balanced, 1.15),
    fund("Multiplier III", "Multiplier III", Kind::Equity, 1.25),
    fund("Premier Multi-cap Fund", "Premier Multi-cap Fund", Kind::Equity, 1.25),
    fund("Mid Cap Fund", "Mid Cap Fund", Kind::Equity, 1.25),
    fund("CREST (Thematic Fund)", "CREST (Thematic Fund)", Kind::Equity, 1.25),
    fund("Flexi Cap Fund", "Flexi Cap", Kind::Equity, 1.25),
    fund("Virtue Fund II", "Virtue II", Kind::Equity, 1.25),
    fund("India Opportunities Fund", "India Opportunities Fund", Kind::Equity, 1.35),
    fund("Sustainable Equity Fund", "Sustainable Equity Fund", Kind::Equity, 1.35),
    fund("Small Cap Fund", "Small Cap Fund", Kind::Equity, 1.25),
    fund("Bharat Manufacturing Fund", "Bharat Manufacturing Fund", Kind::Equity, 1.25),
    fund("Bharat Consumption Fund", "Bharat Consumption Fund", Kind::Equity, 1.25),
    fund("Nifty 500 Momentum 50 Index Fund", "Nifty 500 Momentum 50 Index Fund", Kind::Equity, 1.25),
    fund("Value Fund", "Value Fund", Kind::Equity, 1.25),
    fund("Dividend Leaders Index Fund", "Dividend Leaders Index Fund", Kind::Equity, 1.35),
    fund("Multifactor Index Fund", "Multifactor Index Fund", Kind::Equity, 1.35),
    fund("Enhanced Value Index Fund", "Enhanced Value Index Fund", Kind::Equity, 1.35),
    fund("Momentum Quality Index Fund", "Momentum Quality Index Fund", Kind::Equity, 1.35),
];

/// A policy's fund by its label or source name.
pub fn find(group: &str, name: &str) -> Option<&'static Fund> {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    policy(group)?.funds.iter().find(|f| norm(f.label) == norm(name) || norm(f.name) == norm(name))
}

/// Daily NAVs as (day number, NAV), oldest first.
pub type History = Vec<(i64, f64)>;

/// A fund's NAVs from `from` to `to` (day numbers), as far back as its launch.
pub fn history(name: &str, from: i64, to: i64) -> Result<History, String> {
    let body = [("fundCode", name), ("fromDate", &chart_date(from)), ("toDate", &chart_date(to)), ("viewType", "GRAPH")]
        .iter()
        .map(|(k, v)| format!("{k}={}", encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let request = Request::post(URL, body)
        .with_header("Content-Type", "application/x-www-form-urlencoded")
        .with_header("User-Agent", "Mozilla/5.0 (NEXUS)");
    let resp = send(request).map_err(|e: HttpError| match e {
        HttpError::Unreachable { detail } => format!("Could not reach PNB MetLife: {detail}"),
        other => other.to_string(),
    })?;
    if !resp.ok() {
        return Err(format!("PNB MetLife returned status {}.", resp.status));
    }
    let navs = parse(&resp.body).ok_or_else(|| format!("PNB MetLife's NAV reply for {name} was not readable."))?;
    if navs.is_empty() {
        return Err(format!("PNB MetLife had no NAVs for {name}."));
    }
    Ok(navs)
}

/// The chart's reply is HTML-escaped JSON whose `funds` is itself a JSON
/// string of `[["2026-10-01", 67.2057], ...]`.
fn parse(body: &str) -> Option<History> {
    let json = body.replace("&#34;", "\"").replace("&quot;", "\"").replace("&amp;", "&");
    let outer: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
    let rows: Vec<(String, f64)> = serde_json::from_str(outer.get("funds")?.as_str()?).ok()?;
    let mut navs: History = rows
        .into_iter()
        .filter(|(_, nav)| nav.is_finite() && *nav > 0.0)
        .filter_map(|(date, nav)| Some((day_of(&date)?, nav)))
        .collect();
    navs.sort_by_key(|n| n.0);
    navs.dedup_by_key(|n| n.0);
    Some(navs)
}

/// "2026-10-01" as a day number.
pub fn day_of(iso: &str) -> Option<i64> {
    let mut parts = iso.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok().filter(|m| (1..=12).contains(m))?;
    let d: u32 = parts.next()?.parse().ok().filter(|d| (1..=31).contains(d))?;
    Some(days_from_civil(y, m, d))
}

pub fn iso(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// A day number as the chart's "04-Nov-2026".
fn chart_date(day: i64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (y, m, d) = civil_from_days(day);
    format!("{d:02}-{}-{y:04}", MONTHS[m as usize - 1])
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// How far back the report looks: enough for a 5-year return.
const YEARS_BACK: i64 = 5;
/// NAVs are published once a day, so a report is reused for a few hours.
const CACHE_SECS: i64 = 6 * 60 * 60;

/// Each fund's NAVs or why they could not be fetched; index i is
/// `PNB_FUNDS[i]`.
type Fetched = Vec<Result<History, String>>;

static CACHE: Mutex<Option<(i64, Fetched)>> = Mutex::new(None);

/// Every fund's history, one request each, in parallel; cached for a few
/// hours unless `refresh`.
pub fn all_histories(refresh: bool, now: i64) -> Fetched {
    if !refresh {
        if let Some((at, cached)) = CACHE.lock().ok().and_then(|c| c.clone()) {
            if now - at < CACHE_SECS {
                return cached;
            }
        }
    }
    let today = now.div_euclid(86_400);
    let from = today - YEARS_BACK * 365 - 10;
    let fetched: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = PNB_FUNDS.iter().map(|f| s.spawn(move || history(f.name, from, today))).collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("The fetch stopped unexpectedly.".to_string()))).collect()
    });
    if fetched.iter().any(Result::is_ok) {
        if let Ok(mut c) = CACHE.lock() {
            *c = Some((now, fetched.clone()));
        }
    }
    fetched
}

/// One calendar year's return, %. `partial` when the year is under way or
/// the fund launched in it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct YearReturn {
    pub year: i64,
    pub ret: f64,
    pub partial: bool,
}

/// Every 1-year hold inside the history, ended a week apart: how often it
/// made money and the range. A steadier read of a fund than any one year.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rolling {
    pub holds: usize,
    /// % of holds that ended up.
    pub positive: f64,
    pub median: f64,
    pub worst: f64,
    pub best: f64,
}

/// One fund's record. Returns are %, the 3- and 5-year ones a year.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FundStats {
    pub label: String,
    pub kind: Kind,
    pub fmc: Option<f64>,
    pub nav: f64,
    pub nav_date: String,
    /// The first NAV, when the fund launched inside the years looked at.
    pub launched: Option<String>,
    pub r1m: Option<f64>,
    pub r3m: Option<f64>,
    pub r6m: Option<f64>,
    pub r1y: Option<f64>,
    pub cagr3y: Option<f64>,
    pub cagr5y: Option<f64>,
    /// Annualised volatility of daily returns over the past year, or since
    /// launch for a younger fund with enough prices.
    pub volatility: Option<f64>,
    /// The largest fall from a high over the same span, as a negative %.
    pub worst_fall: Option<f64>,
    pub years: Vec<YearReturn>,
    pub rolling: Option<Rolling>,
    /// How closely its daily moves followed the user's funds over the past
    /// year, -1 to 1; lower diversifies more.
    pub correlation: Option<f64>,
    /// When returns come from a published factsheet rather than daily NAVs:
    /// the date they run to, which can be months before `nav_date`.
    pub returns_as_of: Option<String>,
    pub benchmark: Option<String>,
    pub bench_1y: Option<f64>,
    pub bench_3y: Option<f64>,
}

/// Fewer daily returns than this make a volatility figure noise.
pub const MIN_POINTS: usize = 40;
/// How far before a period's start the nearest price may be: a long weekend
/// and a holiday, not a gap in the record.
const NEAR_DAYS: i64 = 10;

pub fn stats(fund: &Fund, navs: &[(i64, f64)], window_start: i64) -> Option<FundStats> {
    let &(last_day, nav) = navs.last()?;
    let first_day = navs[0].0;
    // Base NAV `days` back, or None when the fund is younger than that or
    // there is no price near enough the day to be that period's return.
    let back = |days: i64| -> Option<f64> {
        let target = last_day - days;
        if first_day > target {
            return None;
        }
        navs.iter().rev().find(|(d, _)| *d <= target).filter(|(d, _)| target - d <= NEAR_DAYS).map(|n| n.1)
    };
    let pct = |days: i64| back(days).map(|b| (nav / b - 1.0) * 100.0);
    let cagr = |days: i64| back(days).map(|b| ((nav / b).powf(365.0 / days as f64) - 1.0) * 100.0);

    let (volatility, worst_fall) = risk(navs);

    Some(FundStats {
        label: fund.label.to_string(),
        kind: fund.kind,
        fmc: fund.fmc,
        nav,
        nav_date: iso(last_day),
        // A week's slack: the window opens on a holiday more often than not.
        launched: (first_day > window_start + 7).then(|| iso(first_day)),
        r1m: pct(30),
        r3m: pct(91),
        r6m: pct(182),
        r1y: pct(365),
        cagr3y: cagr(3 * 365),
        cagr5y: cagr(5 * 365),
        volatility,
        worst_fall,
        years: calendar_years(navs),
        rolling: rolling(navs),
        ..Default::default()
    })
}

/// Volatility and worst fall over the past year of daily prices, when there
/// are enough of them.
pub fn risk(navs: &[(i64, f64)]) -> (Option<f64>, Option<f64>) {
    let Some(&(last_day, _)) = navs.last() else { return (None, None) };
    let year: Vec<f64> = navs.iter().filter(|(d, _)| *d >= last_day - 365).map(|n| n.1).collect();
    let daily: Vec<f64> = year.windows(2).map(|w| (w[1] / w[0]).ln()).collect();
    if daily.len() < MIN_POINTS {
        return (None, None);
    }
    let mean = daily.iter().sum::<f64>() / daily.len() as f64;
    let var = daily.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (daily.len() - 1) as f64;
    let mut peak = year[0];
    let worst = year.iter().fold(0.0f64, |worst, &v| {
        peak = peak.max(v);
        worst.min((v / peak - 1.0) * 100.0)
    });
    (Some(var.sqrt() * 252f64.sqrt() * 100.0), Some(worst))
}

/// The NAV on or before `day`.
fn on_or_before(navs: &[(i64, f64)], day: i64) -> Option<f64> {
    let i = navs.partition_point(|(d, _)| *d <= day);
    (i > 0).then(|| navs[i - 1].1)
}

fn calendar_years(navs: &[(i64, f64)]) -> Vec<YearReturn> {
    let (Some(first), Some(last)) = (navs.first(), navs.last()) else { return Vec::new() };
    let year_of = |day: i64| civil_from_days(day).0;
    (year_of(first.0)..=year_of(last.0))
        .filter_map(|year| {
            let start = days_from_civil(year - 1, 12, 31);
            let end = days_from_civil(year, 12, 31);
            let base = on_or_before(navs, start);
            let partial = base.is_none() || last.0 < end;
            let base = base.or_else(|| navs.iter().find(|(d, _)| *d > start).map(|n| n.1))?;
            let close = on_or_before(navs, end)?;
            Some(YearReturn { year, ret: (close / base - 1.0) * 100.0, partial })
        })
        .collect()
}

/// 1-year holds need this many to say anything about consistency.
const MIN_HOLDS: usize = 12;

fn rolling(navs: &[(i64, f64)]) -> Option<Rolling> {
    let first = navs.first()?.0;
    let mut rets: Vec<f64> = navs
        .iter()
        .step_by(5)
        .filter(|(d, _)| *d - 365 >= first)
        .filter_map(|&(d, nav)| on_or_before(navs, d - 365).map(|b| (nav / b - 1.0) * 100.0))
        .collect();
    if rets.len() < MIN_HOLDS {
        return None;
    }
    let holds = rets.len();
    let positive = rets.iter().filter(|r| **r > 0.0).count() as f64 / holds as f64 * 100.0;
    rets.sort_by(f64::total_cmp);
    Some(Rolling { holds, positive, median: median(rets.clone())?, worst: rets[0], best: rets[holds - 1] })
}

/// Correlation of daily returns on the days both funds priced, over the
/// past year; None with too few shared days.
fn correlation(a: &[(i64, f64)], b: &[(i64, f64)]) -> Option<f64> {
    let last = a.last()?.0.min(b.last()?.0);
    let shared: Vec<(f64, f64)> = a
        .iter()
        .filter(|(d, _)| *d >= last - 365 && *d <= last)
        .filter_map(|&(d, x)| b.binary_search_by_key(&d, |n| n.0).ok().map(|i| (x, b[i].1)))
        .collect();
    let pairs: Vec<(f64, f64)> = shared.windows(2).map(|w| ((w[1].0 / w[0].0).ln(), (w[1].1 / w[0].1).ln())).collect();
    if pairs.len() < MIN_POINTS {
        return None;
    }
    let n = pairs.len() as f64;
    let (mx, my) = (pairs.iter().map(|p| p.0).sum::<f64>() / n, pairs.iter().map(|p| p.1).sum::<f64>() / n);
    let cov: f64 = pairs.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    let vx: f64 = pairs.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let vy: f64 = pairs.iter().map(|p| (p.1 - my).powi(2)).sum();
    (vx > 0.0 && vy > 0.0).then(|| cov / (vx * vy).sqrt())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Keep,
    TooNew,
    Watch,
    Switch,
}

/// One of the user's funds in the policy.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldFund {
    pub label: String,
    pub units: f64,
    pub value: f64,
    /// Share of the policy, %.
    pub weight: f64,
    pub verdict: Verdict,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchReport {
    pub insurer: String,
    /// The latest NAV date across the funds.
    pub as_of: String,
    pub funds: Vec<FundStats>,
    pub held: Vec<HeldFund>,
    pub headline: String,
    pub points: Vec<String>,
    /// Funds with the strongest 3-year records, best first, not held.
    pub alternatives: Vec<String>,
    /// Ideas worth weighing, each from the numbers, not a verdict.
    pub ideas: Vec<Idea>,
    /// The date the held funds are worth judging again.
    pub review_on: Option<String>,
    /// Funds whose NAVs could not be fetched, with why.
    pub failed: Vec<String>,
    /// What suits the policy's own terms and the holder's retirement, when
    /// they are known.
    pub plan: Option<super::policy_plan::PlanAdvice>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Idea {
    pub title: String,
    pub body: String,
    /// The funds it is about, to show on the chart.
    pub funds: Vec<String>,
}

/// Points behind the median of its kind, over a year, that make a fund
/// worth watching; and a year, over three years, that make it worth leaving.
const TRAIL_1Y: f64 = 5.0;
const TRAIL_3Y: f64 = 3.0;

fn median(mut xs: Vec<f64>) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(f64::total_cmp);
    let n = xs.len();
    Some(if n % 2 == 1 { xs[n / 2] } else { (xs[n / 2 - 1] + xs[n / 2]) / 2.0 })
}

/// "5 days" or "8 months", from one date to another.
fn span(from: &str, to: &str) -> String {
    let days = match (day_of(from), day_of(to)) {
        (Some(a), Some(b)) => b - a,
        _ => 0,
    };
    if days < 60 {
        format!("{days} days")
    } else {
        format!("{} months", (days as f64 / 30.44).round())
    }
}

/// Days between a fund's latest NAV and the date its published returns run
/// to: none for a daily history, months for a factsheet.
fn publication_lag(f: &FundStats) -> i64 {
    match (day_of(&f.nav_date), f.returns_as_of.as_deref().and_then(day_of)) {
        (Some(nav), Some(returns)) => (nav - returns).max(0),
        _ => 0,
    }
}

/// A held fund against the median of its kind.
fn judge(f: &FundStats, funds: &[FundStats]) -> (Verdict, String) {
    let peers = funds.iter().filter(|p| p.kind == f.kind);
    let med1y = median(peers.clone().filter_map(|p| p.r1y).collect());
    let med3y = median(peers.filter_map(|p| p.cagr3y).collect());
    let Some(r1y) = f.r1y else {
        let Some(since) = f.launched.as_deref() else {
            return (
                Verdict::TooNew,
                "Not in the insurer's published record yet, so there is nothing to judge it on; look again in six months.".to_string(),
            );
        };
        let review = day_of(since).map(|d| iso(d + 365 + publication_lag(f))).unwrap_or_default();
        return (
            Verdict::TooNew,
            format!("Launched {since}, so {} of prices. Too short to judge a fund on; look again after {review}.", span(since, &f.nav_date)),
        );
    };
    let to = f.returns_as_of.as_deref().map(|d| format!(" (to {d})")).unwrap_or_default();
    let Some(m1) = med1y else {
        return (Verdict::Keep, format!("{r1y:+.1}% over the past year{to}; no other fund of its kind to compare with."));
    };
    let gap = r1y - m1;
    if gap >= -TRAIL_1Y {
        return (Verdict::Keep, format!("{r1y:+.1}% over the past year{to}, against {m1:+.1}% for the median fund of its kind."));
    }
    if let (Some(c3), Some(m3)) = (f.cagr3y, med3y) {
        if c3 - m3 < -TRAIL_3Y {
            return (
                Verdict::Switch,
                format!(
                    "{:.1} points behind the median over the past year and {:.1} points a year behind over three years.",
                    -gap,
                    m3 - c3
                ),
            );
        }
    }
    (
        Verdict::Watch,
        format!("{:.1} points behind the median over the past year. One weak year alone is no reason to switch; look again in six months.", -gap),
    )
}

/// The report for the funds held, as (label, units), from the fetched stats.
pub fn report(policy: &Policy, funds: Vec<FundStats>, holdings: &[(String, f64)], failed: Vec<String>) -> SwitchReport {
    let value_of = |label: &str, units: f64| funds.iter().find(|f| f.label == label).map_or(0.0, |f| f.nav * units);
    let total: f64 = holdings.iter().map(|(l, u)| value_of(l, *u)).sum();
    let held: Vec<HeldFund> = holdings
        .iter()
        .filter_map(|(label, units)| {
            let f = funds.iter().find(|f| &f.label == label)?;
            let (verdict, reason) = judge(f, &funds);
            let value = f.nav * units;
            Some(HeldFund {
                label: label.clone(),
                units: *units,
                value,
                weight: if total > 0.0 { value / total * 100.0 } else { 0.0 },
                verdict,
                reason,
            })
        })
        .collect();

    let mut ranked: Vec<&FundStats> = funds
        .iter()
        .filter(|f| f.kind == Kind::Equity && f.cagr3y.is_some() && !held.iter().any(|h| h.label == f.label))
        .collect();
    ranked.sort_by(|a, b| b.cagr3y.partial_cmp(&a.cagr3y).unwrap_or(std::cmp::Ordering::Equal));
    let alternatives: Vec<String> = ranked.iter().take(3).map(|f| f.label.clone()).collect();

    let names = |v: Verdict| held.iter().filter(|h| h.verdict == v).map(|h| h.label.clone()).collect::<Vec<_>>();
    let (switch, watch) = (names(Verdict::Switch), names(Verdict::Watch));
    let headline = if !switch.is_empty() {
        match alternatives.first() {
            Some(best) => format!("Consider switching {} into {best}.", switch.join(" and ")),
            None => format!("Consider switching out of {}.", switch.join(" and ")),
        }
    } else if held.is_empty() {
        "No funds in the policy to judge yet.".to_string()
    } else if held.iter().all(|h| h.verdict == Verdict::TooNew) {
        "Don't switch yet: your funds are too new to judge.".to_string()
    } else if !watch.is_empty() {
        format!("No switch yet; keep an eye on {}.", watch.join(" and "))
    } else {
        let young = names(Verdict::TooNew);
        match young.len() {
            0 => "No switch needed: your funds are keeping up with funds like them.".to_string(),
            1 => format!("No switch needed. {} is too new to judge yet.", young[0]),
            _ => format!("No switch needed. {} are too new to judge yet.", young.join(" and ")),
        }
    };

    let mut points = vec![policy.switching.to_string()];
    if !held.is_empty() && held.iter().all(|h| funds.iter().any(|f| f.label == h.label && f.kind == Kind::Equity)) {
        points.push(format!(
            "All of the policy is in equity, which suits money that won't be needed for years. As retirement comes within five to seven years, moving a share into {} steadies it.",
            safest_debt(&funds).map_or("a debt fund", |f| f.label.as_str())
        ));
    }
    if let Some(d) = funds.iter().filter_map(|f| f.returns_as_of.as_deref()).max() {
        points.push(format!(
            "{} publishes returns only in its monthly factsheet, so returns here run to {d}; NAVs are the latest. Volatility and how funds move together fill in as daily prices are recorded.",
            policy.insurer
        ));
    }
    let mut by_1y: Vec<&FundStats> = funds.iter().filter(|f| f.kind == Kind::Equity && f.r1y.is_some()).collect();
    by_1y.sort_by(|a, b| b.r1y.partial_cmp(&a.r1y).unwrap_or(std::cmp::Ordering::Equal));
    if let Some(top) = by_1y.first() {
        points.push(format!(
            "The best past year, {} at {:+.1}%, is not a reason to move: a fund that leads one year often trails the next. Three- and five-year records say more.",
            top.label,
            top.r1y.unwrap_or_default()
        ));
    }
    match alternatives.as_slice() {
        [] => {}
        [only] => points.push(format!(
            "If you want new premiums in a fund with a longer record, {only} is the only one in the plan with three years of it."
        )),
        _ => points.push(format!(
            "If you want new premiums in a fund with a longer record, the strongest over three years are {}. Check their volatility and worst fall too; higher returns came with deeper drops.",
            alternatives.join(", ")
        )),
    }

    let as_of = funds.iter().map(|f| f.nav_date.clone()).max().unwrap_or_default();
    let review_on = review_date(&funds, &held, &as_of);
    let ideas = ideas(&funds, &held, review_on.as_deref());
    SwitchReport { insurer: policy.insurer.to_string(), as_of, funds, held, headline, points, alternatives, ideas, review_on, failed, plan: None }
}

/// The debt fund with the best record: 3-year, else 1-year.
pub fn safest_debt(funds: &[FundStats]) -> Option<&FundStats> {
    let key = |f: &FundStats| (f.cagr3y.is_some(), f.cagr3y.or(f.r1y).unwrap_or(f64::NEG_INFINITY));
    funds
        .iter()
        .filter(|f| f.kind == Kind::Debt && (f.cagr3y.is_some() || f.r1y.is_some()))
        .max_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal))
}

/// When the held funds are next worth judging: the day the youngest has a
/// year of prices, else six months on.
fn review_date(funds: &[FundStats], held: &[HeldFund], as_of: &str) -> Option<String> {
    let today = day_of(as_of)?;
    let year_old = held
        .iter()
        .filter(|h| h.verdict == Verdict::TooNew)
        .filter_map(|h| {
            let f = funds.iter().find(|f| f.label == h.label)?;
            Some(day_of(f.launched.as_deref()?)? + 365 + publication_lag(f))
        })
        .max();
    (!held.is_empty()).then(|| iso(year_old.unwrap_or(today + 182)))
}

/// Below this correlation with the held funds a fund genuinely diversifies.
const DIVERSIFIES: f64 = 0.75;
/// About two years of weekly 1-year holds, so a young fund's short lucky run
/// cannot outrank a long record.
const STEADY_HOLDS: usize = 100;

fn ideas(funds: &[FundStats], held: &[HeldFund], review_on: Option<&str>) -> Vec<Idea> {
    let is_held = |f: &&FundStats| held.iter().any(|h| h.label == f.label);
    let equity = |f: &&FundStats| f.kind == Kind::Equity;
    let mut out = Vec::new();

    // A fall shared with the market is the market, not the fund.
    let med3m = median(funds.iter().filter(equity).filter_map(|f| f.r3m).collect());
    for h in held {
        let Some(f) = funds.iter().find(|f| f.label == h.label) else { continue };
        if let (Some(r3m), Some(m)) = (f.r3m, med3m) {
            if r3m < -3.0 {
                let versus = if r3m >= m - 2.0 { "about the same as" } else { "more than" };
                let to = f.returns_as_of.as_deref().map(|d| format!(" to {d}")).unwrap_or_default();
                out.push(Idea {
                    title: format!("{} is down, and so is the market", f.label),
                    body: format!(
                        "{:+.1}% over three months{to}, {versus} the median equity fund in the plan ({m:+.1}%). Switching out after a fall turns a paper loss into a real one; the units you hold recover only if you still hold them.",
                        r3m
                    ),
                    funds: vec![f.label.clone()],
                });
            }
        }
    }

    // Diversifying: a solid record that moves least like what is held.
    let med3y = median(funds.iter().filter(equity).filter_map(|f| f.cagr3y).collect());
    let mut diversifiers: Vec<&FundStats> = funds
        .iter()
        .filter(equity)
        .filter(|f| !is_held(f))
        .filter(|f| f.correlation.is_some() && matches!((f.cagr3y, med3y), (Some(c), Some(m)) if c >= m))
        .collect();
    diversifiers.sort_by(|a, b| a.correlation.partial_cmp(&b.correlation).unwrap_or(std::cmp::Ordering::Equal));
    let equity_corr: Vec<f64> = funds.iter().filter(equity).filter(|f| !is_held(f)).filter_map(|f| f.correlation).collect();
    if let Some(d) = diversifiers.first().filter(|d| d.correlation.is_some_and(|c| c < DIVERSIFIES)) {
        out.push(Idea {
            title: format!("{} would spread your risk", d.label),
            body: format!(
                "Of the funds with an above-median three-year record ({:.1}% a year), it moved least like yours over the past year (correlation {:.2}; 1 is lockstep). New premiums there would add a different source of return rather than more of the same.",
                d.cagr3y.unwrap_or_default(),
                d.correlation.unwrap_or_default()
            ),
            funds: vec![d.label.clone()],
        });
    } else if let (Some(lo), Some(hi)) = (
        equity_corr.iter().copied().reduce(f64::min),
        equity_corr.iter().copied().reduce(f64::max),
    ) {
        let mut calm: Vec<&FundStats> = funds.iter().filter(|f| f.kind != Kind::Equity && f.correlation.is_some()).collect();
        calm.sort_by(|a, b| a.correlation.partial_cmp(&b.correlation).unwrap_or(std::cmp::Ordering::Equal));
        out.push(Idea {
            title: "Switching between equity funds changes less than it looks".to_string(),
            body: format!(
                "Every other equity fund in the plan moved with yours almost in step over the past year (correlation {lo:.2} to {hi:.2}; 1 is lockstep). They differ in how much they rise and fall, not in when. Real spreading of risk here only comes from the balanced and debt funds{}.",
                calm.first().map(|f| format!(", {} most of all ({:.2})", f.label, f.correlation.unwrap_or_default())).unwrap_or_default()
            ),
            funds: calm.first().map(|f| vec![f.label.clone()]).unwrap_or_default(),
        });
    }

    // Consistency: the equity fund whose 1-year holds made money most often.
    let mut steady: Vec<&FundStats> =
        funds.iter().filter(equity).filter(|f| f.rolling.as_ref().is_some_and(|r| r.holds >= STEADY_HOLDS)).collect();
    steady.sort_by(|a, b| {
        let key = |f: &FundStats| f.rolling.as_ref().map_or((0.0, 0.0), |r| (r.positive, r.worst));
        key(b).partial_cmp(&key(a)).unwrap_or(std::cmp::Ordering::Equal)
    });
    if let Some(f) = steady.first() {
        let r = f.rolling.as_ref().expect("filtered");
        out.push(Idea {
            title: format!("{} has been the steadiest", f.label),
            body: format!(
                "Of {} one-year holds over the years looked at, {:.0}% made money; the typical one returned {:+.1}%, the worst {:+.1}%. A good core if you prefer fewer surprises.",
                r.holds, r.positive, r.median, r.worst
            ),
            funds: vec![f.label.clone()],
        });
    }

    // Index funds and their benchmarks: how closely each held one tracks.
    for h in held {
        let Some(f) = funds.iter().find(|f| f.label == h.label) else { continue };
        if let (Some(r), Some(b)) = (f.r1y, f.bench_1y) {
            let gap = r - b;
            let to = f.returns_as_of.as_deref().map(|d| format!(" to {d}")).unwrap_or_default();
            out.push(Idea {
                title: format!("{} {} its benchmark", f.label, if gap >= 0.0 { "beat" } else { "trailed" }),
                body: format!(
                    "{r:+.1}% over the year{to}, against {b:+.1}% for {}. {}",
                    f.benchmark.as_deref().unwrap_or("its index"),
                    if gap >= 0.0 {
                        "Ahead of the index it is measured against, after charges."
                    } else {
                        "Behind the index it is measured against; a gap that persists over years is a reason to look elsewhere, a single year is not."
                    }
                ),
                funds: vec![f.label.clone()],
            });
        }
    }

    // The safe slice, for money needed sooner.
    if let Some(f) = safest_debt(funds) {
        let record = match (f.cagr3y, f.r1y) {
            (Some(c), _) => format!("{c:.1}% a year over three years"),
            (None, Some(r)) => format!("{r:+.1}% over its first year"),
            _ => String::new(),
        };
        let fall = f.worst_fall.map(|w| {
            let held_fall = held
                .iter()
                .filter_map(|h| funds.iter().find(|x| x.label == h.label))
                .filter_map(|x| x.worst_fall.map(|w| (w, &x.label)))
                .reduce(|a, b| if b.0 < a.0 { b } else { a })
                .map(|(w, label)| format!(" against {w:.1}% for {label}"))
                .unwrap_or_default();
            format!(", with a worst fall of {w:.1}% in the past year{held_fall}")
        });
        out.push(Idea {
            title: format!("{} for the money you'll need sooner", f.label),
            body: format!(
                "The best debt record in the plan: {record}{}. Move into it gradually in the last five to seven years before retirement.",
                fall.unwrap_or_default()
            ),
            funds: vec![f.label.clone()],
        });
    }

    // Rebalancing keeps the split chosen.
    if held.len() > 1 {
        let split = held.iter().map(|h| format!("{} {:.0}%", h.label, h.weight)).collect::<Vec<_>>().join(", ");
        out.push(Idea {
            title: "Keep the split you chose".to_string(),
            body: format!(
                "Today it is {split}. Once a year, or when a fund drifts 10 points from the share you chose, a switch brings it back. That sells some of what rose and buys what fell, without guessing which comes next."
            ),
            funds: held.iter().map(|h| h.label.clone()).collect(),
        });
    }

    if let Some(day) = review_on {
        out.push(Idea {
            title: format!("Look again on {day}"),
            body: "By then your funds will have a record worth comparing. Until then, daily moves are noise; this page keeps updating as NAVs are published.".to_string(),
            funds: Vec::new(),
        });
    }
    out
}

/// Each fund's correlation with the held funds, weighted by what is held.
pub fn add_correlations(funds: &mut [FundStats], histories: &[(&str, &History)], held: &[(String, f64)]) {
    let weights: Vec<(&History, f64)> = held
        .iter()
        .filter_map(|(label, units)| {
            let navs = histories.iter().find(|(l, _)| l == label)?.1;
            Some((navs, navs.last()?.1 * units))
        })
        .collect();
    for f in funds.iter_mut() {
        let Some((_, mine)) = histories.iter().find(|(l, _)| *l == f.label) else { continue };
        let (sum, weight) = weights
            .iter()
            .filter(|(navs, _)| !std::ptr::eq(*navs, *mine))
            .filter_map(|(navs, w)| correlation(mine, navs).map(|c| (c * w, *w)))
            .fold((0.0, 0.0), |(s, t), (c, w)| (s + c, t + w));
        f.correlation = (weight > 0.0).then(|| sum / weight);
    }
}

/// The whole report for a policy. `logged` is each fund's daily NAVs as
/// recorded by NEXUS (by label), and `prices` each held fund's latest price
/// and date, for funds no source carries yet.
pub fn build(
    group: &str,
    holdings: &[(String, f64)],
    logged: &[(String, History)],
    prices: &[(String, f64, String)],
    refresh: bool,
    now: i64,
) -> Result<SwitchReport, String> {
    let policy = policy(group).ok_or_else(|| format!("No switch report for {group}."))?;
    if policy.group == TATA_AIA_GROUP {
        return tata_factsheet::build(policy, holdings, logged, prices, refresh, now);
    }
    let histories = all_histories(refresh, now);
    let window_start = now.div_euclid(86_400) - YEARS_BACK * 365 - 10;
    let mut funds = Vec::new();
    let mut failed = Vec::new();
    let mut fetched = Vec::new();
    for (fund, h) in PNB_FUNDS.iter().zip(&histories) {
        match h.as_ref().map(|navs| stats(fund, navs, window_start)) {
            Ok(Some(s)) => {
                funds.push(s);
                fetched.push((fund.label, h.as_ref().expect("ok")));
            }
            Ok(None) => failed.push(format!("{}: no NAVs", fund.label)),
            Err(e) => failed.push(format!("{}: {e}", fund.label)),
        }
    }
    if funds.is_empty() {
        return Err(failed.first().cloned().unwrap_or_else(|| "No fund NAVs could be fetched.".to_string()));
    }
    add_correlations(&mut funds, &fetched, holdings);
    Ok(report(policy, funds, holdings, failed))
}

/// Funds' NAVs by label, as (yyyy-mm-dd, NAV), from the cached fetch: daily
/// for PNB MetLife; the factsheet's month-end NAVs plus the logged daily ones
/// for Tata AIA.
pub fn histories_of(group: &str, labels: &[String], logged: &[(String, History)], now: i64) -> Vec<(String, Vec<(String, f64)>)> {
    let as_iso = |navs: &[(i64, f64)]| navs.iter().map(|&(d, nav)| (iso(d), nav)).collect::<Vec<_>>();
    if group == TATA_AIA_GROUP {
        return labels
            .iter()
            .filter_map(|label| Some((label.clone(), as_iso(&tata_factsheet::history_of(label, logged, now)?))))
            .collect();
    }
    let histories = all_histories(false, now);
    labels
        .iter()
        .filter_map(|label| {
            let i = PNB_FUNDS.iter().position(|f| f.label == label)?;
            let navs = histories.get(i)?.as_ref().ok()?;
            Some((label.clone(), as_iso(navs)))
        })
        .collect()
}

/// A fund's latest NAV and its date, for pricing a holding.
pub fn latest_pnb(name: &str, now: i64) -> Result<(f64, String), String> {
    let today = now.div_euclid(86_400);
    let navs = history(name, today - 14, today)?;
    let &(day, nav) = navs.last().ok_or_else(|| format!("PNB MetLife had no recent NAV for {name}."))?;
    Ok((nav, iso(day)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(iso: &str) -> i64 {
        day_of(iso).unwrap()
    }

    /// NAVs growing by `rate` a year, one a day, from `from` to `to`.
    fn growing(from: &str, to: &str, rate: f64) -> History {
        (d(from)..=d(to)).map(|day| (day, 10.0 * (1.0 + rate).powf((day - d(from)) as f64 / 365.0))).collect()
    }

    fn stat(label: &str, kind: Kind, r1y: Option<f64>, cagr3y: Option<f64>) -> FundStats {
        FundStats {
            label: label.into(),
            kind,
            fmc: Some(1.25),
            nav: 10.0,
            nav_date: "2026-11-03".into(),
            r1y,
            cagr3y,
            ..Default::default()
        }
    }

    #[test]
    fn the_chart_reply_is_read() {
        let body = r#"{&#34;funds&#34;: &#34;[[\&#34;2026-10-30\&#34;,10],[\&#34;2026-11-03\&#34;,10.1096],[\&#34;2026-11-02\&#34;,10.0009]]&#34;}"#;
        assert_eq!(
            parse(body).unwrap(),
            vec![(d("2026-10-30"), 10.0), (d("2026-11-02"), 10.0009), (d("2026-11-03"), 10.1096)]
        );
        assert_eq!(parse("<html>maintenance</html>"), None);
    }

    #[test]
    fn dates_go_out_as_the_chart_expects() {
        assert_eq!(chart_date(d("2026-11-04")), "04-Nov-2026");
        assert_eq!(chart_date(d("2021-01-09")), "09-Jan-2021");
        assert_eq!(iso(d("2024-02-29")), "2024-02-29");
    }

    #[test]
    fn every_fund_name_is_found_by_label_or_chart_name() {
        assert_eq!(find(PNB_METLIFE_GROUP, "Balancer II").unwrap().name, "Balancer  Fund II");
        assert_eq!(find(PNB_METLIFE_GROUP, "balancer fund ii").unwrap().label, "Balancer II");
        assert_eq!(find(PNB_METLIFE_GROUP, "Value Fund").unwrap().kind, Kind::Equity);
        assert!(find(PNB_METLIFE_GROUP, "Pension Value Fund").is_none());
        assert_eq!(find(TATA_AIA_GROUP, "Income Pension Fund").unwrap().kind, Kind::Debt);
        assert!(find(TATA_AIA_GROUP, "Value Fund").is_none(), "a fund belongs to one policy");
    }

    #[test]
    fn returns_are_computed_only_as_far_back_as_the_fund_goes() {
        let navs = growing("2023-11-03", "2026-11-03", 0.10);
        let s = stats(find(PNB_METLIFE_GROUP, "Virtue II").unwrap(), &navs, d("2021-10-25")).unwrap();
        assert!((s.r1y.unwrap() - 10.0).abs() < 0.1, "{:?}", s.r1y);
        assert!((s.cagr3y.unwrap() - 10.0).abs() < 0.1, "{:?}", s.cagr3y);
        assert_eq!(s.cagr5y, None, "three years of prices give no five-year return");
        assert_eq!(s.launched.as_deref(), Some("2023-11-03"));
        assert!(s.volatility.unwrap() < 0.5, "a steady climb barely moves");
        assert!(s.worst_fall.unwrap() > -0.01);
    }

    #[test]
    fn a_fall_from_the_high_is_the_worst_fall() {
        let navs: History = (0..100).map(|i| (d("2026-07-01") + i, if i < 50 { 10.0 + i as f64 * 0.1 } else { 14.9 - (i - 50) as f64 * 0.1 })).collect();
        let s = stats(find(PNB_METLIFE_GROUP, "Value Fund").unwrap(), &navs, d("2021-10-25")).unwrap();
        assert!((s.worst_fall.unwrap() - (10.0 / 14.9 - 1.0) * 100.0).abs() < 1.0, "{:?}", s.worst_fall);
        assert_eq!(s.r1y, None);
    }

    #[test]
    fn calendar_years_mark_the_launch_and_current_year_partial() {
        let navs = growing("2024-07-01", "2026-11-03", 0.10);
        let years = calendar_years(&navs);
        assert_eq!(years.iter().map(|y| (y.year, y.partial)).collect::<Vec<_>>(), vec![(2024, true), (2025, false), (2026, true)]);
        assert!((years[1].ret - 10.0).abs() < 0.1, "{:?}", years[1]);
    }

    #[test]
    fn rolling_holds_need_a_year_and_a_bit() {
        assert_eq!(rolling(&growing("2026-01-01", "2026-11-03", 0.1)), None);
        let r = rolling(&growing("2023-11-03", "2026-11-03", 0.1)).unwrap();
        assert_eq!(r.positive, 100.0);
        assert!((r.median - 10.0).abs() < 0.2, "{r:?}");
    }

    #[test]
    fn correlation_is_one_in_lockstep_and_negative_when_opposed() {
        let a: History = (0..120).map(|i| (d("2026-06-01") + i, 10.0 + ((i * 7) % 5) as f64 * 0.1)).collect();
        let b: History = a.iter().map(|&(d, v)| (d, v * 2.0)).collect();
        let c: History = a.iter().map(|&(d, v)| (d, 30.0 / v)).collect();
        assert!((correlation(&a, &b).unwrap() - 1.0).abs() < 1e-9);
        assert!(correlation(&a, &c).unwrap() < -0.9);
        assert_eq!(correlation(&a[..20], &b[..20]), None, "too few shared days");
    }

    #[test]
    fn a_young_fund_is_too_new_to_judge() {
        let mut f = stat("Momentum Quality Index Fund", Kind::Equity, None, None);
        f.launched = Some("2026-10-30".into());
        let (v, why) = judge(&f, &[f.clone(), stat("Virtue II", Kind::Equity, Some(5.0), None)]);
        assert_eq!(v, Verdict::TooNew);
        assert!(why.contains("2027-10-30"), "{why}");
        assert!(why.contains("4 days of prices"), "{why}");
    }

    #[test]
    fn one_weak_year_is_watched_and_three_weak_years_switched() {
        let peers = vec![
            stat("A", Kind::Equity, Some(10.0), Some(15.0)),
            stat("B", Kind::Equity, Some(12.0), Some(16.0)),
            stat("C", Kind::Equity, Some(14.0), Some(17.0)),
        ];
        let mut funds = peers.clone();
        funds.push(stat("Mine", Kind::Equity, Some(2.0), Some(16.0)));
        assert_eq!(judge(&funds[3], &funds).0, Verdict::Watch, "behind on 1 year, fine on 3");
        funds[3] = stat("Mine", Kind::Equity, Some(2.0), Some(9.0));
        assert_eq!(judge(&funds[3], &funds).0, Verdict::Switch);
        funds[3] = stat("Mine", Kind::Equity, Some(9.0), Some(9.0));
        assert_eq!(judge(&funds[3], &funds).0, Verdict::Keep, "within 5 points over a year is in line");
    }

    #[test]
    fn debt_is_judged_against_debt() {
        let funds = vec![
            stat("Equity star", Kind::Equity, Some(30.0), None),
            stat("Bond", Kind::Debt, Some(7.0), None),
            stat("Liquid", Kind::Debt, Some(6.5), None),
        ];
        assert_eq!(judge(&funds[2], &funds).0, Verdict::Keep);
    }

    #[test]
    fn the_report_names_a_switch_target_and_weights_holdings() {
        let funds = vec![
            stat("A", Kind::Equity, Some(10.0), Some(15.0)),
            stat("B", Kind::Equity, Some(12.0), Some(18.0)),
            stat("C", Kind::Equity, Some(14.0), Some(17.0)),
            stat("Mine", Kind::Equity, Some(1.0), Some(8.0)),
        ];
        let r = report(&POLICIES[0], funds, &[("Mine".into(), 300.0), ("C".into(), 100.0)], vec![]);
        assert_eq!(r.headline, "Consider switching Mine into B.");
        assert_eq!(r.alternatives, vec!["B", "A"], "held funds are not offered as alternatives");
        assert!((r.held[0].weight - 75.0).abs() < 1e-9);
    }

    #[test]
    fn new_funds_only_means_wait() {
        let mut young = stat("Value Fund", Kind::Equity, None, None);
        young.launched = Some("2026-08-01".into());
        let r = report(&POLICIES[0], vec![young, stat("Virtue II", Kind::Equity, Some(5.0), Some(14.0))], &[("Value Fund".into(), 10.0)], vec![]);
        assert_eq!(r.headline, "Don't switch yet: your funds are too new to judge.");
    }

    /// Run with `cargo test -- --ignored` to check every fund name against
    /// PNB MetLife's chart as it is today.
    #[test]
    #[ignore = "network"]
    fn every_fund_has_navs_on_pnb_metlife() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        for (f, h) in PNB_FUNDS.iter().zip(all_histories(true, now)) {
            let h = h.unwrap_or_else(|e| panic!("{}: {e}", f.label));
            println!("{}: {} NAVs, last {:?}", f.label, h.len(), h.last());
        }
        let held = [("Momentum Quality Index Fund".into(), 240.53), ("Value Fund".into(), 222.82)];
        let r = build(PNB_METLIFE_GROUP, &held, &[], &[], false, now).unwrap();
        println!("{}", serde_json::to_string_pretty(&r).unwrap());
    }
}
