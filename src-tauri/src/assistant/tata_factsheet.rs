//! Tata AIA's fund factsheet, the only fund record Tata AIA publishes in a
//! form a program can read.
//!
//! `/content/tataaiamfisip/api/fund-details.<yyyymm>.json` is the monthly
//! factsheet behind its fund pages (it answers with the latest one whatever
//! month is asked for, but fails without one): for every fund, the NAV 1, 3 and 6 months and 1 to
//! 5 years before the month's end, the change since each (the multi-year
//! ones a year, compounded), and the same for its benchmark. There is no
//! daily history, so a fund's chart is those month-end NAVs plus the daily
//! prices NEXUS has recorded since, and its volatility waits on enough of
//! those.

use std::sync::Mutex;

use super::http::{send, HttpError, Request};
use super::mf_connector::civil_from_days;
use super::spend::days_from_civil;
use super::ulip_switch::{self, add_correlations, day_of, iso, risk, Fund, FundStats, History, Policy, SwitchReport};

const URL: &str = "https://www.tataaia.com/content/tataaiamfisip/api/fund-details";

/// One fund's factsheet row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The SFIN without spaces, as "ULIF09631/12/25MUP110".
    pub sfin: String,
    /// Day number of the month's end the figures run to.
    pub as_of: i64,
    pub nav: f64,
    pub launched: Option<i64>,
    pub r1m: Option<f64>,
    pub r3m: Option<f64>,
    pub r6m: Option<f64>,
    pub r1y: Option<f64>,
    pub cagr3y: Option<f64>,
    pub cagr5y: Option<f64>,
    pub benchmark: Option<String>,
    pub bench_1y: Option<f64>,
    pub bench_3y: Option<f64>,
    /// Month-end NAVs the factsheet gives, oldest first.
    pub anchors: History,
}

/// Look-backs as (months, column stem).
const LOOKBACKS: [(u32, &str); 8] = [
    (1, "Last 1 Month"),
    (3, "Last 3 Months"),
    (6, "Last 6 Months"),
    (12, "Last 1 Year"),
    (24, "Last 2 Years"),
    (36, "Last 3 Years"),
    (48, "Last 4 Years"),
    (60, "Last 5 Years"),
];

/// The last day of the month `back` months before (year, month).
fn month_end(year: i64, month: u32, back: u32) -> i64 {
    let total = year * 12 + i64::from(month) - 1 - i64::from(back);
    let (y, m) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    days_from_civil(ny, nm, 1) - 1
}

/// "31/12/25" inside an SFIN, as a day number.
fn sfin_date(sfin: &str) -> Option<i64> {
    let i = sfin.find('/')?;
    let part = sfin.get(i.checked_sub(2)?..i + 6)?;
    let mut it = part.split('/');
    let d: u32 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok().filter(|m| (1..=12).contains(m))?;
    let y: i64 = it.next()?.parse().ok()?;
    Some(days_from_civil(2000 + y, m, d))
}

