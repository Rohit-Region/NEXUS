//! Mid and small-cap stocks for a small budget, in three divisions by how
//! much they swing.
//!
//! # Universe and divisions
//!
//! The Nifty Midcap 150 and Nifty Smallcap 250, as NSE publishes them.
//! niftyindices.com turns away automated requests some of the time, so the
//! lists as of October 2026 ship with the app (`data/`) and are used, with a
//! note on screen, whenever the live ones cannot be read.
//!
//! All 400 are priced. Those within the monthly budget (at least one share)
//! with at least ₹2 Cr traded a day are split into thirds by volatility (the
//! annualised standard deviation of daily returns over the last 60
//! sessions), so each division always has candidates:
//!
//! | Division        | Volatility  | Held for          | Weights                                                        |
//! |-----------------|-------------|-------------------|----------------------------------------------------------------|
//! | Steady          | lowest third| ~3 months (60 sessions) | quality 30, growth 25, valuation 20, smart money 15, momentum 10 |
//! | Balanced        | middle third| ~1 month (20)     | growth 30, quality 20, momentum 20, smart money 15, valuation 15 |
//! | High-volatility | top third   | ~1 week (5)       | momentum 35, news 25, smart money 15, growth 15, valuation 10    |
//!
//! Every stock in a division is scored (company data is cached per stock
//! for 12 hours and warmed in the background at startup, see `warm`). These
//! are starting weights: each division learns its own from its own track
//! record (see `pick_journal`).
//!
//! # Prediction
//!
//! Each pick carries an expected return over its division's holding period,
//! a likely range (one standard deviation), the chance it ends higher, a
//! target and a stop-loss. Until a division has 20 settled calls in a score
//! band, the estimate is a model: the stock's own volatility over the period,
//! tilted by its score (a score of 100 expects +0.35 standard deviations, 0
//! expects -0.35). After that it is what past calls in that band actually
//! returned, and the card says which.
//!
//! # Knowing the investor
//!
//! Stocks already held are left out. A sector already over 30% of the
//! investor's stock holdings is marked down, so suggestions spread rather
//! than pile in.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::pick_journal::{Band, HORIZONS, KIND_BALANCED, KIND_STEADY, KIND_VOLATILE};
use super::stock_board::{self, Row};
use super::stock_picks::{
    blend, fundamentals_for, neg, news_for, news_score, percentiles, short_name, Factor, Fundamentals, Headline,
};

/// The benchmark these stocks are measured against.
pub const BENCH_SYMBOL: &str = "NIFTYSMLCAP250.NS";
pub const BENCH_NAME: &str = "Nifty Smallcap 250";

/// One division's rules.
pub struct Spec {
    pub kind: &'static str,
    pub label: &'static str,
    pub blurb: &'static str,
    /// Index into `HORIZONS`: how long a pick is meant to be held.
    pub horizon: usize,
    pub factors: &'static [&'static str],
    pub base: &'static [f64],
    /// A one-week gain above this counts as chasing.
    pub chase_week: f64,
}

pub const DIVISIONS: [Spec; 3] = [
    Spec {
        kind: KIND_STEADY,
        label: "Steady",
        blurb: "The calmest third. Quality companies growing at a fair price, held for months.",
        horizon: 2,
        factors: &["Quality", "Growth", "Valuation", "Smart money", "Momentum"],
        base: &[30.0, 25.0, 20.0, 15.0, 10.0],
        chase_week: 8.0,
    },
    Spec {
        kind: KIND_BALANCED,
        label: "Balanced",
        blurb: "The middle third. Growth with some swing, held for about a month.",
        horizon: 1,
        factors: &["Growth", "Quality", "Momentum", "Smart money", "Valuation"],
        base: &[30.0, 20.0, 20.0, 15.0, 15.0],
        chase_week: 8.0,
    },
    Spec {
        kind: KIND_VOLATILE,
        label: "High-volatility",
        blurb: "The wildest third. Short-term moves driven by momentum and news, held for about a week. Use the stop-loss.",
        horizon: 0,
        factors: &["Momentum", "News", "Smart money", "Growth", "Valuation"],
        base: &[35.0, 25.0, 15.0, 15.0, 10.0],
        chase_week: 15.0,
    },
];

