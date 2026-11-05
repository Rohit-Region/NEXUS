//! The pick journal: every day's suggestions, what happened next, and what
//! that teaches the scores.
//!
//! # Recording
//!
//! Every kind of suggestion is written down the first time it is made on a
//! trading day: Nifty 50 stocks (`stock`); the three mid and small-cap
//! divisions (`steady`, `balanced`, `volatile`; `growth` is the single list
//! they replaced, kept for its history); small-cap funds (`fund`); and
//! news-driven predictions (`news`, verdict "Up" or "Down"). The user's own
//! buys are recorded too (`mine`, verdict = what NEXUS said about that stock
//! that day), so the Report can compare what they did with what NEXUS said.
//! Each row keeps the score, verdict, factor scores and price, with the
//! benchmark's level beside it. Later loads that day change nothing: the record is what was suggested,
//! not a revision of it.
//!
//! # Settling
//!
//! A suggestion's outcome is its return over the next 5, 20 and 60 trading
//! sessions, against a benchmark: the Nifty 50 for a Nifty stock, the Nifty
//! Smallcap 250 for a growth stock, the average of every fund ranked that day
//! for a fund. Stock outcomes come from Yahoo's daily closes, fund outcomes
//! from AMFI's NAV history. Each is filled in once its window has passed and
//! never changes after.
//!
//! # Learning
//!
//! Both stock kinds learn, each from its own record. Once 60 trading days
//! have 20-day outcomes, each factor's skill is measured: on each day, how
//! well its scores ranked the stocks by what they went on to return beyond
//! the benchmark (a Spearman rank correlation, the "information coefficient"),
//! averaged over the days. A factor that ranked well gains weight, one that
//! ranked badly loses it:
//!
//! - multiplier = 1 + 4 x skill, held between 0.5x and 2x its base weight;
//! - scaled by confidence, which grows from half at 60 days to full at 120,
//!   because 20-day windows overlap and 60 days is only about three
//!   independent looks at the market;
//! - renormalised so the weights keep their original total.
//!
//! Funds are tracked but their weights do not learn: fund rankings barely
//! move day to day and play out over years, so a few months of outcomes
//! would be noise presented as insight.
//!
//! The learning runs only on days NEXUS is opened. A missed day is a gap in
//! the record, not an error: outcomes for logged days still settle later from
//! price history.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection};
use serde::Serialize;

use super::stock_board::Row;

/// Trading sessions after a suggestion at which its outcome is read.
pub const HORIZONS: [usize; 3] = [5, 20, 60];
/// The horizon the weights learn from (index into `HORIZONS`).
const LEARN_HORIZON: usize = 1;
/// Days with outcomes before any weight moves.
pub const LEARN_MIN_DAYS: usize = 60;
/// Days with outcomes at which a learned adjustment applies in full.
const FULL_CONFIDENCE_DAYS: f64 = 120.0;
const IC_GAIN: f64 = 4.0;
const MIN_MULT: f64 = 0.5;
const MAX_MULT: f64 = 2.0;
/// A day ranks too few stocks to say anything below this.
const MIN_RANKED: usize = 15;