pub fn parse(body: &str) -> Result<Vec<Row>, String> {
    let bad = || "Tata AIA's factsheet was not in the expected shape; the page may have changed.".to_string();
    let v: serde_json::Value = serde_json::from_str(body).map_err(|_| bad())?;
    let details = v.pointer("/responseJson/fundDetails").ok_or_else(bad)?;
    let columns: Vec<&str> = details.get("columns").and_then(|c| c.as_array()).ok_or_else(bad)?.iter().filter_map(|c| c.as_str()).collect();
    let col = |name: &str| columns.iter().position(|c| *c == name);
    let rows = details.get("data").and_then(|d| d.as_array()).ok_or_else(bad)?;
    let (Some(c_month), Some(c_sfin)) = (col("Month"), col("SFIN No.")) else { return Err(bad()) };

    Ok(rows
        .iter()
        .filter_map(|row| {
            let cells: Vec<&str> = row.as_array()?.iter().map(|c| c.as_str().unwrap_or("")).collect();
            let text = |name: &str| col(name).and_then(|i| cells.get(i)).map(|s| s.trim()).filter(|s| !s.is_empty() && *s != "-");
            let num = |name: &str| text(name).and_then(|s| s.parse::<f64>().ok()).filter(|n| n.is_finite());
            let month = cells.get(c_month)?;
            let (year, mon): (i64, u32) = (month.get(..4)?.parse().ok()?, month.get(4..6)?.parse().ok()?);
            let as_of = month_end(year, mon, 0);
            let sfin = cells.get(c_sfin)?.replace(' ', "");

            // Each look-back gives a NAV and the change since, so the NAV
            // at the month's end is the first one times its change.
            let lookbacks: Vec<(u32, f64, Option<f64>)> = LOOKBACKS
                .iter()
                .filter_map(|(months, stem)| {
                    let nav = num(&format!("{stem} NAV")).filter(|n| *n > 0.0)?;
                    Some((*months, nav, num(&format!("{stem} NAV Change %"))))
                })
                .collect();
            let nav = lookbacks.iter().find_map(|(_, nav, chg)| chg.map(|c| nav * (1.0 + c / 100.0)))?;
            let change = |months: u32| lookbacks.iter().find(|(m, _, _)| *m == months).and_then(|l| l.2);
            let launched = sfin_date(&sfin);

            let mut anchors: History = lookbacks.iter().map(|(m, v, _)| (month_end(year, mon, *m), *v)).collect();
            if let (Some(day), Some(v)) = (launched, num("Since Inception  NAV").filter(|n| *n > 0.0)) {
                anchors.push((day, v));
            }
            anchors.push((as_of, nav));
            anchors.sort_by_key(|a| a.0);
            anchors.dedup_by_key(|a| a.0);

            Some(Row {
                sfin,
                as_of,
                nav,
                launched,
                r1m: change(1),
                r3m: change(3),
                r6m: change(6),
                r1y: change(12),
                cagr3y: change(36),
                cagr5y: change(60),
                benchmark: text("Benchmark Index").map(|b| b.trim_end_matches("- 100%").trim().to_string()),
                bench_1y: num("BenchmarkIndex Last 1 Year NAV Change %"),
                bench_3y: num("BenchmarkIndex Last 3 Years NAV Change %"),
                anchors,
            })
        })
        .collect())
}

fn fetch(now: i64) -> Result<Vec<Row>, String> {
    let (y, m, _) = civil_from_days(now.div_euclid(86_400));
    let request = Request::get(&format!("{URL}.{y:04}{m:02}.json")).with_header("User-Agent", "Mozilla/5.0 (NEXUS)");
    let resp = send(request).map_err(|e: HttpError| match e {
        HttpError::Unreachable { detail } => format!("Could not reach Tata AIA: {detail}"),
        other => other.to_string(),
    })?;
    if !resp.ok() {
        return Err(format!("Tata AIA returned status {}.", resp.status));
    }
    parse(&resp.body)
}

/// The factsheet changes monthly; a day's reuse is plenty.
const CACHE_SECS: i64 = 24 * 60 * 60;
static CACHE: Mutex<Option<(i64, Vec<Row>)>> = Mutex::new(None);

fn rows(refresh: bool, now: i64) -> Result<Vec<Row>, String> {
    if !refresh {
        if let Some((at, cached)) = CACHE.lock().ok().and_then(|c| c.clone()) {
            if now - at < CACHE_SECS {
                return Ok(cached);
            }
        }
    }
    let fresh = fetch(now)?;
    if let Ok(mut c) = CACHE.lock() {
        *c = Some((now, fresh.clone()));
    }
    Ok(fresh)
}

fn row_of<'a>(fund: &Fund, rows: &'a [Row]) -> Option<&'a Row> {
    rows.iter().find(|r| r.sfin.ends_with(fund.name))
}

/// A fund's month-end NAVs from the factsheet, then the daily ones recorded
/// after it.
fn combined(row: Option<&Row>, logged: Option<&History>) -> History {
    let mut navs: History = row.map(|r| r.anchors.clone()).unwrap_or_default();
    let after = navs.last().map_or(i64::MIN, |a| a.0);
    navs.extend(logged.into_iter().flatten().filter(|(d, _)| *d > after));
    navs
}

/// The chart history of one Tata AIA fund, by label.
pub fn history_of(label: &str, logged: &[(String, History)], now: i64) -> Option<History> {
    let fund = ulip_switch::TATA_FUNDS.iter().find(|f| f.label == label)?;
    let rows = CACHE.lock().ok().and_then(|c| c.clone()).map(|c| c.1).or_else(|| self::rows(false, now).ok())?;
    let navs = combined(row_of(fund, &rows), logged.iter().find(|(l, _)| l == label).map(|l| &l.1));
    (!navs.is_empty()).then_some(navs)
}

