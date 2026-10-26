//! The pick journal: every day's suggestions, what happened next, and what
//! that teaches the stock score.
//!
//! # Recording
//!
//! The first time the Stocks section scores the Nifty 50 on a trading day,
//! every stock's score, verdict, factor scores and price are written down,
//! with the Nifty's level beside them. Funds are written down the same way
//! when Suits for you loads. Later loads that day change nothing: the record
//! is what was suggested, not a revision of it.
//!
//! # Settling
//!
//! A suggestion's outcome is its return over the next 5, 20 and 60 trading
//! sessions, measured against a benchmark: the Nifty 50 for a stock, the
//! average of every fund ranked that day for a fund. Stock outcomes come from
//! the price board's daily closes, fund outcomes from AMFI's NAV history.
//! Each is filled in once its window has passed and never changes after.
//!
//! # Learning
//!
//! Once 60 trading days have 20-day outcomes, each factor's skill is
//! measured: on each day, how well its scores ranked the stocks by what they
//! went on to return beyond the Nifty (a Spearman rank correlation, the
//! "information coefficient"), averaged over the days. A factor that ranked
//! well gains weight, one that ranked badly loses it:
//!
//! - multiplier = 1 + 4 x skill, held between 0.5x and 2x its base weight;
//! - scaled by confidence, which grows from half at 60 days to full at 120,
//!   because 20-day windows overlap and 60 days is only about three
//!   independent looks at the market;
//! - renormalised so the weights still total 100.
//!
//! Funds are tracked but their weights do not learn: fund rankings barely
//! move day to day and play out over years, so a few months of outcomes
//! would be noise presented as insight.
//!
//! The learning runs only on days NEXUS is opened. A missed day is a gap in
//! the record, not an error: outcomes for logged days still settle later from
//! price history.

use std::collections::HashMap;

use rusqlite::{params, Connection};
use serde::Serialize;

use super::stock_board::{Board, Row};
use super::stock_picks::{BASE_WEIGHTS, FACTORS};

/// Trading sessions after a suggestion at which its outcome is read.
pub const HORIZONS: [usize; 3] = [5, 20, 60];
/// The horizon the stock weights learn from (index into `HORIZONS`).
const LEARN_HORIZON: usize = 1;
/// Days with outcomes before any weight moves.
pub const LEARN_MIN_DAYS: usize = 60;
/// Days with outcomes at which a learned adjustment applies in full.
const FULL_CONFIDENCE_DAYS: f64 = 120.0;
const IC_GAIN: f64 = 4.0;
const MIN_MULT: f64 = 0.5;
const MAX_MULT: f64 = 2.0;
/// A day ranks too few stocks to say anything below this.
const MIN_RANKED: usize = 20;

pub const KIND_STOCK: &str = "stock";
pub const KIND_FUND: &str = "fund";

/// One suggestion to write down.
pub struct NewEntry {
    pub symbol: String,
    pub score: f64,
    pub verdict: String,
    pub price: f64,
    pub bench: Option<f64>,
    pub factors: Vec<(String, Option<f64>)>,
}

/// One suggestion as recorded, with whatever outcomes have settled.
#[derive(Debug, Clone)]
struct Logged {
    day: i64,
    symbol: String,
    score: f64,
    verdict: String,
    price: f64,
    bench: Option<f64>,
    factors: HashMap<String, f64>,
    ret: [Option<f64>; 3],
    bench_ret: [Option<f64>; 3],
}

impl Logged {
    fn excess(&self, h: usize) -> Option<f64> {
        Some(self.ret[h]? - self.bench_ret[h]?)
    }
}

/// Monday to Friday. Day 0 (1 January 1970) was a Thursday.
pub fn is_weekday(day: i64) -> bool {
    !matches!((day + 4).rem_euclid(7), 0 | 6)
}