/// (index, live list, bundled copy).
const LISTS: [(&str, &str, &str); 2] = [
    (
        "Midcap 150",
        "https://www.niftyindices.com/IndexConstituent/ind_niftymidcap150list.csv",
        include_str!("data/nifty_midcap150.csv"),
    ),
    (
        "Smallcap 250",
        "https://www.niftyindices.com/IndexConstituent/ind_niftysmallcap250list.csv",
        include_str!("data/nifty_smallcap250.csv"),
    ),
];
const AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124 Safari/537.36";

/// Index membership changes twice a year.
const UNIVERSE_FOR: Duration = Duration::from_secs(7 * 86_400);
const PRICES_FOR: Duration = Duration::from_secs(30 * 60);
const MIN_TURNOVER_CR: f64 = 2.0;
const MIN_MARKET_CAP_CR: f64 = 1_000.0;
/// Sessions of daily returns volatility is measured over.
const VOL_SESSIONS: usize = 60;
/// A sector over this share of the investor's stock holdings is "heavy".
pub const HEAVY_SECTOR: f64 = 0.30;
/// Settled calls a score band needs before predictions use them.
const CALIBRATED_AT: usize = 20;
/// How far a score tilts the model estimate, in standard deviations at 100.
const SCORE_TILT: f64 = 0.35;

/// The lists and whether they are the bundled copies.
static UNIVERSE: Mutex<Option<(Instant, Vec<Member>, bool)>> = Mutex::new(None);
static PRICES: Mutex<Option<(Instant, Prices)>> = Mutex::new(None);

#[derive(Debug, Clone)]
pub struct Member {
    pub symbol: String,
    pub name: String,
    pub industry: String,
    pub index: &'static str,
}

/// Every universe stock priced, plus the benchmark.
#[derive(Debug, Clone)]
pub struct Prices {
    pub rows: Vec<Row>,
    pub bench: Option<Row>,
    pub as_of: i64,
}

/// What a pick is expected to do over its division's holding period.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prediction {
    pub sessions: usize,
    /// Expected return and one-standard-deviation range, in percent.
    pub expected: f64,
    pub low: f64,
    pub high: f64,
    /// Chance of ending higher than today, 0 to 1.
    pub prob_up: f64,
    pub target: f64,
    pub stop: f64,
    /// "model", or how many past calls in this score band it is based on.
    pub basis: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthPick {
    pub symbol: String,
    pub name: String,
    pub industry: String,
    /// "Midcap 150" or "Smallcap 250".
    pub index: String,
    pub price: f64,
    /// Whole shares the budget buys, and what they cost.
    pub shares: u32,
    pub cost: f64,
    pub change_pct: Option<f64>,
    pub ret_1w: Option<f64>,
    pub ret_1m: Option<f64>,
    pub ret_3m: Option<f64>,
    /// Annualised, in percent.
    pub volatility: Option<f64>,
    pub turnover_cr: Option<f64>,
    pub score: f64,
    /// "Buy", "Accumulate", "Hold" or "Avoid".
    pub verdict: String,
    pub factors: Vec<Factor>,
    pub reasons: Vec<String>,
    pub risks: Vec<String>,
    pub prediction: Option<Prediction>,
    pub headlines: Vec<Headline>,
    pub buy_near: Option<f64>,
    pub support: Option<f64>,
    pub fundamentals: Fundamentals,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Division {
    pub kind: String,
    pub label: String,
    pub blurb: String,
    pub sessions: usize,
    /// The volatility band this division covers, annualised percent.
    pub vol_low: f64,
    pub vol_high: f64,
    /// Best score first.
    pub picks: Vec<GrowthPick>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthPicks {
    pub divisions: Vec<Division>,
    pub budget: f64,
    pub universe: usize,
    pub affordable: usize,
    /// Stocks left out because they are already held.
    pub owned_skipped: Vec<String>,
    /// Sectors already heavy in the investor's holdings.
    pub heavy_sectors: Vec<String>,
    pub bench_3m: Option<f64>,
    pub gaps: Vec<String>,
    pub fetched_at: i64,
}

/// What the investor already holds, for leaving out and spreading.
#[derive(Debug, Clone, Default)]
pub struct Investor {
    pub owned: HashSet<String>,
    pub heavy_sectors: HashSet<String>,
}

/// Per-division weights and calibration, keyed by division kind.
#[derive(Debug, Clone, Default)]
pub struct Learned {
    pub weights: HashMap<String, Vec<f64>>,
    pub bands: HashMap<String, Vec<Band>>,
}

/// Split one CSV line, honouring double quotes.
fn csv_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|f| f.trim().to_string()).collect()
}

