//! Stock suggestions: which Nifty 50 stocks look set to rise, and which to avoid.
//!
//! # The model
//!
//! Each stock gets five factor scores from 0 to 100, then a weighted mean.
//! These are the starting weights; the pick journal moves them, within
//! bounds, once enough past suggestions have played out (see `pick_journal`):
//!
//! | Factor        | Weight | From                                                    |
//! |---------------|--------|---------------------------------------------------------|
//! | Momentum      | 25     | 3-month return relative to the Nifty, 1-month return, trend |
//! | Fundamentals  | 25     | profit and revenue growth, latest-quarter profit, ROE, debt |
//! | News          | 20     | tone of the company's own headlines this week, recent first |
//! | Smart money   | 15     | change in FII + DII stake over two quarters, promoter selling |
//! | Valuation     | 15     | P/E against its industry's P/E                          |
//!
//! Most inputs are ranked against the other 49 stocks (a percentile), so a
//! score says "better than most of the index on this", not an absolute grade.
//! A factor with no data is dropped and the rest reweighted, so a missing feed
//! makes a score less informed, not lower.
//!
//! Penalties come off the total for setups that tend to disappoint whatever
//! the score: a stock already up sharply this week (chasing), or one far below
//! its 50-day average (a falling knife).
//!
//! # What this is
//!
//! A rules-based screen with its reasons shown. It reads public prices,
//! filings-derived fundamentals and headlines; it cannot know what is not yet
//! public, and past momentum is not a promise. The screen says so.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use super::groww;
use super::stock_board::{self, Row};

/// Factor names, in the order of every weight array. The pick journal keys
/// its learning on these, so renaming one starts its history afresh.
pub const FACTORS: [&str; 5] = ["Momentum", "Fundamentals", "News", "Smart money", "Valuation"];
/// The starting weights, out of 100. The pick journal may move them within
/// bounds once it has enough outcomes (see `pick_journal`).
pub const BASE_WEIGHTS: [f64; 5] = [25.0, 25.0, 20.0, 15.0, 15.0];

/// Fundamentals change quarterly; a twelve-hour cache costs nothing.
const FUNDAMENTALS_FOR: Duration = Duration::from_secs(12 * 3600);
/// Headlines and prices move; the inputs are refetched this often. Scoring
/// itself reruns on every call, so a change of weights shows at once.
const PICKS_FOR: Duration = Duration::from_secs(30 * 60);
/// Requests in flight, shared by Groww and Google News.
const IN_FLIGHT: usize = 8;
/// Headlines older than this do not count.
const NEWS_DAYS: i64 = 7;

static FUNDAMENTALS: Mutex<Option<(Instant, HashMap<String, Fundamentals>)>> = Mutex::new(None);
static INPUTS: Mutex<Option<(Instant, Inputs)>> = Mutex::new(None);