/// Write down a day's suggestions. A day already recorded is left alone, so
/// the record is the first suggestion of the day, not the last.
pub fn record(conn: &Connection, kind: &str, day: i64, entries: &[NewEntry]) -> Result<usize, String> {
    let mut stmt = conn
        .prepare(
            "INSERT OR IGNORE INTO pick_log (day, kind, symbol, score, verdict, price, bench, factors)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .map_err(|e| format!("Could not record suggestions: {e}"))?;
    let mut added = 0;
    for e in entries {
        let factors: HashMap<&str, f64> =
            e.factors.iter().filter_map(|(n, v)| v.map(|v| (n.as_str(), v))).collect();
        let json = serde_json::to_string(&factors).map_err(|e| e.to_string())?;
        added += stmt
            .execute(params![day, kind, e.symbol, e.score, e.verdict, e.price, e.bench, json])
            .map_err(|e| format!("Could not record suggestions: {e}"))?;
    }
    Ok(added)
}

fn load(conn: &Connection, kind: &str, unsettled_only: bool) -> Result<Vec<Logged>, String> {
    let sql = format!(
        "SELECT day, symbol, score, verdict, price, bench, factors,
                ret5, ret20, ret60, bench5, bench20, bench60
         FROM pick_log WHERE kind = ?1 {} ORDER BY day, symbol",
        if unsettled_only { "AND ret60 IS NULL" } else { "" }
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("Could not read the journal: {e}"))?;
    let rows = stmt
        .query_map([kind], |r| {
            let factors: String = r.get(6)?;
            Ok(Logged {
                day: r.get(0)?,
                symbol: r.get(1)?,
                score: r.get(2)?,
                verdict: r.get(3)?,
                price: r.get(4)?,
                bench: r.get(5)?,
                factors: serde_json::from_str(&factors).unwrap_or_default(),
                ret: [r.get(7)?, r.get(8)?, r.get(9)?],
                bench_ret: [r.get(10)?, r.get(11)?, r.get(12)?],
            })
        })
        .map_err(|e| format!("Could not read the journal: {e}"))?;
    Ok(rows.flatten().collect())
}

/// The close `sessions` trading days after `day`, from a dated series.
fn after(history: &[(i64, f64)], day: i64, sessions: usize) -> Option<f64> {
    history.iter().filter(|(d, _)| *d > day).nth(sessions - 1).map(|(_, c)| *c)
}

/// The last close on or before `day`.
fn on_or_before(history: &[(i64, f64)], day: i64) -> Option<f64> {
    history.iter().rev().find(|(d, _)| *d <= day).map(|(_, c)| *c)
}

fn pct(now: f64, base: f64) -> Option<f64> {
    (base > 0.0).then(|| (now - base) / base * 100.0)
}

fn save_outcome(conn: &Connection, kind: &str, e: &Logged) -> Result<(), String> {
    conn.execute(
        "UPDATE pick_log SET ret5 = ?1, ret20 = ?2, ret60 = ?3, bench5 = ?4, bench20 = ?5, bench60 = ?6
         WHERE day = ?7 AND kind = ?8 AND symbol = ?9",
        params![e.ret[0], e.ret[1], e.ret[2], e.bench_ret[0], e.bench_ret[1], e.bench_ret[2], e.day, kind, e.symbol],
    )
    .map_err(|e| format!("Could not save an outcome: {e}"))?;
    Ok(())
}

/// Fill in stock outcomes whose windows have passed, from the board's closes.
pub fn settle_stocks(conn: &Connection, board: &Board) -> Result<usize, String> {
    let histories: HashMap<&str, &Row> = board.stocks.iter().map(|r| (r.symbol.as_str(), r)).collect();
    let nifty = board.indices.iter().find(|i| i.symbol == "^NSEI").map(|i| i.history.as_slice());
    let mut settled = 0;
    for mut e in load(conn, KIND_STOCK, true)? {
        let Some(row) = histories.get(e.symbol.as_str()) else { continue };
        let mut changed = false;
        for (h, sessions) in HORIZONS.iter().enumerate() {
            if e.ret[h].is_none() {
                if let Some(c) = after(&row.history, e.day, *sessions) {
                    e.ret[h] = pct(c, e.price);
                    changed = true;
                }
            }
            if e.bench_ret[h].is_none() {
                if let (Some(n), Some(base)) = (nifty.and_then(|n| after(n, e.day, *sessions)), e.bench) {
                    e.bench_ret[h] = pct(n, base);
                    changed = true;
                }
            }
        }
        if changed {
            save_outcome(conn, KIND_STOCK, &e)?;
            settled += 1;
        }
    }
    Ok(settled)
}

/// Funds with outcomes still to settle, as (scheme code, earliest day), for
/// fetching their NAV history off the database lock. A fund only appears
/// once at least one horizon could have passed.
pub fn pending_funds(conn: &Connection, today: i64) -> Result<Vec<(String, i64)>, String> {
    let mut first: HashMap<String, i64> = HashMap::new();
    for e in load(conn, KIND_FUND, true)? {
        // Five sessions is at least seven calendar days.
        if today - e.day >= 7 {
            let d = first.entry(e.symbol).or_insert(e.day);
            *d = (*d).min(e.day);
        }
    }
    Ok(first.into_iter().collect())
}

/// Fill in fund outcomes from NAV histories keyed by scheme code. The
/// benchmark is filled at read time (the day's peer average), not here.
pub fn settle_funds(conn: &Connection, navs: &HashMap<String, Vec<(i64, f64)>>) -> Result<usize, String> {
    let mut settled = 0;
    for mut e in load(conn, KIND_FUND, true)? {
        let Some(history) = navs.get(&e.symbol) else { continue };
        // The NAV the fund was bought at is the one published for the day
        // it was suggested, not the possibly day-old one Groww showed.
        let base = on_or_before(history, e.day).unwrap_or(e.price);
        let mut changed = false;
        for (h, sessions) in HORIZONS.iter().enumerate() {
            if e.ret[h].is_none() {
                if let Some(c) = after(history, e.day, *sessions) {
                    e.ret[h] = pct(c, base);
                    changed = true;
                }
            }
        }
        if changed {
            save_outcome(conn, KIND_FUND, &e)?;
            settled += 1;
        }
    }
    Ok(settled)
}

/// Average ranks (1-based, ties shared), for a rank correlation.
fn ranks(values: &[f64]) -> Vec<f64> {
    let mut idx: Vec<usize> = (0..values.len()).collect();
    idx.sort_by(|a, b| values[*a].total_cmp(&values[*b]));
    let mut out = vec![0.0; values.len()];
    let mut i = 0;
    while i < idx.len() {
        let mut j = i;
        while j + 1 < idx.len() && values[idx[j + 1]] == values[idx[i]] {
            j += 1;
        }
        let rank = (i + j) as f64 / 2.0 + 1.0;
        for k in &idx[i..=j] {
            out[*k] = rank;
        }
        i = j + 1;
    }
    out
}

/// Spearman rank correlation, `None` when either side has no spread.
pub fn spearman(xs: &[f64], ys: &[f64]) -> Option<f64> {
    if xs.len() != ys.len() || xs.len() < 3 {
        return None;
    }
    let (rx, ry) = (ranks(xs), ranks(ys));
    let n = rx.len() as f64;
    let (mx, my) = (rx.iter().sum::<f64>() / n, ry.iter().sum::<f64>() / n);
    let cov: f64 = rx.iter().zip(&ry).map(|(a, b)| (a - mx) * (b - my)).sum();
    let vx: f64 = rx.iter().map(|a| (a - mx).powi(2)).sum();
    let vy: f64 = ry.iter().map(|b| (b - my).powi(2)).sum();
    (vx > 0.0 && vy > 0.0).then(|| cov / (vx * vy).sqrt())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FactorSkill {
    pub name: String,
    /// Mean daily rank correlation with 20-session excess return.
    pub skill: Option<f64>,
    pub base_weight: f64,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Learning {
    /// Days with settled 20-session outcomes for enough stocks.
    pub days: usize,
    pub needed: usize,
    pub active: bool,
    /// 0 to 1: how much of the measured adjustment is applied.
    pub confidence: f64,
    pub factors: Vec<FactorSkill>,
}

/// The stock weights to score with today, and the evidence behind them.
pub fn learned_weights(conn: &Connection) -> Result<([f64; 5], Learning), String> {
    let mut by_day: HashMap<i64, Vec<Logged>> = HashMap::new();
    for e in load(conn, KIND_STOCK, false)? {
        if e.excess(LEARN_HORIZON).is_some() {
            by_day.entry(e.day).or_default().push(e);
        }
    }
    by_day.retain(|_, v| v.len() >= MIN_RANKED);
    let days = by_day.len();

    let skills: Vec<Option<f64>> = FACTORS
        .iter()
        .map(|f| {
            let per_day: Vec<f64> = by_day
                .values()
                .filter_map(|entries| {
                    let (xs, ys): (Vec<f64>, Vec<f64>) = entries
                        .iter()
                        .filter_map(|e| Some((*e.factors.get(*f)?, e.excess(LEARN_HORIZON)?)))
                        .unzip();
                    if xs.len() < MIN_RANKED {
                        return None;
                    }
                    spearman(&xs, &ys)
                })
                .collect();
            (!per_day.is_empty()).then(|| per_day.iter().sum::<f64>() / per_day.len() as f64)
        })
        .collect();

    let active = days >= LEARN_MIN_DAYS;
    let confidence = if active { (days as f64 / FULL_CONFIDENCE_DAYS).min(1.0) } else { 0.0 };
    let mut weights = BASE_WEIGHTS;
    if active {
        for (w, skill) in weights.iter_mut().zip(&skills) {
            if let Some(s) = skill {
                let m = (1.0 + IC_GAIN * s).clamp(MIN_MULT, MAX_MULT);
                *w *= 1.0 + (m - 1.0) * confidence;
            }
        }
        let total: f64 = weights.iter().sum();
        let base_total: f64 = BASE_WEIGHTS.iter().sum();
        if total > 0.0 {
            for w in &mut weights {
                *w *= base_total / total;
            }
        }
    }

    let factors = FACTORS
        .iter()
        .zip(skills)
        .zip(BASE_WEIGHTS.iter().zip(weights))
        .map(|((name, skill), (base, weight))| FactorSkill {
            name: name.to_string(),
            skill,
            base_weight: *base,
            weight,
        })
        .collect();
    Ok((weights, Learning { days, needed: LEARN_MIN_DAYS, active, confidence, factors }))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// Suggestions with this horizon settled.
    pub count: usize,
    /// Mean return beyond the benchmark, in percentage points.
    pub avg_excess: Option<f64>,
    /// Share that beat the benchmark, 0 to 1.
    pub hit_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupRecord {
    pub label: String,
    /// One per entry of `HORIZONS`.
    pub outcomes: Vec<Outcome>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PastPick {
    pub day: i64,
    pub symbol: String,
    pub verdict: String,
    pub score: f64,
    /// Return beyond the benchmark per horizon, `None` until settled.
    pub excess: Vec<Option<f64>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackRecord {
    pub horizons: Vec<usize>,
    pub stock_days: usize,
    pub fund_days: usize,
    pub first_day: Option<i64>,
    pub stocks: Vec<GroupRecord>,
    pub funds: Vec<GroupRecord>,
    pub learning: Learning,
    /// The latest Buy and Accumulate calls, newest first.
    pub recent: Vec<PastPick>,
}

fn outcome(excesses: impl Iterator<Item = f64>) -> Outcome {
    let v: Vec<f64> = excesses.collect();
    let n = v.len();
    Outcome {
        count: n,
        avg_excess: (n > 0).then(|| v.iter().sum::<f64>() / n as f64),
        hit_rate: (n > 0).then(|| v.iter().filter(|x| **x > 0.0).count() as f64 / n as f64),
    }
}

fn group(label: &str, entries: &[&Logged], excess: impl Fn(&Logged, usize) -> Option<f64>) -> GroupRecord {
    GroupRecord {
        label: label.to_string(),
        outcomes: (0..HORIZONS.len())
            .map(|h| outcome(entries.iter().filter_map(|e| excess(e, h))))
            .collect(),
    }
}

pub fn track_record(conn: &Connection) -> Result<TrackRecord, String> {
    let stocks = load(conn, KIND_STOCK, false)?;
    let funds = load(conn, KIND_FUND, false)?;
    let distinct = |v: &[Logged]| v.iter().map(|e| e.day).collect::<std::collections::HashSet<_>>().len();

    let stock_groups = ["Buy", "Accumulate", "Hold", "Avoid"]
        .iter()
        .map(|label| {
            let members: Vec<&Logged> = stocks.iter().filter(|e| e.verdict == *label).collect();
            group(label, &members, |e, h| e.excess(h))
        })
        .collect();

    // A fund's benchmark is the average of every fund ranked the same day.
    let mut peer: HashMap<(i64, usize), f64> = HashMap::new();
    for h in 0..HORIZONS.len() {
        let mut sums: HashMap<i64, (f64, usize)> = HashMap::new();
        for e in &funds {
            if let Some(r) = e.ret[h] {
                let s = sums.entry(e.day).or_default();
                s.0 += r;
                s.1 += 1;
            }
        }
        for (day, (sum, n)) in sums {
            peer.insert((day, h), sum / n as f64);
        }
    }
    let fund_excess = |e: &Logged, h: usize| Some(e.ret[h]? - peer.get(&(e.day, h))?);
    let top: Vec<&Logged> = funds.iter().filter(|e| e.verdict == "Top pick").collect();
    let rest: Vec<&Logged> = funds.iter().filter(|e| e.verdict != "Top pick").collect();
    let fund_groups = vec![group("Top picks", &top, fund_excess), group("The rest", &rest, fund_excess)];

    let mut recent: Vec<PastPick> = stocks
        .iter()
        .filter(|e| e.verdict == "Buy" || e.verdict == "Accumulate")
        .map(|e| PastPick {
            day: e.day,
            symbol: e.symbol.clone(),
            verdict: e.verdict.clone(),
            score: e.score,
            excess: (0..HORIZONS.len()).map(|h| e.excess(h)).collect(),
        })
        .collect();
    recent.sort_by(|a, b| b.day.cmp(&a.day).then(b.score.total_cmp(&a.score)));
    recent.truncate(15);

    let (_, learning) = learned_weights(conn)?;
    Ok(TrackRecord {
        horizons: HORIZONS.to_vec(),
        stock_days: distinct(&stocks),
        fund_days: distinct(&funds),
        first_day: stocks.iter().chain(&funds).map(|e| e.day).min(),
        stocks: stock_groups,
        funds: fund_groups,
        learning,
        recent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let (_, sql) = crate::db::migrations::MIGRATIONS
            .iter()
            .find(|(_, sql)| sql.contains("pick_log"))
            .expect("pick_log migration");
        conn.execute_batch(sql).expect("pick_log table");
        conn
    }

    fn entry(symbol: &str, score: f64, verdict: &str, factors: &[(&str, f64)]) -> NewEntry {
        NewEntry {
            symbol: symbol.into(),
            score,
            verdict: verdict.into(),
            price: 100.0,
            bench: Some(1000.0),
            factors: factors.iter().map(|(n, v)| (n.to_string(), Some(*v))).collect(),
        }
    }

    #[test]
    fn weekends_are_not_trading_days() {
        // 26 October 2026 is a Monday.
        let monday = crate::assistant::spend::days_from_civil(2026, 10, 26);
        assert!(is_weekday(monday));
        assert!(is_weekday(monday + 4));
        assert!(!is_weekday(monday + 5));
        assert!(!is_weekday(monday + 6));
    }

    #[test]
    fn the_first_record_of_a_day_is_the_one_kept() {
        let conn = db();
        assert_eq!(record(&conn, KIND_STOCK, 10, &[entry("A", 70.0, "Buy", &[])]).unwrap(), 1);
        assert_eq!(record(&conn, KIND_STOCK, 10, &[entry("A", 30.0, "Avoid", &[])]).unwrap(), 0);
        let rows = load(&conn, KIND_STOCK, false).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].verdict, "Buy");
    }

    #[test]
    fn outcomes_are_read_sessions_after_the_day_against_the_nifty() {
        let conn = db();
        record(&conn, KIND_STOCK, 10, &[entry("A", 70.0, "Buy", &[("Momentum", 80.0)])]).unwrap();
        // Six sessions after day 10: the stock gains 1 a session, the Nifty 5.
        let stock: Vec<(i64, f64)> = (10..=16).map(|d| (d, 100.0 + (d - 10) as f64)).collect();
        let nifty: Vec<(i64, f64)> = (10..=16).map(|d| (d, 1000.0 + 5.0 * (d - 10) as f64)).collect();
        let row = |symbol: &str, history: Vec<(i64, f64)>| Row {
            symbol: symbol.into(),
            name: symbol.into(),
            price: 0.0,
            change_pct: None,
            ret_1w: None,
            ret_1m: None,
            ret_3m: None,
            vs_sma20: None,
            vs_sma50: None,
            from_high: None,
            volume_ratio: None,
            spark: vec![],
            history,
        };
        let board = Board {
            indices: vec![row("^NSEI", nifty)],
            stocks: vec![row("A", stock)],
            skipped: vec![],
            as_of: 0,
            fetched_at: 0,
        };
        assert_eq!(settle_stocks(&conn, &board).unwrap(), 1);
        let e = &load(&conn, KIND_STOCK, false).unwrap()[0];
        // Fifth session after day 10 is day 15: stock +5%, Nifty +2.5%.
        assert_eq!(e.ret[0], Some(5.0));
        assert_eq!(e.bench_ret[0], Some(2.5));
        assert_eq!(e.excess(0), Some(2.5));
        // 20 and 60 sessions have not passed.
        assert_eq!(e.ret[1], None);

        let tr = track_record(&conn).unwrap();
        let buy = &tr.stocks[0];
        assert_eq!(buy.label, "Buy");
        assert_eq!(buy.outcomes[0].count, 1);
        assert_eq!(buy.outcomes[0].hit_rate, Some(1.0));
    }

    /// Live, ignored by default: score today, record it, settle, summarise.
    /// `cargo test -- --ignored --nocapture live_journal`.
    #[test]
    #[ignore]
    fn live_journal() {
        let conn = db();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let (w, _) = learned_weights(&conn).unwrap();
        let (picks, board) = crate::assistant::stock_picks::picks(false, now, &w).expect("picks");
        let bench = board.indices.iter().find(|i| i.symbol == "^NSEI").map(|i| i.price);
        let entries: Vec<NewEntry> = picks
            .picks
            .iter()
            .map(|p| NewEntry {
                symbol: p.symbol.clone(),
                score: p.score,
                verdict: p.verdict.clone(),
                price: p.price,
                bench,
                factors: p.factors.iter().map(|f| (f.name.clone(), f.score)).collect(),
            })
            .collect();
        // Back-date the record 30 sessions so every horizon but 60 can settle.
        let day = crate::assistant::stock_board::day_of(board.as_of, crate::assistant::stock_board::IST_OFFSET) - 45;
        let prices: HashMap<&str, f64> = board
            .stocks
            .iter()
            .filter_map(|r| Some((r.symbol.as_str(), on_or_before(&r.history, day)?)))
            .collect();
        let entries: Vec<NewEntry> = entries
            .into_iter()
            .filter_map(|mut e| {
                e.price = *prices.get(e.symbol.as_str())?;
                Some(e)
            })
            .collect();
        let nifty = board.indices.iter().find(|i| i.symbol == "^NSEI").unwrap();
        let entries: Vec<NewEntry> = entries
            .into_iter()
            .map(|mut e| {
                e.bench = on_or_before(&nifty.history, day);
                e
            })
            .collect();
        println!("recorded {}", record(&conn, KIND_STOCK, day, &entries).unwrap());
        println!("settled {}", settle_stocks(&conn, &board).unwrap());
        let tr = track_record(&conn).unwrap();
        for g in &tr.stocks {
            println!("{:11} {:?}", g.label, g.outcomes.iter().map(|o| (o.count, o.avg_excess.map(|x| (x * 10.0).round() / 10.0))).collect::<Vec<_>>());
        }
        println!("learning {:?}", tr.learning.factors.iter().map(|f| (f.name.clone(), f.skill.map(|s| (s * 100.0).round() / 100.0))).collect::<Vec<_>>());
    }

    #[test]
    fn spearman_reads_rank_agreement() {
        assert!((spearman(&[1.0, 2.0, 3.0, 4.0], &[10.0, 20.0, 30.0, 40.0]).unwrap() - 1.0).abs() < 1e-9);
        assert!((spearman(&[1.0, 2.0, 3.0, 4.0], &[4.0, 3.0, 2.0, 1.0]).unwrap() + 1.0).abs() < 1e-9);
        assert_eq!(spearman(&[1.0, 1.0, 1.0], &[1.0, 2.0, 3.0]), None);
    }

    /// Log `days` days of 30 stocks whose 20-session excess return follows
    /// Momentum exactly and News inversely.
    fn seed_learning(conn: &Connection, days: i64) {
        for day in 0..days {
            let entries: Vec<NewEntry> = (0..30)
                .map(|i| {
                    let s = i as f64;
                    entry(&format!("S{i}"), 50.0, "Hold", &[("Momentum", s), ("News", 30.0 - s), ("Valuation", 50.0)])
                })
                .collect();
            record(conn, KIND_STOCK, day, &entries).unwrap();
            for i in 0..30 {
                conn.execute(
                    "UPDATE pick_log SET ret20 = ?1, bench20 = 0 WHERE day = ?2 AND symbol = ?3",
                    params![i as f64, day, format!("S{i}")],
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn weights_stay_at_base_until_enough_days() {
        let conn = db();
        seed_learning(&conn, (LEARN_MIN_DAYS - 1) as i64);
        let (w, l) = learned_weights(&conn).unwrap();
        assert!(!l.active);
        assert_eq!(w, BASE_WEIGHTS);
        // The skill is already visible, just not acted on.
        assert!(l.factors[0].skill.unwrap() > 0.99);
    }

    #[test]
    fn skilled_factors_gain_weight_within_bounds_and_the_total_holds() {
        let conn = db();
        seed_learning(&conn, FULL_CONFIDENCE_DAYS as i64);
        let (w, l) = learned_weights(&conn).unwrap();
        assert!(l.active);
        assert_eq!(l.confidence, 1.0);
        let base_total: f64 = BASE_WEIGHTS.iter().sum();
        assert!((w.iter().sum::<f64>() - base_total).abs() < 1e-9);
        // Momentum (perfect skill) up, News (perfectly wrong) down.
        assert!(w[0] > BASE_WEIGHTS[0]);
        assert!(w[2] < BASE_WEIGHTS[2]);
        // No factor is switched off or allowed to take over.
        assert!(w.iter().all(|x| *x > 0.0));
        assert!(w[0] <= BASE_WEIGHTS[0] * MAX_MULT);
    }
}