pub const KIND_STOCK: &str = "stock";
pub const KIND_GROWTH: &str = "growth";
pub const KIND_STEADY: &str = "steady";
pub const KIND_BALANCED: &str = "balanced";
pub const KIND_VOLATILE: &str = "volatile";
pub const KIND_FUND: &str = "fund";
pub const KIND_NEWS: &str = "news";
pub const KIND_MINE: &str = "mine";
/// What NEXUS records as its view of a stock it did not score that day.
pub const NOT_COVERED: &str = "Not covered";

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

    /// Return beyond the benchmark in the direction that was called: a
    /// "Down" prediction that fell further than the index scores positive.
    fn called_excess(&self, h: usize) -> Option<f64> {
        let x = self.excess(h)?;
        Some(if self.verdict == "Down" { -x } else { x })
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
pub fn on_or_before(history: &[(i64, f64)], day: i64) -> Option<f64> {
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

/// Fill in stock outcomes whose windows have passed, from rows' dated
/// closes and the benchmark's. Stocks not in `rows` wait for a later call.
pub fn settle_prices(conn: &Connection, kind: &str, rows: &[Row], bench: Option<&[(i64, f64)]>) -> Result<usize, String> {
    let histories: HashMap<&str, &Row> = rows.iter().map(|r| (r.symbol.as_str(), r)).collect();
    let mut settled = 0;
    for mut e in load(conn, kind, true)? {
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
                if let (Some(n), Some(base)) = (bench.and_then(|b| after(b, e.day, *sessions)), e.bench) {
                    e.bench_ret[h] = pct(n, base);
                    changed = true;
                }
            }
        }
        if changed {
            save_outcome(conn, kind, &e)?;
            settled += 1;
        }
    }
    Ok(settled)
}