/// One fund's record from its factsheet row, its recorded daily prices, and
/// failing both, the held holding's own price.
fn stats(fund: &Fund, row: Option<&Row>, logged: Option<&History>, price: Option<(f64, &str)>, window_start: i64) -> Option<FundStats> {
    // The newest of: the last recorded day, the holding's own price, the
    // factsheet's month end.
    let candidates = [
        logged.and_then(|l| l.last()).copied(),
        price.and_then(|(v, d)| Some((day_of(d)?, v))),
        row.map(|r| (r.as_of, r.nav)),
    ];
    let (day, nav) = candidates.into_iter().flatten().max_by_key(|c| c.0)?;
    let nav_date = iso(day);
    let (volatility, worst_fall) = logged.map(|l| risk(l)).unwrap_or((None, None));
    Some(FundStats {
        label: fund.label.to_string(),
        kind: fund.kind,
        fmc: fund.fmc,
        nav,
        nav_date,
        launched: row.and_then(|r| r.launched).filter(|d| *d > window_start).map(iso),
        r1m: row.and_then(|r| r.r1m),
        r3m: row.and_then(|r| r.r3m),
        r6m: row.and_then(|r| r.r6m),
        r1y: row.and_then(|r| r.r1y),
        cagr3y: row.and_then(|r| r.cagr3y),
        cagr5y: row.and_then(|r| r.cagr5y),
        volatility,
        worst_fall,
        returns_as_of: row.map(|r| iso(r.as_of)),
        benchmark: row.and_then(|r| r.benchmark.clone()),
        bench_1y: row.and_then(|r| r.bench_1y),
        bench_3y: row.and_then(|r| r.bench_3y),
        ..Default::default()
    })
}