/// Members from one NSE constituents file:
/// `Company Name,Industry,Symbol,Series,ISIN Code`.
fn parse_list(body: &str, index: &'static str) -> Vec<Member> {
    body.lines()
        .skip(1)
        .filter_map(|line| {
            let f = csv_fields(line);
            let symbol = f.get(2)?.clone();
            (!symbol.is_empty()).then(|| Member {
                symbol,
                name: f.first().cloned().unwrap_or_default(),
                industry: f.get(1).cloned().unwrap_or_default(),
                index,
            })
        })
        .collect()
}

/// The live list, or `None` when the site answers with anything else (it
/// serves an HTML error page with status 200 when it refuses).
fn live_list(url: &str, index: &'static str) -> Option<Vec<Member>> {
    let resp = super::http::send(super::http::Request::get(url).with_header("User-Agent", AGENT)).ok()?;
    let members = parse_list(&resp.body, index);
    (resp.ok() && members.len() >= 50).then_some(members)
}

/// The universe and whether any of it came from the bundled copies.
fn universe() -> (Vec<Member>, bool) {
    if let Some((at, u, bundled)) = UNIVERSE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        // A bundled list is retried sooner, in case the site lets us in.
        let keep = if *bundled { PRICES_FOR } else { UNIVERSE_FOR };
        if at.elapsed() < keep {
            return (u.clone(), *bundled);
        }
    }
    let mut members = Vec::new();
    let mut bundled = false;
    for (index, url, copy) in LISTS {
        match live_list(url, index) {
            Some(m) => members.extend(m),
            None => {
                log::warn!("growth_picks: live {index} list unavailable, using the bundled copy");
                bundled = true;
                members.extend(parse_list(copy, index));
            }
        }
    }
    *UNIVERSE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), members.clone(), bundled));
    (members, bundled)
}

/// Every universe stock priced, from cache when fresh.
pub fn prices(refresh: bool) -> Result<Prices, String> {
    if !refresh {
        if let Some((at, p)) = PRICES.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if at.elapsed() < PRICES_FOR {
                return Ok(p.clone());
            }
        }
    }
    let (members, _) = universe();
    let symbols: Vec<String> = std::iter::once(BENCH_SYMBOL.to_string())
        .chain(members.iter().map(|m| format!("{}.NS", m.symbol)))
        .collect();
    let mut results = stock_board::fetch_rows(&symbols).into_iter();
    let bench = results.next().and_then(|r| r.ok()).map(|(r, _)| r);
    let mut rows = Vec::new();
    let mut as_of = 0;
    for r in results.flatten() {
        as_of = as_of.max(r.1);
        rows.push(r.0);
    }
    if rows.is_empty() {
        return Err("Could not read any mid or small-cap prices. Is the network up?".to_string());
    }
    let p = Prices { rows, bench, as_of };
    *PRICES.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), p.clone()));
    Ok(p)
}

/// The universe with names and industries, for the news radar.
pub fn members() -> Vec<Member> {
    universe().0
}

/// Fetch prices and company data for the whole universe, so the first visit
/// to Suits for you does not wait on 400 lookups. Run once off the main
/// thread at startup; a failure only means the first visit fetches instead.
pub fn warm() {
    let started = Instant::now();
    match prices(false) {
        Ok(p) => {
            let symbols: Vec<String> = p.rows.iter().map(|r| r.symbol.clone()).collect();
            let got = fundamentals_for(&symbols, false).len();
            log::info!("growth_picks: warmed {} prices, {got} company records in {:?}", p.rows.len(), started.elapsed());
        }
        Err(e) => log::warn!("growth_picks: warm-up failed: {e}"),
    }
}