/// Symbols of `kind` with outcomes still to settle, as (symbol, earliest
/// day), for fetching their history off the database lock. A symbol only
/// appears once at least one horizon could have passed.
pub fn pending(conn: &Connection, kind: &str, today: i64) -> Result<Vec<(String, i64)>, String> {
    let mut first: HashMap<String, i64> = HashMap::new();
    for e in load(conn, kind, true)? {
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

/// The weights to score `kind` with today, and the evidence behind them.
/// `factors` and `base` are in the same order; the result is too.
pub fn learned_weights(conn: &Connection, kind: &str, factors: &[&str], base: &[f64]) -> Result<(Vec<f64>, Learning), String> {
    let mut by_day: HashMap<i64, Vec<Logged>> = HashMap::new();
    for e in load(conn, kind, false)? {
        if e.excess(LEARN_HORIZON).is_some() {
            by_day.entry(e.day).or_default().push(e);
        }
    }
    by_day.retain(|_, v| v.len() >= MIN_RANKED);
    let days = by_day.len();

    let skills: Vec<Option<f64>> = factors
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
    let mut weights = base.to_vec();
    if active {
        for (w, skill) in weights.iter_mut().zip(&skills) {
            if let Some(s) = skill {
                let m = (1.0 + IC_GAIN * s).clamp(MIN_MULT, MAX_MULT);
                *w *= 1.0 + (m - 1.0) * confidence;
            }
        }
        let total: f64 = weights.iter().sum();
        let base_total: f64 = base.iter().sum();
        if total > 0.0 {
            for w in &mut weights {
                *w *= base_total / total;
            }
        }
    }

    let skills = factors
        .iter()
        .zip(skills)
        .zip(base.iter().zip(&weights))
        .map(|((name, skill), (b, w))| FactorSkill { name: name.to_string(), skill, base_weight: *b, weight: *w })
        .collect();
    Ok((weights, Learning { days, needed: LEARN_MIN_DAYS, active, confidence, factors: skills }))
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

/// One recorded day: what the picks did against what the benchmark did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayReport {
    pub day: i64,
    /// How many were picked (Buy and Accumulate, or a fund's Top picks).
    pub picked: usize,
    /// Mean return of the picks, of those to avoid, and of the benchmark,
    /// one per horizon, `None` until settled.
    pub picks_ret: Vec<Option<f64>>,
    pub avoid_ret: Vec<Option<f64>>,
    pub bench_ret: Vec<Option<f64>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PastPick {
    pub day: i64,
    pub symbol: String,
    pub verdict: String,
    pub score: f64,
    pub price: f64,
    /// Return per horizon, and return beyond the benchmark in the called
    /// direction (see `called_excess`), `None` until settled.
    pub ret: Vec<Option<f64>>,
    pub excess: Vec<Option<f64>>,
}

/// Everything recorded for one kind of suggestion.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KindRecord {
    pub kind: String,
    pub benchmark: String,
    pub days: usize,
    pub first_day: Option<i64>,
    pub groups: Vec<GroupRecord>,
    /// Newest first.
    pub daily: Vec<DayReport>,
    /// The latest positive calls, newest first.
    pub recent: Vec<PastPick>,
    /// `None` for a kind whose weights do not learn.
    pub learning: Option<Learning>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackRecord {
    pub horizons: Vec<usize>,
    pub kinds: Vec<KindRecord>,
}

fn mean(v: impl Iterator<Item = f64>) -> Option<f64> {
    let (s, n) = v.fold((0.0, 0usize), |(s, n), x| (s + x, n + 1));
    (n > 0).then(|| s / n as f64)
}

fn outcome(excesses: impl Iterator<Item = f64>) -> Outcome {
    let v: Vec<f64> = excesses.collect();
    let n = v.len();
    Outcome {
        count: n,
        avg_excess: mean(v.iter().copied()),
        hit_rate: (n > 0).then(|| v.iter().filter(|x| **x > 0.0).count() as f64 / n as f64),
    }
}

/// Which verdicts count as a pick and as an avoid for a kind. For the
/// user's own buys every buy is a pick, and an "avoid" is a buy NEXUS had
/// called Avoid.
fn is_pick(kind: &str, verdict: &str) -> bool {
    match kind {
        KIND_FUND => verdict == "Top pick",
        KIND_NEWS => verdict == "Up",
        KIND_MINE => true,
        _ => verdict == "Buy" || verdict == "Accumulate",
    }
}

fn is_avoid(kind: &str, verdict: &str) -> bool {
    verdict
        == match kind {
            KIND_FUND => "Weak",
            KIND_NEWS => "Down",
            _ => "Avoid",
        }
}

/// The verdict groups a kind's report is split into, in display order.
fn labels(kind: &str) -> &'static [&'static str] {
    match kind {
        KIND_FUND => &["Top pick", "Good", "Average", "Weak"],
        KIND_NEWS => &["Up", "Down"],
        KIND_MINE => &["Buy", "Accumulate", "Hold", "Avoid", NOT_COVERED],
        _ => &["Buy", "Accumulate", "Hold", "Avoid"],
    }
}

/// One kind's record. `learning` is passed in because only the caller knows
/// a kind's factors and base weights.
pub fn kind_record(
    conn: &Connection,
    kind: &str,
    benchmark: &str,
    learning: Option<Learning>,
) -> Result<KindRecord, String> {
    let mut entries = load(conn, kind, false)?;

    // A fund's benchmark is the average of every fund ranked the same day,
    // filled in here rather than stored.
    if kind == KIND_FUND {
        let mut peer: HashMap<(i64, usize), f64> = HashMap::new();
        for h in 0..HORIZONS.len() {
            let mut by_day: HashMap<i64, Vec<f64>> = HashMap::new();
            for e in &entries {
                if let Some(r) = e.ret[h] {
                    by_day.entry(e.day).or_default().push(r);
                }
            }
            for (day, rs) in by_day {
                if let Some(m) = mean(rs.into_iter()) {
                    peer.insert((day, h), m);
                }
            }
        }
        for e in &mut entries {
            for h in 0..HORIZONS.len() {
                e.bench_ret[h] = peer.get(&(e.day, h)).copied();
            }
        }
    }

    let groups = labels(kind)
        .iter()
        .map(|label| GroupRecord {
            label: label.to_string(),
            outcomes: (0..HORIZONS.len())
                .map(|h| outcome(entries.iter().filter(|e| e.verdict == *label).filter_map(|e| e.called_excess(h))))
                .collect(),
        })
        .collect();

    let mut days: Vec<i64> = entries.iter().map(|e| e.day).collect::<HashSet<_>>().into_iter().collect();
    days.sort_unstable_by(|a, b| b.cmp(a));
    let daily = days
        .iter()
        .map(|day| {
            let today: Vec<&Logged> = entries.iter().filter(|e| e.day == *day).collect();
            let avg = |keep: &dyn Fn(&Logged) -> bool, h: usize| {
                mean(today.iter().filter(|e| keep(e)).filter_map(|e| e.ret[h]))
            };
            DayReport {
                day: *day,
                picked: today.iter().filter(|e| is_pick(kind, &e.verdict)).count(),
                picks_ret: (0..HORIZONS.len()).map(|h| avg(&|e| is_pick(kind, &e.verdict), h)).collect(),
                avoid_ret: (0..HORIZONS.len()).map(|h| avg(&|e| is_avoid(kind, &e.verdict), h)).collect(),
                bench_ret: (0..HORIZONS.len())
                    .map(|h| today.iter().find_map(|e| e.bench_ret[h]))
                    .collect(),
            }
        })
        .collect();

    let mut recent: Vec<PastPick> = entries
        .iter()
        .filter(|e| is_pick(kind, &e.verdict))
        .map(|e| PastPick {
            day: e.day,
            symbol: e.symbol.clone(),
            verdict: e.verdict.clone(),
            score: e.score,
            price: e.price,
            ret: e.ret.to_vec(),
            excess: (0..HORIZONS.len()).map(|h| e.called_excess(h)).collect(),
        })
        .collect();
    recent.sort_by(|a, b| b.day.cmp(&a.day).then(b.score.total_cmp(&a.score)));
    recent.truncate(40);

    Ok(KindRecord {
        kind: kind.to_string(),
        benchmark: benchmark.to_string(),
        days: days.len(),
        first_day: days.last().copied(),
        groups,
        daily,
        recent,
        learning,
    })
}

/// NEXUS's own call on a stock on a day, from any stock list that scored it:
/// (verdict, score), or `None` if no list did.
pub fn nexus_view(conn: &Connection, day: i64, symbol: &str) -> Option<(String, f64)> {
    conn.query_row(
        "SELECT verdict, score FROM pick_log
         WHERE day = ?1 AND symbol = ?2 AND kind IN ('stock', 'steady', 'balanced', 'volatile', 'growth')
         ORDER BY score DESC LIMIT 1",
        params![day, symbol],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

/// What past suggestions with scores in one band went on to return.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Band {
    pub lo: f64,
    pub hi: f64,
    pub count: usize,
    pub mean_ret: Option<f64>,
    pub sd_ret: Option<f64>,
}

/// Score bands, matching the verdict thresholds.
const BANDS: [(f64, f64); 4] = [(0.0, 45.0), (45.0, 58.0), (58.0, 68.0), (68.0, 100.01)];

/// For each score band, the settled returns of `kind`'s past suggestions at
/// horizon index `h`. Predictions switch to these once a band has enough.
pub fn calibration(conn: &Connection, kind: &str, h: usize) -> Result<Vec<Band>, String> {
    let entries = load(conn, kind, false)?;
    Ok(BANDS
        .iter()
        .map(|(lo, hi)| {
            let rets: Vec<f64> = entries
                .iter()
                .filter(|e| e.score >= *lo && e.score < *hi)
                .filter_map(|e| e.ret[h])
                .collect();
            let m = mean(rets.iter().copied());
            let sd = m.filter(|_| rets.len() > 1).map(|m| {
                (rets.iter().map(|r| (r - m).powi(2)).sum::<f64>() / (rets.len() - 1) as f64).sqrt()
            });
            Band { lo: *lo, hi: hi.min(100.0), count: rets.len(), mean_ret: m, sd_ret: sd }
        })
        .collect())
}

/// Hit rate of settled news predictions at horizon index `h`, by
/// confidence and by sector, as lines for the next prompt: the radar is told
/// its own record so it can lean on what has worked.
pub fn news_feedback(conn: &Connection, h: usize) -> Result<Vec<String>, String> {
    let entries = load(conn, KIND_NEWS, false)?;
    let mut by: HashMap<String, (usize, usize)> = HashMap::new();
    for e in &entries {
        let Some(x) = e.called_excess(h) else { continue };
        let conf = e.factors.get("confidence").copied().unwrap_or(0.0);
        let level = if conf >= 75.0 { "high" } else if conf >= 50.0 { "medium" } else { "low" };
        let mut keys = vec![format!("{level}-confidence calls")];
        for (k, v) in &e.factors {
            if let Some(sector) = k.strip_prefix("sector:") {
                if *v > 0.0 {
                    keys.push(format!("{sector} calls"));
                }
            }
        }
        for k in keys {
            let s = by.entry(k).or_default();
            s.0 += 1;
            if x > 0.0 {
                s.1 += 1;
            }
        }
    }
    let mut lines: Vec<(usize, String)> = by
        .into_iter()
        .filter(|(_, (n, _))| *n >= 3)
        .map(|(k, (n, hit))| (n, format!("{k}: {hit} of {n} right ({:.0}%)", hit as f64 / n as f64 * 100.0)))
        .collect();
    lines.sort_by_key(|l| std::cmp::Reverse(l.0));
    Ok(lines.into_iter().map(|(_, l)| l).take(12).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::stock_picks::{BASE_WEIGHTS, FACTORS};

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // The journal's own migrations, in order: the table, then its widening.
        for (_, sql) in crate::db::migrations::MIGRATIONS.iter().filter(|(_, sql)| sql.contains("pick_log")) {
            conn.execute_batch(sql).expect("pick_log migration");
        }
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

    fn row(symbol: &str, history: Vec<(i64, f64)>) -> Row {
        Row {
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
            turnover_cr: None,
            spark: vec![],
            history,
        }
    }

    fn base_weights(conn: &Connection) -> (Vec<f64>, Learning) {
        learned_weights(conn, KIND_STOCK, &FACTORS, &BASE_WEIGHTS).unwrap()
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
        // The same symbol under another kind is a separate suggestion.
        assert_eq!(record(&conn, KIND_GROWTH, 10, &[entry("A", 30.0, "Avoid", &[])]).unwrap(), 1);
        let rows = load(&conn, KIND_STOCK, false).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].verdict, "Buy");
    }

    #[test]
    fn outcomes_are_read_sessions_after_the_day_against_the_benchmark() {
        let conn = db();
        record(&conn, KIND_STOCK, 10, &[entry("A", 70.0, "Buy", &[("Momentum", 80.0)])]).unwrap();
        record(&conn, KIND_STOCK, 10, &[entry("B", 20.0, "Avoid", &[("Momentum", 10.0)])]).unwrap();
        // Six sessions after day 10: A gains 1 a session, B loses 1, the benchmark gains 5.
        let a: Vec<(i64, f64)> = (10..=16).map(|d| (d, 100.0 + (d - 10) as f64)).collect();
        let b: Vec<(i64, f64)> = (10..=16).map(|d| (d, 100.0 - (d - 10) as f64)).collect();
        let nifty: Vec<(i64, f64)> = (10..=16).map(|d| (d, 1000.0 + 5.0 * (d - 10) as f64)).collect();
        assert_eq!(settle_prices(&conn, KIND_STOCK, &[row("A", a), row("B", b)], Some(&nifty)).unwrap(), 2);
        let e = &load(&conn, KIND_STOCK, false).unwrap()[0];
        // Fifth session after day 10 is day 15: A +5%, Nifty +2.5%.
        assert_eq!(e.ret[0], Some(5.0));
        assert_eq!(e.bench_ret[0], Some(2.5));
        assert_eq!(e.excess(0), Some(2.5));
        // 20 and 60 sessions have not passed.
        assert_eq!(e.ret[1], None);

        let rec = kind_record(&conn, KIND_STOCK, "Nifty 50", None).unwrap();
        assert_eq!(rec.groups[0].label, "Buy");
        assert_eq!(rec.groups[0].outcomes[0].count, 1);
        assert_eq!(rec.groups[0].outcomes[0].hit_rate, Some(1.0));
        let d = &rec.daily[0];
        assert_eq!((d.day, d.picked), (10, 1));
        assert_eq!(d.picks_ret[0], Some(5.0));
        assert_eq!(d.avoid_ret[0], Some(-5.0));
        assert_eq!(d.bench_ret[0], Some(2.5));
        assert_eq!(rec.recent.len(), 1);
    }

    #[test]
    fn funds_are_measured_against_the_days_peer_average() {
        let conn = db();
        let fund = |code: &str, verdict: &str| NewEntry {
            symbol: code.into(),
            score: 50.0,
            verdict: verdict.into(),
            price: 10.0,
            bench: None,
            factors: vec![],
        };
        record(&conn, KIND_FUND, 10, &[fund("1", "Top pick"), fund("2", "Weak")]).unwrap();
        let mut navs = HashMap::new();
        navs.insert("1".to_string(), (9..=16).map(|d| (d, if d > 10 { 11.0 } else { 10.0 })).collect());
        navs.insert("2".to_string(), (9..=16).map(|d| (d, if d > 10 { 9.0 } else { 10.0 })).collect());
        settle_funds(&conn, &navs).unwrap();
        let rec = kind_record(&conn, KIND_FUND, "average fund", None).unwrap();
        // +10% and -10% around a 0% average.
        assert_eq!(rec.groups[0].outcomes[0].avg_excess, Some(10.0));
        assert_eq!(rec.groups[3].outcomes[0].avg_excess, Some(-10.0));
    }

    #[test]
    fn down_calls_are_graded_by_falling() {
        let conn = db();
        let mut e = entry("A", 80.0, "Down", &[("confidence", 80.0)]);
        e.bench = Some(1000.0);
        record(&conn, KIND_NEWS, 10, &[e]).unwrap();
        let a: Vec<(i64, f64)> = (10..=16).map(|d| (d, 100.0 - (d - 10) as f64)).collect();
        let idx: Vec<(i64, f64)> = (10..=16).map(|d| (d, 1000.0)).collect();
        settle_prices(&conn, KIND_NEWS, &[row("A", a)], Some(&idx)).unwrap();
        let rec = kind_record(&conn, KIND_NEWS, "Nifty 50", None).unwrap();
        // Fell 5% while the index was flat: a right call, +5 in its direction.
        let down = rec.groups.iter().find(|g| g.label == "Down").unwrap();
        assert_eq!(down.outcomes[0].avg_excess, Some(5.0));
        assert_eq!(down.outcomes[0].hit_rate, Some(1.0));
    }

    #[test]
    fn calibration_groups_returns_by_score_band() {
        let conn = db();
        record(&conn, KIND_STEADY, 1, &[entry("A", 70.0, "Buy", &[]), entry("B", 72.0, "Buy", &[]), entry("C", 30.0, "Avoid", &[])]).unwrap();
        for (s, r) in [("A", 4.0), ("B", 6.0), ("C", -3.0)] {
            conn.execute("UPDATE pick_log SET ret20 = ?1 WHERE symbol = ?2", params![r, s]).unwrap();
        }
        let bands = calibration(&conn, KIND_STEADY, 1).unwrap();
        assert_eq!(bands[3].count, 2);
        assert_eq!(bands[3].mean_ret, Some(5.0));
        assert_eq!(bands[0].mean_ret, Some(-3.0));
        assert_eq!(bands[1].count, 0);
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
        let (w, l) = base_weights(&conn);
        assert!(!l.active);
        assert_eq!(w, BASE_WEIGHTS.to_vec());
        // The skill is already visible, just not acted on.
        assert!(l.factors[0].skill.unwrap() > 0.99);
    }

    #[test]
    fn skilled_factors_gain_weight_within_bounds_and_the_total_holds() {
        let conn = db();
        seed_learning(&conn, FULL_CONFIDENCE_DAYS as i64);
        let (w, l) = base_weights(&conn);
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
        // Another kind's record is its own: growth has learned nothing.
        let (_, g) = learned_weights(&conn, KIND_GROWTH, &FACTORS, &BASE_WEIGHTS).unwrap();
        assert_eq!(g.days, 0);
    }
}