/// Everything fetched for a scoring run.
#[derive(Clone)]
struct Inputs {
    board: stock_board::Board,
    fundamentals: HashMap<String, Fundamentals>,
    news: Vec<Option<Vec<Headline>>>,
    gaps: Vec<String>,
}
/// NSE symbol to Groww page id. Ids do not change, so they are kept for the
/// life of the app and a refresh skips fifty searches.
static GROWW_IDS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
/// A stake shift bigger than this in two quarters is a reclassification in
/// the source data, not buying or selling.
const MAX_STAKE_SHIFT: f64 = 8.0;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fundamentals {
    pub display_name: Option<String>,
    pub pe: Option<f64>,
    pub industry_pe: Option<f64>,
    pub roe: Option<f64>,
    pub debt_equity: Option<f64>,
    /// Trailing-twelve-month growth over the year before, in percent.
    pub revenue_growth: Option<f64>,
    pub profit_growth: Option<f64>,
    /// Latest quarter's profit against the same quarter a year earlier.
    pub profit_q_yoy: Option<f64>,
    /// FII + DII stake change over the last two quarters, in points.
    pub inst_change: Option<f64>,
    pub promoter_change: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Headline {
    pub title: String,
    pub link: String,
    pub source: String,
    pub published: Option<i64>,
    /// +1 positive, -1 negative, 0 neutral, by the keyword rules below.
    pub tone: i8,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Factor {
    pub name: String,
    /// 0 to 100, `None` when there was no data.
    pub score: Option<f64>,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pick {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub change_pct: Option<f64>,
    pub ret_1m: Option<f64>,
    pub ret_3m: Option<f64>,
    pub score: f64,
    /// "Buy", "Accumulate", "Hold" or "Avoid".
    pub verdict: String,
    pub factors: Vec<Factor>,
    pub reasons: Vec<String>,
    pub risks: Vec<String>,
    pub headlines: Vec<Headline>,
    /// The 20-day average when price is above it: a pullback entry level.
    pub buy_near: Option<f64>,
    /// The 50-day average: below it, the trend case is broken.
    pub support: Option<f64>,
    pub fundamentals: Fundamentals,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Picks {
    /// Every stock, best score first.
    pub picks: Vec<Pick>,
    pub nifty_3m: Option<f64>,
    /// Feeds that failed, so the screen can say what the scores lack.
    pub gaps: Vec<String>,
    pub fetched_at: i64,
}

/// Score the Nifty 50 with `weights` (in `FACTORS` order). Also returns the
/// board the scores were built on, for the pick journal.
pub fn picks(refresh: bool, now: i64, weights: &[f64; 5]) -> Result<(Picks, stock_board::Board), String> {
    let cached = if refresh {
        None
    } else {
        INPUTS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|(at, _)| at.elapsed() < PICKS_FOR)
            .map(|(_, i)| i.clone())
    };
    let inputs = match cached {
        Some(i) => i,
        None => {
            let i = fetch_inputs(refresh, now)?;
            *INPUTS.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), i.clone()));
            i
        }
    };

    let board = inputs.board;
    let nifty_3m = board.indices.iter().find(|i| i.symbol == "^NSEI").and_then(|i| i.ret_3m);
    let mut out = score_all(&board.stocks, &inputs.fundamentals, &inputs.news, nifty_3m, now, weights);
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    let fetched_at = board.fetched_at;
    Ok((Picks { picks: out, nifty_3m, gaps: inputs.gaps, fetched_at }, board))
}

fn fetch_inputs(refresh: bool, now: i64) -> Result<Inputs, String> {
    let board = stock_board::board(refresh, now)?;

    // Fundamentals (Groww) and headlines (Google News) are different hosts,
    // so they are fetched side by side; one after the other doubles a cold load.
    let queries: Vec<String> = board.stocks.iter().map(|r| short_name(&r.name)).collect();
    let (fundamentals, news) = std::thread::scope(|scope| {
        let f = scope.spawn(|| load_fundamentals(&board.stocks, refresh));
        let n = in_chunks(&queries, |q| company_news(q, now));
        (f.join().expect("fundamentals worker panicked"), n)
    });

    let mut gaps = Vec::new();
    let missing_f = board.stocks.iter().filter(|r| !fundamentals.contains_key(&r.symbol)).count();
    if missing_f > 0 {
        gaps.push(format!("fundamentals unavailable for {missing_f} stocks"));
    }
    let missing_n = news.iter().filter(|n| n.is_err()).count();
    if missing_n > 0 {
        gaps.push(format!("news unavailable for {missing_n} stocks"));
    }

    let news: Vec<Option<Vec<Headline>>> = news.into_iter().map(|n| n.ok()).collect();
    Ok(Inputs { board, fundamentals, news, gaps })
}

/// Run `f` over `items`, `IN_FLIGHT` at a time, results in input order.
fn in_chunks<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let mut out = Vec::with_capacity(items.len());
    for chunk in items.chunks(IN_FLIGHT) {
        let f = &f;
        let batch: Vec<R> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk.iter().map(|i| scope.spawn(move || f(i))).collect();
            handles.into_iter().map(|h| h.join().expect("worker panicked")).collect()
        });
        out.extend(batch);
    }
    out
}