/// Annualised volatility of daily returns over the last `VOL_SESSIONS`, in
/// percent, and the daily figure it came from.
pub fn volatility(row: &Row) -> Option<(f64, f64)> {
    let closes: Vec<f64> = row.history.iter().map(|(_, c)| *c).collect();
    let n = closes.len();
    if n < 21 {
        return None;
    }
    let start = n.saturating_sub(VOL_SESSIONS + 1);
    let rets: Vec<f64> = closes[start..]
        .windows(2)
        .filter(|w| w[0] > 0.0 && w[1] > 0.0)
        .map(|w| (w[1] / w[0]).ln())
        .collect();
    if rets.len() < 20 {
        return None;
    }
    let m = rets.iter().sum::<f64>() / rets.len() as f64;
    let var = rets.iter().map(|r| (r - m).powi(2)).sum::<f64>() / (rets.len() - 1) as f64;
    let daily = var.sqrt();
    Some((daily * 252f64.sqrt() * 100.0, daily))
}

/// The standard normal CDF (Abramowitz and Stegun 7.1.26, error < 1.5e-7).
pub fn normal_cdf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs() / std::f64::consts::SQRT_2);
    let poly = t * (0.254_829_592 + t * (-0.284_496_736 + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let erf = 1.0 - poly * (-(x * x) / 2.0).exp();
    if x >= 0.0 {
        0.5 * (1.0 + erf)
    } else {
        0.5 * (1.0 - erf)
    }
}

/// Expected move over `sessions` for a stock with daily volatility `daily`
/// and this score, from calibration bands when one has enough history.
pub fn predict(price: f64, score: f64, daily: f64, sessions: usize, bands: &[Band]) -> Prediction {
    let band = bands
        .iter()
        .find(|b| score >= b.lo && (score < b.hi || b.hi >= 100.0))
        .filter(|b| b.count >= CALIBRATED_AT);
    let model_sd = daily * (sessions as f64).sqrt() * 100.0;
    let (expected, sd, basis) = match band.and_then(|b| Some((b.mean_ret?, b.sd_ret.unwrap_or(model_sd), b.count))) {
        Some((m, sd, n)) => (m, sd.max(0.1), format!("{n} past calls scored {:.0} to {:.0}", band.unwrap().lo, band.unwrap().hi)),
        None => ((score - 50.0) / 50.0 * SCORE_TILT * model_sd, model_sd.max(0.1), "model".to_string()),
    };
    Prediction {
        sessions,
        expected,
        low: expected - sd,
        high: expected + sd,
        prob_up: normal_cdf(expected / sd),
        target: price * (1.0 + (expected + 0.6 * sd) / 100.0),
        stop: price * (1.0 + (expected - 0.8 * sd) / 100.0),
        basis,
    }
}

/// Rank the affordable mid and small caps for `budget`, in divisions. Also
/// returns the prices, for the pick journal.
pub fn picks(
    budget: f64,
    refresh: bool,
    now: i64,
    learned: &Learned,
    investor: &Investor,
) -> Result<(GrowthPicks, Prices), String> {
    let (members, bundled) = universe();
    let meta: HashMap<&str, &Member> = members.iter().map(|m| (m.symbol.as_str(), m)).collect();
    let prices = prices(refresh)?;
    let bench_3m = prices.bench.as_ref().and_then(|b| b.ret_3m);

    let affordable: Vec<&Row> = prices
        .rows
        .iter()
        .filter(|r| r.price > 0.0 && r.price <= budget)
        .filter(|r| r.turnover_cr.is_some_and(|t| t >= MIN_TURNOVER_CR))
        .collect();
    let mut owned_skipped: Vec<String> =
        affordable.iter().filter(|r| investor.owned.contains(&r.symbol)).map(|r| r.symbol.clone()).collect();
    owned_skipped.sort();
    let mut candidates: Vec<(&Row, f64, f64)> = affordable
        .iter()
        .filter(|r| !investor.owned.contains(&r.symbol))
        .filter_map(|r| volatility(r).map(|(annual, daily)| (*r, annual, daily)))
        .collect();
    candidates.sort_by(|a, b| a.1.total_cmp(&b.1));

    let symbols: Vec<String> = candidates.iter().map(|(r, _, _)| r.symbol.clone()).collect();
    let fundamentals = fundamentals_for(&symbols, refresh);
    let mut gaps = Vec::new();
    if bundled {
        gaps.push("NSE's index lists could not be read, so the October 2026 lists were used".to_string());
    }
    let missing = symbols.iter().filter(|s| !fundamentals.contains_key(*s)).count();
    if missing > 0 {
        gaps.push(format!("company data unavailable for {missing} stocks"));
    }

    // Thirds by volatility, lowest first.
    let n = candidates.len();
    let cuts = [0, n / 3, 2 * n / 3, n];
    let mut divisions = Vec::new();
    for (d, spec) in DIVISIONS.iter().enumerate() {
        let members: Vec<(&Row, f64, f64)> = candidates[cuts[d]..cuts[d + 1]].to_vec();
        let news = if spec.factors.contains(&"News") {
            let names: Vec<String> = members
                .iter()
                .map(|(r, _, _)| fundamentals.get(&r.symbol).and_then(|f| f.display_name.clone()).unwrap_or_else(|| short_name(&r.name)))
                .collect();
            news_for(&names, now).into_iter().map(|r| r.ok()).collect()
        } else {
            vec![None; members.len()]
        };
        let weights = learned.weights.get(spec.kind).cloned().unwrap_or_else(|| spec.base.to_vec());
        let bands = learned.bands.get(spec.kind).cloned().unwrap_or_default();
        let ctx = Context { fundamentals: &fundamentals, meta: &meta, bench_3m, budget, now, investor, bands: &bands };
        let mut picks = score_division(spec, &weights, &members, &news, &ctx);
        picks.sort_by(|a, b| b.score.total_cmp(&a.score));
        divisions.push(Division {
            kind: spec.kind.to_string(),
            label: spec.label.to_string(),
            blurb: spec.blurb.to_string(),
            sessions: HORIZONS[spec.horizon],
            vol_low: members.first().map(|m| m.1).unwrap_or(0.0),
            vol_high: members.last().map(|m| m.1).unwrap_or(0.0),
            picks,
        });
    }

    let mut heavy_sectors: Vec<String> = investor.heavy_sectors.iter().cloned().collect();
    heavy_sectors.sort();
    let result = GrowthPicks {
        divisions,
        budget,
        universe: prices.rows.len(),
        affordable: affordable.len(),
        owned_skipped,
        heavy_sectors,
        bench_3m,
        gaps,
        fetched_at: now,
    };
    Ok((result, prices))
}

/// PEG (P/E over profit growth) as 0..1: 0.75 or under scores 1, 2.5 or
/// over scores 0. `None` without a positive P/E and growth.
pub fn peg_score(pe: Option<f64>, growth: Option<f64>) -> Option<f64> {
    let (pe, g) = (pe?, growth?);
    if pe <= 0.0 || g <= 0.0 {
        return None;
    }
    let peg = pe / g;
    Some(((2.5 - peg) / (2.5 - 0.75)).clamp(0.0, 1.0))
}

/// What every division's scoring reads besides its own stocks.
struct Context<'a> {
    fundamentals: &'a HashMap<String, Fundamentals>,
    meta: &'a HashMap<&'a str, &'a Member>,
    bench_3m: Option<f64>,
    budget: f64,
    now: i64,
    investor: &'a Investor,
    bands: &'a [Band],
}