pub fn build(
    policy: &Policy,
    holdings: &[(String, f64)],
    logged: &[(String, History)],
    prices: &[(String, f64, String)],
    refresh: bool,
    now: i64,
) -> Result<SwitchReport, String> {
    let rows = rows(refresh, now)?;
    let window_start = now.div_euclid(86_400) - 5 * 365 - 10;
    let log_of = |label: &str| logged.iter().find(|(l, _)| l == label).map(|l| &l.1);
    let mut funds = Vec::new();
    let mut failed = Vec::new();
    for fund in policy.funds {
        let price = prices.iter().find(|(l, _, _)| l == fund.label).map(|(_, v, d)| (*v, d.as_str()));
        match stats(fund, row_of(fund, &rows), log_of(fund.label), price, window_start) {
            Some(s) => funds.push(s),
            None => failed.push(format!("{}: not in the factsheet and not yet priced", fund.label)),
        }
    }
    let daily: Vec<(&str, &History)> = logged.iter().map(|(l, h)| (l.as_str(), h)).collect();
    add_correlations(&mut funds, &daily, holdings);
    Ok(ulip_switch::report(policy, funds, holdings, failed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        let columns = [
            "Month",
            "SFIN No.",
            "FundName",
            "Benchmark Index",
            "BenchmarkIndex Last 1 Year NAV Change %",
            "BenchmarkIndex Last 3 Years NAV Change %",
            "Last 1 Month NAV",
            "Last 1 Month NAV Change %",
            "Last 3 Months NAV",
            "Last 3 Months NAV Change %",
            "Last 1 Year NAV",
            "Last 1 Year NAV Change %",
            "Last 3 Years NAV",
            "Last 3 Years NAV Change %",
            "Last 5 Years NAV",
            "Last 5 Years NAV Change %",
            "Since Inception  NAV",
            "Since Inception  NAV Change %",
        ];
        let old = ["202606", "ULIF 020 04/02/08 FEP 110", "FUTURE EQUITY PENSION FUND", "Nifty 50 - 100%", "-6.47", "12.0", "88.0", "2.0", "85.0", "5.6", "89.0", "0.6", "57.0", "16.27", "48.0", "13.49", "10", "12.7"];
        let young = ["202606", "ULIF 096 31/12/25 MUP 110", "MULTICAP OPPORTUNITIES PENSION FUND", "-", "-", "-", "9.5", "3.31", "9.0", "9.05", "-", "-", "-", "-", "-", "-", "10", "-1.86"];
        serde_json::json!({ "responseJson": { "fundDetails": { "columns": columns, "data": [old, young] } } }).to_string()
    }

    #[test]
    fn rows_carry_returns_benchmarks_and_month_end_navs() {
        let rows = parse(&sample()).unwrap();
        let fep = &rows[0];
        assert_eq!(iso(fep.as_of), "2026-06-30");
        assert!((fep.nav - 89.76).abs() < 0.01, "{}", fep.nav);
        assert_eq!((fep.r1y, fep.cagr3y, fep.cagr5y), (Some(0.6), Some(16.27), Some(13.49)));
        assert_eq!(fep.benchmark.as_deref(), Some("Nifty 50"));
        assert_eq!(fep.bench_1y, Some(-6.47));
        assert_eq!(fep.r6m, None, "a column the sheet lacks is no return, not zero");
        let days: Vec<String> = fep.anchors.iter().map(|a| iso(a.0)).collect();
        assert_eq!(days, ["2008-02-04", "2021-06-30", "2023-06-30", "2025-06-30", "2026-03-31", "2026-05-31", "2026-06-30"]);

        let mup = &rows[1];
        assert_eq!(mup.r1y, None, "'-' is no figure");
        assert_eq!(mup.launched.map(iso).as_deref(), Some("2025-12-31"));
    }

    #[test]
    fn month_ends_step_back_across_years() {
        assert_eq!(iso(month_end(2026, 6, 0)), "2026-06-30");
        assert_eq!(iso(month_end(2026, 6, 4)), "2026-02-28");
        assert_eq!(iso(month_end(2026, 3, 25)), "2024-02-29");
        assert_eq!(sfin_date("ULIF10127/05/26DIVLEINDXP110").map(iso).as_deref(), Some("2026-05-27"));
    }

    #[test]
    fn a_fund_is_matched_by_its_sfin_tail() {
        let rows = parse(&sample()).unwrap();
        let mup = ulip_switch::find(super::super::portfolio_chat::TATA_AIA_GROUP, "Multicap Opportunities Pension Fund").unwrap();
        assert_eq!(row_of(mup, &rows).unwrap().sfin, "ULIF09631/12/25MUP110");
    }

    #[test]
    fn recorded_prices_extend_the_factsheet_and_set_the_nav() {
        let rows = parse(&sample()).unwrap();
        let fund = &ulip_switch::TATA_FUNDS[0];
        let log: History = vec![(day_of("2026-06-30").unwrap(), 1.0), (day_of("2026-11-03").unwrap(), 92.5)];
        let navs = combined(Some(&rows[0]), Some(&log));
        assert_eq!(navs.last().map(|n| (iso(n.0), n.1)), Some(("2026-11-03".into(), 92.5)));
        assert!(navs.iter().all(|n| n.1 != 1.0), "a logged day not after the factsheet's is dropped");
        let s = stats(fund, Some(&rows[0]), Some(&log), None, 0).unwrap();
        assert_eq!((s.nav, s.nav_date.as_str(), s.returns_as_of.as_deref()), (92.5, "2026-11-03", Some("2026-06-30")));
    }

    #[test]
    fn a_held_fund_missing_from_the_factsheet_uses_its_own_price() {
        let mvf = ulip_switch::TATA_FUNDS.iter().find(|f| f.name == "MVF").unwrap();
        let s = stats(mvf, None, None, Some((9.9548, "2026-11-03")), 0).unwrap();
        assert_eq!((s.nav, s.r1y, s.launched), (9.9548, None, None));
        assert!(stats(mvf, None, None, None, 0).is_none());
    }

    /// Run with `cargo test -- --ignored` to check every Tata AIA fund against
    /// the live factsheet.
    #[test]
    #[ignore = "network"]
    fn every_tata_fund_but_the_newest_is_in_the_factsheet() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let rows = fetch(now).unwrap();
        for f in ulip_switch::TATA_FUNDS {
            match row_of(f, &rows) {
                Some(r) => println!("{}: as of {}, nav {:.4}, 1y {:?}, 3y {:?}", f.label, iso(r.as_of), r.nav, r.r1y, r.cagr3y),
                None => println!("{}: not in the factsheet", f.label),
            }
        }
        assert!(ulip_switch::TATA_FUNDS.iter().filter(|f| f.name != "MVF").all(|f| row_of(f, &rows).is_some()));
    }

    /// Run with `cargo test -- --ignored` to see the report on the live
    /// factsheet for the policy's holdings.
    #[test]
    #[ignore = "network"]
    fn report_on_the_live_factsheet() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let held = [
            ("Multicap Opportunities Pension Fund".to_string(), 314.8426),
            ("Alpha 50 Index Pension Fund".to_string(), 415.7761),
            ("Momentum Value 50 Index Fund".to_string(), 786.6407),
        ];
        let prices = [
            ("Multicap Opportunities Pension Fund".to_string(), 10.0601, "2026-11-03".to_string()),
            ("Alpha 50 Index Pension Fund".to_string(), 11.0338, "2026-11-03".to_string()),
            ("Momentum Value 50 Index Fund".to_string(), 9.9548, "2026-11-03".to_string()),
        ];
        let group = super::super::portfolio_chat::TATA_AIA_GROUP;
        let r = ulip_switch::build(group, &held, &[], &prices, true, now).unwrap();
        println!("{}", serde_json::to_string_pretty(&r).unwrap());
    }
}