fn load_fundamentals(stocks: &[Row], refresh: bool) -> HashMap<String, Fundamentals> {
    if !refresh {
        if let Some((at, m)) = FUNDAMENTALS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if at.elapsed() < FUNDAMENTALS_FOR && !m.is_empty() {
                return m.clone();
            }
        }
    }
    let symbols: Vec<String> = stocks.iter().map(|r| r.symbol.clone()).collect();
    let results = in_chunks(&symbols, |s| {
        let known = GROWW_IDS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(s).cloned());
        let id = match known {
            Some(id) => id,
            None => {
                let id = groww::stock_id(s)?;
                GROWW_IDS
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_insert_with(HashMap::new)
                    .insert(s.clone(), id.clone());
                id
            }
        };
        groww::company(&id).map(|v| parse_fundamentals(&v))
    });
    let mut map = HashMap::new();
    for (s, r) in symbols.into_iter().zip(results) {
        match r {
            Ok(f) => {
                map.insert(s, f);
            }
            Err(e) => log::warn!("stock_picks: fundamentals for {s}: {e}"),
        }
    }
    if !map.is_empty() {
        *FUNDAMENTALS.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), map.clone()));
    }
    map
}

/// "Reliance Industries Limited" -> "Reliance Industries", for a news search.
fn short_name(name: &str) -> String {
    let mut s = name.trim().to_string();
    for suffix in [" Limited", " Ltd.", " Ltd", " LIMITED", " LTD"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.to_string();
        }
    }
    s
}

/// Quarter labels ("Sep '26") as a sortable month count.
fn quarter_key(label: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (m, y) = label.split_once(" '")?;
    let month = MONTHS.iter().position(|x| *x == m.trim())? as i64;
    let year: i64 = y.trim().parse().ok()?;
    Some(year * 12 + month)
}

fn parse_fundamentals(v: &Value) -> Fundamentals {
    let mut f = Fundamentals {
        display_name: v
            .pointer("/header/displayName")
            .and_then(|n| n.as_str())
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty()),
        ..Default::default()
    };

    if let Some(list) = v.get("fundamentals").and_then(|x| x.as_array()) {
        for item in list {
            let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let value = item.get("value").and_then(|n| n.as_str()).and_then(groww::number);
            match name {
                "ROE" => f.roe = value,
                "P/E Ratio(TTM)" => f.pe = value,
                "Industry P/E" => f.industry_pe = value,
                "Debt to Equity" => f.debt_equity = value,
                _ => {}
            }
        }
    }

    if let Some(list) = v.get("financialStatement").and_then(|x| x.as_array()) {
        for item in list {
            let title = item.get("title").and_then(|n| n.as_str()).unwrap_or("");
            let ttm = item.pointer("/cagr/oneYearTtm").and_then(|n| n.as_f64()).map(|g| g * 100.0);
            match title {
                "Revenue" => f.revenue_growth = ttm,
                "Profit" => {
                    f.profit_growth = ttm;
                    f.profit_q_yoy = item.get("quarterly").and_then(|q| q.as_object()).and_then(|q| {
                        let mut rows: Vec<(i64, f64)> = q
                            .iter()
                            .filter_map(|(k, v)| Some((quarter_key(k)?, v.as_f64()?)))
                            .collect();
                        rows.sort_by_key(|r| r.0);
                        let (k, latest) = *rows.last()?;
                        let (_, year_ago) = *rows.iter().find(|r| r.0 == k - 12)?;
                        // A swing from loss to profit is not a percentage.
                        (year_ago > 0.0).then(|| (latest - year_ago) / year_ago * 100.0)
                    });
                }
                _ => {}
            }
        }
    }

    if let Some(data) = v.pointer("/shareholdingPatternV2/data").and_then(|d| d.as_object()) {
        let mut quarters: Vec<(i64, &Value)> = data
            .iter()
            .filter_map(|(k, v)| Some((quarter_key(k)?, v)))
            .collect();
        quarters.sort_by_key(|q| std::cmp::Reverse(q.0));
        let get = |q: &Value, k: &str| q.get(k).and_then(|x| x.as_f64());
        if quarters.len() >= 2 {
            let latest = quarters[0].1;
            let earlier = quarters[2.min(quarters.len() - 1)].1;
            let inst = |q: &Value| Some(get(q, "fii")? + get(q, "dii")?);
            let plausible = |d: f64| (d.abs() <= MAX_STAKE_SHIFT).then_some(d);
            f.inst_change = inst(latest).zip(inst(earlier)).and_then(|(a, b)| plausible(a - b));
            f.promoter_change = get(latest, "promoters")
                .zip(get(earlier, "promoters"))
                .and_then(|(a, b)| plausible(a - b));
        }
    }
    f
}