fn score_division(
    spec: &Spec,
    weights: &[f64],
    rows: &[(&Row, f64, f64)],
    news: &[Option<Vec<Headline>>],
    ctx: &Context,
) -> Vec<GrowthPick> {
    let f = |r: &Row| ctx.fundamentals.get(&r.symbol).cloned().unwrap_or_default();
    let col = |get: &dyn Fn(&Row) -> Option<f64>| percentiles(&rows.iter().map(|(r, _, _)| get(r)).collect::<Vec<_>>());

    // Ranked within the division: a steady stock is compared with steady ones.
    let revenue = col(&|r| f(r).revenue_growth);
    let profit = col(&|r| f(r).profit_growth);
    let profit_q = col(&|r| f(r).profit_q_yoy);
    let roe = col(&|r| f(r).roe);
    let low_debt = col(&|r| neg(f(r).debt_equity));
    let rs3 = col(&|r| r.ret_3m.map(|x| x - ctx.bench_3m.unwrap_or(0.0)));
    let r1m = col(&|r| r.ret_1m);
    let r1w = col(&|r| r.ret_1w);
    let inst = col(&|r| f(r).inst_change);
    let short_term = spec.horizon == 0;

    rows.iter()
        .enumerate()
        .map(|(i, (r, annual_vol, daily_vol))| {
            let fd = f(r);
            let loss_making = fd.pe.is_some_and(|pe| pe <= 0.0) || fd.profit_growth.is_some_and(|g| g < -50.0);
            let industry = ctx.meta.get(r.symbol.as_str()).map(|m| m.industry.clone()).unwrap_or_default();

            let trend = match (r.vs_sma20, r.vs_sma50) {
                (Some(a), Some(b)) if a > 0.0 && b > 0.0 && a < b => Some(1.0),
                (Some(a), Some(b)) if a < 0.0 && b < 0.0 && a > b => Some(0.0),
                (_, Some(b)) if b > 0.0 => Some(0.6),
                (_, Some(_)) => Some(0.3),
                _ => None,
            };
            let volume = r.volume_ratio.map(|v| ((v - 0.8) / 1.5).clamp(0.0, 1.0));
            let mut values: HashMap<&str, Option<f64>> = HashMap::new();
            values.insert("Growth", blend(&[(revenue[i], 0.35), (profit[i], 0.35), (profit_q[i], 0.3)]));
            values.insert(
                "Quality",
                if loss_making { Some(0.0) } else { blend(&[(roe[i], 0.6), (low_debt[i], 0.4)]) },
            );
            // Short term reads the last week and volume; longer, the quarter.
            values.insert(
                "Momentum",
                if short_term {
                    blend(&[(r1w[i], 0.4), (r1m[i], 0.2), (trend, 0.2), (volume, 0.2)])
                } else {
                    blend(&[(rs3[i], 0.5), (r1m[i], 0.25), (trend, 0.25)])
                },
            );
            let promoter = fd.promoter_change.map(|d| if d >= 0.0 { 1.0 } else if d < -1.0 { 0.0 } else { 0.5 });
            values.insert("Smart money", blend(&[(inst[i], 0.7), (promoter, 0.3)]));
            let vs_industry = match (fd.pe, fd.industry_pe) {
                (Some(pe), Some(ind)) if pe > 0.0 && ind > 0.0 => Some((1.5 - pe / ind).clamp(0.0, 1.0)),
                _ => None,
            };
            values.insert("Valuation", blend(&[(peg_score(fd.pe, fd.profit_growth), 0.6), (vs_industry, 0.4)]));
            let headlines = news.get(i).cloned().flatten();
            values.insert("News", headlines.as_ref().map(|h| news_score(h, ctx.now)));

            let factors: Vec<Factor> = spec
                .factors
                .iter()
                .zip(weights)
                .map(|(name, w)| Factor {
                    name: name.to_string(),
                    score: values.get(name).copied().flatten().map(|x| x * 100.0),
                    weight: *w,
                })
                .collect();
            let base = blend(&factors.iter().map(|f| (f.score, f.weight)).collect::<Vec<_>>()).unwrap_or(0.0);

            let mut reasons = Vec::new();
            let mut risks = Vec::new();
            let mut penalty = 0.0;

            if let Some(g) = fd.revenue_growth.filter(|g| *g > 15.0) {
                reasons.push(format!("Revenue up {g:.0}% year on year"));
            }
            match fd.profit_growth {
                Some(g) if g > 20.0 => reasons.push(format!("Profit up {g:.0}% year on year")),
                Some(g) if g < 0.0 => risks.push(format!("Profit down {:.0}% year on year", -g)),
                _ => {}
            }
            if let Some(x) = fd.roe.filter(|x| *x > 18.0) {
                reasons.push(format!("High return on equity: {x:.1}%"));
            }
            if fd.debt_equity.is_some_and(|d| d < 0.2) {
                reasons.push("Almost no debt".into());
            } else if let Some(d) = fd.debt_equity.filter(|d| *d > 1.5) {
                risks.push(format!("Heavily borrowed: debt {d:.1}x equity"));
            }
            if let (Some(pe), Some(g)) = (fd.pe, fd.profit_growth) {
                if pe > 0.0 && g > 0.0 && pe / g < 1.0 {
                    reasons.push(format!("Cheap for its growth: P/E {pe:.0} with profit up {g:.0}%"));
                }
            }
            if short_term {
                if let Some(w) = r.ret_1w.filter(|w| *w > 3.0) {
                    reasons.push(format!("Up {w:.1}% this week"));
                }
                if let Some(v) = r.volume_ratio.filter(|v| *v >= 1.8) {
                    reasons.push(format!("Trading {v:.1}x its usual volume"));
                }
            } else if let (Some(r3), Some(b3)) = (r.ret_3m, ctx.bench_3m) {
                if r3 - b3 > 5.0 {
                    reasons.push(format!("{:.1} pts ahead of the Smallcap 250 over 3 months", r3 - b3));
                }
            }
            if let Some(d) = fd.inst_change {
                if d > 0.5 {
                    reasons.push(format!("Institutions added {d:.1} pts of stake in two quarters"));
                } else if d < -0.5 {
                    risks.push(format!("Institutions cut their stake by {:.1} pts", -d));
                }
            }
            if let Some(d) = fd.promoter_change.filter(|d| *d < -1.0) {
                risks.push(format!("Promoters sold {:.1} pts of stake", -d));
            }
            if let Some(hs) = &headlines {
                let pos = hs.iter().filter(|h| h.tone > 0).count();
                let negc = hs.iter().filter(|h| h.tone < 0).count();
                if pos >= 2 && pos > negc {
                    reasons.push(format!("{pos} positive headlines this week"));
                } else if negc >= 2 && negc > pos {
                    risks.push(format!("{negc} negative headlines this week"));
                }
            }

            if loss_making {
                penalty += 15.0;
                risks.push("Loss-making or profit collapsing".into());
            }
            if fd.market_cap.is_some_and(|m| m < MIN_MARKET_CAP_CR) {
                penalty += 8.0;
                risks.push(format!("Tiny company: ₹{:.0} Cr market value", fd.market_cap.unwrap_or(0.0)));
            }
            let chasing = r.ret_1w.is_some_and(|w| w > spec.chase_week) || r.change_pct.is_some_and(|c| c > 6.0);
            if chasing {
                penalty += 8.0;
                risks.push("Already up sharply this week: wait for a pullback rather than chase".into());
            }
            if r.vs_sma50.is_some_and(|v| v < -12.0) {
                penalty += 8.0;
                risks.push("Well below its 50-day average: still falling".into());
            }
            if !industry.is_empty() && ctx.investor.heavy_sectors.contains(&industry) {
                penalty += 5.0;
                risks.push(format!("You already hold a lot of {industry}"));
            }

            let score = (base - penalty).clamp(0.0, 100.0);
            let verdict = if score >= 68.0 && !chasing {
                "Buy"
            } else if score >= 58.0 {
                "Accumulate"
            } else if score >= 45.0 {
                "Hold"
            } else {
                "Avoid"
            };

            let shares = (ctx.budget / r.price).floor().max(0.0) as u32;
            let sma = |pct: Option<f64>| pct.map(|p| r.price / (1.0 + p / 100.0));
            let m = ctx.meta.get(r.symbol.as_str());
            GrowthPick {
                symbol: r.symbol.clone(),
                name: fd.display_name.clone().unwrap_or_else(|| r.name.clone()),
                industry,
                index: m.map(|m| m.index.to_string()).unwrap_or_default(),
                price: r.price,
                shares,
                cost: shares as f64 * r.price,
                change_pct: r.change_pct,
                ret_1w: r.ret_1w,
                ret_1m: r.ret_1m,
                ret_3m: r.ret_3m,
                volatility: Some(*annual_vol),
                turnover_cr: r.turnover_cr,
                score,
                verdict: verdict.to_string(),
                factors,
                reasons,
                risks,
                prediction: Some(predict(r.price, score, *daily_vol, HORIZONS[spec.horizon], ctx.bands)),
                headlines: headlines.unwrap_or_default().into_iter().take(4).collect(),
                buy_near: sma(r.vs_sma20).filter(|s| *s < r.price),
                support: sma(r.vs_sma50),
                fundamentals: fd,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nse_lists_parse_with_quoted_fields() {
        let body = "Company Name,Industry,Symbol,Series,ISIN Code\n\
                    ACC Ltd.,Construction Materials,ACC,EQ,INE012A01025\n\
                    \"Foo, Bar Ltd.\",Capital Goods,FOOBAR,EQ,INE000000000\n";
        let m = parse_list(body, "Smallcap 250");
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].symbol, "ACC");
        assert_eq!(m[0].industry, "Construction Materials");
        assert_eq!(m[1].name, "Foo, Bar Ltd.");
    }

    #[test]
    fn the_bundled_lists_hold_the_whole_universe() {
        let n: usize = LISTS.iter().map(|(index, _, copy)| parse_list(copy, index).len()).sum();
        // 150 + 250, give or take a row while NSE swaps a constituent.
        assert!((395..=405).contains(&n), "{n}");
    }

    #[test]
    fn peg_rewards_growth_bought_cheaply() {
        assert_eq!(peg_score(Some(15.0), Some(30.0)), Some(1.0));
        assert_eq!(peg_score(Some(60.0), Some(20.0)), Some(0.0));
        let mid = peg_score(Some(30.0), Some(20.0)).unwrap();
        assert!(mid > 0.0 && mid < 1.0);
        // Losses and shrinking profits have no meaningful PEG.
        assert_eq!(peg_score(Some(-5.0), Some(20.0)), None);
        assert_eq!(peg_score(Some(20.0), Some(-10.0)), None);
    }

    #[test]
    fn every_division_weights_total_100() {
        for d in &DIVISIONS {
            assert_eq!(d.factors.len(), d.base.len(), "{}", d.label);
            assert!((d.base.iter().sum::<f64>() - 100.0).abs() < 1e-9, "{}", d.label);
        }
    }

    #[test]
    fn normal_cdf_matches_known_points() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 1e-7);
        assert!((normal_cdf(1.0) - 0.841_344_7).abs() < 1e-6);
        assert!((normal_cdf(-1.96) - 0.024_997_9).abs() < 1e-6);
    }

    #[test]
    fn volatility_is_annualised_from_daily_returns() {
        // Alternating +1% / -1% days: daily sd about 1%, annualised about 15.9%.
        let mut p = 100.0;
        let history: Vec<(i64, f64)> = (0..61)
            .map(|d| {
                p *= if d % 2 == 0 { 1.01 } else { 1.0 / 1.01 };
                (d, p)
            })
            .collect();
        let row = Row { history, ..test_row() };
        let (annual, daily) = volatility(&row).unwrap();
        assert!((daily - 0.01).abs() < 0.001, "{daily}");
        assert!((annual - 15.9).abs() < 0.5, "{annual}");
        assert_eq!(volatility(&Row { history: vec![(0, 1.0); 5], ..test_row() }), None);
    }

    fn test_row() -> Row {
        Row {
            symbol: "X".into(),
            name: "X".into(),
            price: 100.0,
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
            history: vec![],
        }
    }

    #[test]
    fn predictions_tilt_with_score_until_calibrated() {
        let empty: Vec<Band> = vec![];
        let good = predict(100.0, 80.0, 0.02, 20, &empty);
        let bad = predict(100.0, 20.0, 0.02, 20, &empty);
        assert_eq!(good.basis, "model");
        assert!(good.expected > 0.0 && bad.expected < 0.0);
        assert!(good.prob_up > 0.5 && bad.prob_up < 0.5);
        assert!(good.stop < 100.0 && good.target > 100.0);
        // Once a band has enough settled calls, its record replaces the model.
        let bands = vec![Band { lo: 68.0, hi: 100.0, count: 25, mean_ret: Some(-2.0), sd_ret: Some(5.0) }];
        let p = predict(100.0, 80.0, 0.02, 20, &bands);
        assert_eq!(p.expected, -2.0);
        assert_eq!((p.low, p.high), (-7.0, 3.0));
        assert!(p.basis.starts_with("25 past calls"));
    }

    /// Live, ignored by default. `cargo test -- --ignored --nocapture live_growth`.
    #[test]
    #[ignore]
    fn live_growth() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let t = Instant::now();
        let (p, prices) = picks(500.0, false, now, &Learned::default(), &Investor::default()).expect("growth");
        println!("{:?}: {} priced, {} affordable, bench {:?}, gaps {:?}", t.elapsed(), prices.rows.len(), p.affordable, p.bench_3m, p.gaps);
        for d in &p.divisions {
            println!("== {} ({} stocks, vol {:.0}-{:.0}%)", d.label, d.picks.len(), d.vol_low, d.vol_high);
            for x in d.picks.iter().take(4) {
                let pr = x.prediction.as_ref().unwrap();
                println!(
                    "{:>12} {:7.1} {:5.1} {:10} exp {:+.1}% [{:+.1},{:+.1}] up {:.0}% tgt {:.1} stop {:.1}",
                    x.symbol, x.price, x.score, x.verdict, pr.expected, pr.low, pr.high, pr.prob_up * 100.0, pr.target, pr.stop
                );
            }
        }
    }
}