/// Phrases that mark a headline's tone, matched at the start of a word.
const POSITIVE: &[&str] = &[
    "surge", "soar", "jump", "rall", "gain", "climb", "record high", "all-time high", "52-week high",
    "upgrade", "outperform", "beat", "strong", "profit rise", "profit up", "profit jump", "record profit",
    "record revenue", "order win", "wins order", "bags", "secures", "approval", "buyback", "bonus issue",
    "top pick", "bullish", "multibagger", "target raised", "raises target", "hikes target", "expansion",
];
const NEGATIVE: &[&str] = &[
    "fall", "slump", "plunge", "crash", "tumble", "declin", "drop", "downgrade", "underperform",
    "sell rating", "cuts target", "target cut", "miss", "weak", "loss", "probe", "penalt", "fraud",
    "raid", "notice", "resign", "default", "bearish", "52-week low", "lawsuit", "strike", "slows",
    "sink", "slide", "sheds", "selloff", "sell-off",
];

pub fn tone(title: &str) -> i8 {
    let t = format!(" {}", title.to_lowercase());
    let hits = |list: &[&str]| list.iter().filter(|p| t.contains(&format!(" {p}"))).count();
    match hits(POSITIVE).cmp(&hits(NEGATIVE)) {
        std::cmp::Ordering::Greater => 1,
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
    }
}

fn company_news(name: &str, now: i64) -> Result<Vec<Headline>, String> {
    let q = format!("\"{name}\" share when:{NEWS_DAYS}d");
    let url = format!(
        "https://news.google.com/rss/search?q={}&hl=en-IN&gl=IN&ceid=IN:en",
        groww::encode(&q)
    );
    // One retry: Google throttles bursts briefly, and a second ask usually lands.
    let items = super::news_connector::fetch_one(name, &url)
        .or_else(|_| {
            std::thread::sleep(Duration::from_millis(1500));
            super::news_connector::fetch_one(name, &url)
        })
        .map_err(|e| e.to_string())?;
    let cutoff = now - NEWS_DAYS * 86_400;
    let mut out: Vec<Headline> = items
        .into_iter()
        .filter_map(|n| {
            let published = n.published.as_deref().and_then(super::news_desk::parse_date);
            if published.is_some_and(|p| p < cutoff) {
                return None;
            }
            // Google appends " - Publisher" to every title.
            let (title, source) = match n.title.rsplit_once(" - ") {
                Some((t, s)) => (t.trim().to_string(), s.trim().to_string()),
                None => (n.title.clone(), "Google News".to_string()),
            };
            Some(Headline { tone: tone(&title), title, link: n.link, source, published })
        })
        .collect();
    out.sort_by_key(|h| std::cmp::Reverse(h.published.unwrap_or(0)));
    out.truncate(12);
    Ok(out)
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
                (below as f64 + (equal.saturating_sub(1)) as f64 / 2.0) / (n - 1) as f64
            })
        })
        .collect()
}

/// Weighted mean of the present parts, `None` when none are.
fn blend(parts: &[(Option<f64>, f64)]) -> Option<f64> {
    let (sum, w) = parts
        .iter()
        .filter_map(|(v, w)| v.map(|v| (v * w, *w)))
        .fold((0.0, 0.0), |(s, t), (a, b)| (s + a, t + b));
    (w > 0.0).then(|| sum / w)
}

fn neg(v: Option<f64>) -> Option<f64> {
    v.map(|x| -x)
}

fn score_all(
    stocks: &[Row],
    fundamentals: &HashMap<String, Fundamentals>,
    news: &[Option<Vec<Headline>>],
    nifty_3m: Option<f64>,
    now: i64,
    weights: &[f64; 5],
) -> Vec<Pick> {
    let f = |r: &Row| fundamentals.get(&r.symbol).cloned().unwrap_or_default();
    let col = |get: &dyn Fn(&Row) -> Option<f64>| percentiles(&stocks.iter().map(get).collect::<Vec<_>>());

    let rs3 = col(&|r| r.ret_3m.map(|x| x - nifty_3m.unwrap_or(0.0)));
    let r1m = col(&|r| r.ret_1m);
    let profit = col(&|r| f(r).profit_growth);
    let revenue = col(&|r| f(r).revenue_growth);
    let profit_q = col(&|r| f(r).profit_q_yoy);
    let roe = col(&|r| f(r).roe);
    let low_debt = col(&|r| neg(f(r).debt_equity));
    let inst = col(&|r| f(r).inst_change);

    stocks
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let fd = f(r);
            let trend = match (r.vs_sma20, r.vs_sma50) {
                (Some(a), Some(b)) if a > 0.0 && b > 0.0 && a < b => Some(1.0),
                (Some(a), Some(b)) if a < 0.0 && b < 0.0 && a > b => Some(0.0),
                (_, Some(b)) if b > 0.0 => Some(0.6),
                (_, Some(_)) => Some(0.3),
                _ => None,
            };
            let momentum = blend(&[(rs3[i], 0.5), (r1m[i], 0.25), (trend, 0.25)]);
            let fundamentals_s = blend(&[
                (profit[i], 0.3),
                (revenue[i], 0.25),
                (profit_q[i], 0.15),
                (roe[i], 0.2),
                (low_debt[i], 0.1),
            ]);
            // 0.5x the industry P/E scores 1, parity 0.5, 1.5x scores 0.
            let valuation = match (fd.pe, fd.industry_pe) {
                (Some(pe), Some(ind)) if pe > 0.0 && ind > 0.0 => Some((1.5 - pe / ind).clamp(0.0, 1.0)),
                _ => None,
            };
            let promoter = fd.promoter_change.map(|d| if d >= 0.0 { 1.0 } else if d < -1.0 { 0.0 } else { 0.5 });
            let smart = blend(&[(inst[i], 0.7), (promoter, 0.3)]);

            let headlines = news[i].clone();
            let news_s = headlines.as_ref().map(|hs| {
                let raw: f64 = hs
                    .iter()
                    .map(|h| {
                        let age_days = h.published.map(|p| (now - p).max(0) as f64 / 86_400.0).unwrap_or(3.0);
                        h.tone as f64 * (-age_days / 4.0).exp()
                    })
                    .sum();
                0.5 + 0.5 * (raw / 2.5).tanh()
            });

            let factors: Vec<Factor> = [momentum, fundamentals_s, news_s, smart, valuation]
                .into_iter()
                .zip(FACTORS.iter().zip(weights))
                .map(|(score, (name, weight))| Factor {
                    name: name.to_string(),
                    score: score.map(|x| x * 100.0),
                    weight: *weight,
                })
                .collect();
            let base = blend(&factors.iter().map(|f| (f.score, f.weight)).collect::<Vec<_>>()).unwrap_or(0.0);

            let mut reasons = Vec::new();
            let mut risks = Vec::new();
            let mut penalty = 0.0;

            if let (Some(r3), Some(n3)) = (r.ret_3m, nifty_3m) {
                if r3 - n3 > 3.0 {
                    let moved = if r3 >= 0.0 { format!("Up {r3:.1}%") } else { format!("Down only {:.1}%", -r3) };
                    reasons.push(format!("{moved} in 3 months, {:.1} pts ahead of the Nifty", r3 - n3));
                } else if r3 - n3 < -5.0 {
                    risks.push(format!("Lagging the Nifty by {:.1} pts over 3 months", n3 - r3));
                }
            }
            if trend == Some(1.0) {
                reasons.push("Rising trend: above its 20- and 50-day averages".into());
            }
            if let Some(g) = fd.profit_growth {
                if g > 15.0 {
                    reasons.push(format!("Profit up {g:.0}% year on year"));
                } else if g < 0.0 {
                    risks.push(format!("Profit down {:.0}% year on year", -g));
                }
            }
            // Groww's trailing figure is often the latest quarter itself; say it once.
            let distinct = |g: &f64| fd.profit_growth.map_or(true, |t| (t - g).abs() > 5.0);
            if let Some(g) = fd.profit_q_yoy.filter(|g| *g > 20.0).filter(distinct) {
                reasons.push(format!("Latest quarter profit up {g:.0}% on last year"));
            }
            if let Some(g) = fd.revenue_growth.filter(|g| *g > 12.0) {
                reasons.push(format!("Revenue up {g:.0}%"));
            }
            if let Some(x) = fd.roe.filter(|x| *x > 18.0) {
                reasons.push(format!("High return on equity: {x:.1}%"));
            }
            if let (Some(pe), Some(ind)) = (fd.pe, fd.industry_pe) {
                if pe > 0.0 && pe < ind * 0.85 {
                    reasons.push(format!("Cheaper than peers: P/E {pe:.1} vs industry {ind:.1}"));
                } else if pe > ind * 1.5 {
                    risks.push(format!("Expensive: P/E {pe:.1} vs industry {ind:.1}"));
                }
            }
            if let Some(d) = fd.inst_change {
                if d > 0.5 {
                    reasons.push(format!("Institutions (FII + DII) added {d:.1} pts of stake in two quarters"));
                } else if d < -0.5 {
                    risks.push(format!("Institutions cut their stake by {:.1} pts in two quarters", -d));
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
                }
                if negc >= 2 && negc > pos {
                    risks.push(format!("{negc} negative headlines this week"));
                }
            }

            let chasing = r.ret_1w.is_some_and(|w| w > 8.0) || r.change_pct.is_some_and(|c| c > 5.0);
            if chasing {
                penalty += 8.0;
                risks.push("Already up sharply this week: wait for a pullback rather than chase".into());
            }
            if r.vs_sma50.is_some_and(|v| v < -10.0) {
                penalty += 8.0;
                risks.push("More than 10% below its 50-day average: still falling".into());
            }

            let score = (base - penalty).clamp(0.0, 100.0);
            // A strong name that just ran up is a buy on the dip, not today.
            let verdict = if score >= 68.0 && !chasing {
                "Buy"
            } else if score >= 58.0 {
                "Accumulate"
            } else if score >= 45.0 {
                "Hold"
            } else {
                "Avoid"
            };

            let sma = |pct: Option<f64>| pct.map(|p| r.price / (1.0 + p / 100.0));
            Pick {
                symbol: r.symbol.clone(),
                name: r.name.clone(),
                price: r.price,
                change_pct: r.change_pct,
                ret_1m: r.ret_1m,
                ret_3m: r.ret_3m,
                score,
                verdict: verdict.to_string(),
                factors,
                reasons,
                risks,
                headlines: headlines.unwrap_or_default(),
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
    fn headline_tone_reads_common_market_phrasing() {
        assert_eq!(tone("Reliance Industries stock surges 2% as Jio IPO gets subscribed"), 1);
        assert_eq!(tone("TCS shares fall after weak Q2 results"), -1);
        assert_eq!(tone("Infosys to announce results on Thursday"), 0);
        // Matched at word starts, so "rall" does not fire inside "overall".
        assert_eq!(tone("Overall market view for the week"), 0);
    }

    #[test]
    fn percentiles_rank_present_values_and_skip_missing() {
        let p = percentiles(&[Some(1.0), None, Some(3.0), Some(2.0)]);
        assert_eq!(p, vec![Some(0.0), None, Some(1.0), Some(0.5)]);
        assert_eq!(percentiles(&[Some(5.0)]), vec![Some(0.5)]);
    }

    #[test]
    fn blend_reweights_around_missing_parts() {
        assert_eq!(blend(&[(Some(1.0), 1.0), (None, 3.0)]), Some(1.0));
        assert_eq!(blend(&[(None, 1.0)]), None);
    }

    #[test]
    fn quarter_labels_sort_in_time() {
        assert!(quarter_key("Sep '26") > quarter_key("Jun '26"));
        assert!(quarter_key("Jan '26") > quarter_key("Dec '25"));
        assert_eq!(quarter_key("Sep '26").unwrap() - quarter_key("Sep '25").unwrap(), 12);
    }

    #[test]
    fn fundamentals_are_read_from_a_groww_company_body() {
        let v: Value = serde_json::from_str(
            r#"{"header":{"displayName":"Reliance Industries"},
            "fundamentals":[{"name":"ROE","value":"8.94%"},{"name":"P/E Ratio(TTM)","value":"18.27"},
              {"name":"Industry P/E","value":"15.22"},{"name":"Debt to Equity","value":"0.44"}],
            "financialStatement":[{"title":"Revenue","cagr":{"oneYearTtm":0.2}},
              {"title":"Profit","quarterly":{"Jun '25":100,"Sep '25":90,"Jun '26":130},"cagr":{"oneYearTtm":-0.25}}],
            "shareholdingPatternV2":{"data":{
              "Sep '26":{"promoters":50.4,"fii":16.6,"dii":21.7},
              "Jun '26":{"promoters":50.5,"fii":17.2,"dii":21.2},
              "Mar '26":{"promoters":50.0,"fii":18.6,"dii":20.5}}}}"#,
        )
        .unwrap();
        let f = parse_fundamentals(&v);
        assert_eq!(f.display_name.as_deref(), Some("Reliance Industries"));
        assert_eq!(f.roe, Some(8.94));
        assert_eq!(f.pe, Some(18.27));
        assert_eq!(f.revenue_growth, Some(20.0));
        assert_eq!(f.profit_growth, Some(-25.0));
        assert!((f.profit_q_yoy.unwrap() - 30.0).abs() < 1e-9);
        // (16.6 + 21.7) - (18.6 + 20.5) = -0.8
        assert!((f.inst_change.unwrap() + 0.8).abs() < 1e-9);
        assert!((f.promoter_change.unwrap() - 0.4).abs() < 1e-9);
    }

    /// Live, ignored by default. `cargo test -- --ignored --nocapture live_news`.
    #[test]
    #[ignore]
    fn live_news() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let names = ["Tata Steel", "Infosys", "Mahindra & Mahindra", "Bajaj Auto", "Nestle India", "Hindalco"];
        for (n, r) in names.iter().zip(in_chunks(&names, |n| company_news(n, now))) {
            match r {
                Ok(h) => println!("{n}: {} headlines", h.len()),
                Err(e) => println!("{n}: ERROR {e}"),
            }
        }
    }

    /// Live, ignored by default. `cargo test -- --ignored --nocapture live_picks`.
    #[test]
    #[ignore]
    fn live_picks() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let (p, _) = picks(true, now, &BASE_WEIGHTS).expect("picks");
        println!("gaps {:?}", p.gaps);
        for x in p.picks.iter().take(10).chain(p.picks.iter().rev().take(3)) {
            println!("{:>10} {:5.1} {:10} {:?}", x.symbol, x.score, x.verdict, x.factors.iter().map(|f| f.score.map(|s| s.round())).collect::<Vec<_>>());
            println!("           + {:?}\n           - {:?}", x.reasons, x.risks);
        }
    }
}
