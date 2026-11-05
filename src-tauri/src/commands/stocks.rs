//! The Stocks section: the market board, the suggestions (Nifty 50, the three
//! mid and small-cap divisions, funds, the news radar), the investor's risk
//! level, and the journal that grades every suggestion and the investor's
//! own buys.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::State;

use super::{put_setting, setting};
use crate::assistant::fund_picks::{self, FlowHistory, FundPicks};
use crate::assistant::growth_picks::{self, GrowthPicks, Investor, Learned, DIVISIONS};
use crate::assistant::my_trades;
use crate::assistant::news_radar::{self, Radar};
use crate::assistant::pick_journal::{
    self, NewEntry, TrackRecord, KIND_FUND, KIND_GROWTH, KIND_MINE, KIND_NEWS, KIND_STOCK, NOT_COVERED,
};
use crate::assistant::reasoning::{self, ContextCategory, Purpose};
use crate::assistant::stock_board::{self, day_of, Board, Row, IST_OFFSET};
use crate::assistant::stock_picks::{self, Factor, Picks};
use crate::db::DbState;

/// Daily AUM and NAV per fund, the history implied fund flows are read from.
const FLOW_KEY: &str = "fund_flow_history";
/// The last news radar, so reopening the tab does not re-run the research.
const RADAR_KEY: &str = "news_radar";
/// How long a radar stands before it is researched again unprompted.
const RADAR_FOR: i64 = 4 * 3600;
/// Claude reads a long brief; give it time.
const RADAR_TIMEOUT: Duration = Duration::from_secs(180);
/// "cautious", "balanced" or "aggressive".
const RISK_KEY: &str = "invest_risk";
const RISKS: [&str; 3] = ["cautious", "balanced", "aggressive"];
/// The Report settles outcomes at most this often; each settle prices every
/// stock still waiting for one.
const SETTLE_EVERY: Duration = Duration::from_secs(30 * 60);
static LAST_SETTLE: Mutex<Option<Instant>> = Mutex::new(None);

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn lock(state: &DbState) -> Result<std::sync::MutexGuard<'_, rusqlite::Connection>, String> {
    state.0.lock().map_err(|e| format!("Lock error: {e}"))
}

fn factor_pairs(factors: &[Factor]) -> Vec<(String, Option<f64>)> {
    factors.iter().map(|f| (f.name.clone(), f.score)).collect()
}

/// Record a day's suggestions, logging rather than failing: a journal
/// problem must not cost the screen its suggestions.
fn record(conn: &rusqlite::Connection, kind: &str, day: i64, entries: &[NewEntry]) {
    if pick_journal::is_weekday(day) {
        if let Err(e) = pick_journal::record(conn, kind, day, entries) {
            log::warn!("pick journal: {e}");
        }
    }
}

fn settle(conn: &rusqlite::Connection, kind: &str, rows: &[Row], bench: Option<&Row>) {
    if let Err(e) = pick_journal::settle_prices(conn, kind, rows, bench.map(|b| b.history.as_slice())) {
        log::warn!("pick journal: {e}");
    }
}

/// Indices and the Nifty 50 with their returns. Cached for five minutes
/// unless `refresh`.
#[tauri::command(async)]
pub fn nexus_stock_board(refresh: Option<bool>) -> Result<Board, String> {
    stock_board::board(refresh.unwrap_or(false), now_unix())
}

/// The Nifty 50 scored with the journal's current weights. The day's scores
/// are recorded, and past ones settled, as a side effect.
#[tauri::command(async)]
pub fn nexus_stock_picks(state: State<'_, DbState>, refresh: Option<bool>) -> Result<Picks, String> {
    let weights = {
        let conn = lock(&state)?;
        pick_journal::learned_weights(&conn, KIND_STOCK, &stock_picks::FACTORS, &stock_picks::BASE_WEIGHTS)
            .map(|(w, _)| w)
            .unwrap_or_else(|_| stock_picks::BASE_WEIGHTS.to_vec())
    };
    let weights: [f64; 5] = weights.try_into().unwrap_or(stock_picks::BASE_WEIGHTS);
    let (picks, board) = stock_picks::picks(refresh.unwrap_or(false), now_unix(), &weights)?;

    let conn = lock(&state)?;
    // Filed under the date of the last session priced, so a weekend or
    // holiday visit files Friday's scores under Friday, once.
    let nifty = board.indices.iter().find(|i| i.symbol == "^NSEI");
    if board.as_of > 0 {
        let entries: Vec<NewEntry> = picks
            .picks
            .iter()
            .map(|p| NewEntry {
                symbol: p.symbol.clone(),
                score: p.score,
                verdict: p.verdict.clone(),
                price: p.price,
                bench: nifty.map(|n| n.price),
                factors: factor_pairs(&p.factors),
            })
            .collect();
        record(&conn, KIND_STOCK, day_of(board.as_of, IST_OFFSET), &entries);
    }
    settle(&conn, KIND_STOCK, &board.stocks, nifty);
    Ok(picks)
}

/// What the investor holds, and which of their sectors are already heavy,
/// priced from the mid and small-cap rows.
fn investor_from(conn: &rusqlite::Connection, prices: &growth_picks::Prices) -> Investor {
    let industry: HashMap<String, String> =
        growth_picks::members().into_iter().map(|m| (m.symbol, m.industry)).collect();
    let sector_value: HashMap<String, (String, f64)> = prices
        .rows
        .iter()
        .filter_map(|r| Some((r.symbol.clone(), (industry.get(&r.symbol)?.clone(), r.price))))
        .collect();
    my_trades::investor(&my_trades::holdings(conn), &sector_value)
}

/// The mid and small caps within a monthly `budget`, in three divisions by
/// volatility, each scored with its own learned weights and predicted from
/// its own record. Held stocks are left out. Each division is recorded and
/// settled under its own kind.
#[tauri::command(async)]
pub fn nexus_growth_picks(state: State<'_, DbState>, budget: f64, refresh: Option<bool>) -> Result<GrowthPicks, String> {
    if !(budget.is_finite() && budget > 0.0) {
        return Err("Enter a monthly amount above zero.".to_string());
    }
    let refresh = refresh.unwrap_or(false);
    let prices = growth_picks::prices(refresh)?;
    let (learned, investor) = {
        let conn = lock(&state)?;
        let mut learned = Learned::default();
        for d in &DIVISIONS {
            let (w, _) = pick_journal::learned_weights(&conn, d.kind, d.factors, d.base)?;
            learned.weights.insert(d.kind.to_string(), w);
            learned.bands.insert(d.kind.to_string(), pick_journal::calibration(&conn, d.kind, d.horizon)?);
        }
        (learned, investor_from(&conn, &prices))
    };
    let (picks, prices) = growth_picks::picks(budget, false, now_unix(), &learned, &investor)?;

    let conn = lock(&state)?;
    let day = day_of(prices.as_of, IST_OFFSET);
    for division in &picks.divisions {
        if prices.as_of > 0 {
            let entries: Vec<NewEntry> = division
                .picks
                .iter()
                .map(|p| NewEntry {
                    symbol: p.symbol.clone(),
                    score: p.score,
                    verdict: p.verdict.clone(),
                    price: p.price,
                    bench: prices.bench.as_ref().map(|b| b.price),
                    factors: factor_pairs(&p.factors),
                })
                .collect();
            record(&conn, &division.kind, day, &entries);
        }
        settle(&conn, &division.kind, &prices.rows, prices.bench.as_ref());
    }
    settle(&conn, KIND_GROWTH, &prices.rows, prices.bench.as_ref());
    Ok(picks)
}

/// Small-cap funds scored for the Suits-for-you tab. Each call records the
/// day's AUM snapshot and, on a weekday, the day's ranking; the lock is
/// released while the funds are fetched.
#[tauri::command(async)]
pub fn nexus_fund_picks(state: State<'_, DbState>, refresh: Option<bool>) -> Result<FundPicks, String> {
    let mut history: FlowHistory = {
        let conn = lock(&state)?;
        setting(&conn, FLOW_KEY).and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
    };
    let now = now_unix();
    let picks = fund_picks::picks(refresh.unwrap_or(false), now, &mut history)?;
    let conn = lock(&state)?;
    put_setting(&conn, FLOW_KEY, &serde_json::to_string(&history).map_err(|e| e.to_string())?)?;

    let entries: Vec<NewEntry> = picks
        .funds
        .iter()
        .filter_map(|f| {
            Some(NewEntry {
                symbol: f.code.clone(),
                score: f.score,
                verdict: f.verdict.clone(),
                price: f.nav?,
                bench: None,
                factors: f.factors.iter().map(|x| (x.name.clone(), x.score)).collect(),
            })
        })
        .collect();
    record(&conn, KIND_FUND, day_of(now, IST_OFFSET), &entries);
    Ok(picks)
}

/// The investor's risk level, "balanced" until they choose.
#[tauri::command(async)]
pub fn nexus_invest_risk(state: State<'_, DbState>) -> Result<String, String> {
    let conn = lock(&state)?;
    Ok(setting(&conn, RISK_KEY).filter(|r| RISKS.contains(&r.as_str())).unwrap_or_else(|| "balanced".to_string()))
}

#[tauri::command(async)]
pub fn nexus_set_invest_risk(state: State<'_, DbState>, risk: String) -> Result<String, String> {
    if !RISKS.contains(&risk.as_str()) {
        return Err(format!("\"{risk}\" is not a risk level. Use cautious, balanced or aggressive."));
    }
    let conn = lock(&state)?;
    put_setting(&conn, RISK_KEY, &risk)?;
    Ok(risk)
}

/// Today's news radar: market-moving events, the Indian stocks they touch,
/// a plan and a prediction for each. Researched by Claude when external
/// reasoning is allowed and Claude Code answers, otherwise by the local
/// model; kept for four hours unless `refresh`. Every stock call is recorded
/// in the journal and graded later.
#[tauri::command(async)]
pub fn nexus_news_radar(state: State<'_, DbState>, refresh: Option<bool>) -> Result<Radar, String> {
    let now = now_unix();
    let (cached, news_settings, external_ok, local_model, held, feedback, bands) = {
        let conn = lock(&state)?;
        let cached: Option<Radar> = setting(&conn, RADAR_KEY).and_then(|raw| serde_json::from_str(&raw).ok());
        let held: HashSet<String> = my_trades::holdings(&conn)
            .iter()
            .filter(|h| h.is_stock() && h.quantity > 0.0)
            .map(|h| h.ticker())
            .collect();
        (
            cached,
            setting(&conn, "news_settings").and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default(),
            reasoning::read_policy(&conn).external_reasoning_allowed,
            crate::assistant::ollama_provider::local_model(&conn),
            held,
            pick_journal::news_feedback(&conn, 1)?,
            pick_journal::calibration(&conn, KIND_NEWS, 1)?,
        )
    };
    if let Some(r) = cached.filter(|r| !refresh.unwrap_or(false) && now - r.generated_at < RADAR_FOR) {
        return Ok(r);
    }

    let desk = crate::assistant::news_desk::desk(&news_settings, false, now);
    let headlines = news_radar::headlines(&desk, now);
    if headlines.is_empty() {
        return Err("No recent business or tech headlines to read. Check the News section's sources.".to_string());
    }
    let members = growth_picks::members();
    let prices = growth_picks::prices(false)?;
    let board = stock_board::board(false, now)?;
    let large_symbols: Vec<String> = board.stocks.iter().map(|r| r.symbol.clone()).collect();
    let fundamentals = stock_picks::fundamentals_for(&large_symbols, false);
    let flows = news_radar::sector_flows(&members, &prices.rows);
    let inputs = news_radar::Inputs {
        headlines,
        members: &members,
        large: &board.stocks,
        prices: &prices.rows,
        fundamentals: &fundamentals,
        feedback: feedback.clone(),
        held: &held,
        bands: &bands,
    };

    // Claude first for its reasoning about knock-on effects; the local model
    // if Claude is switched off or does not answer.
    let started = Instant::now();
    let mut tried = Vec::new();
    let mut answer: Option<(String, &str, String)> = None;
    if external_ok && crate::assistant::claude_provider::binary().is_some() {
        match crate::assistant::claude_provider::complete(&news_radar::prompt(&inputs, &flows, true), RADAR_TIMEOUT) {
            Ok(text) => answer = Some((text, "claude-code", "claude-code".to_string())),
            Err(e) => tried.push(format!("Claude: {e}")),
        }
    }
    if answer.is_none() {
        match crate::assistant::ollama_provider::generate_text(&local_model, &news_radar::prompt(&inputs, &flows, false)) {
            Ok(text) => answer = Some((text, "ollama", local_model.clone())),
            Err(e) => tried.push(format!("local model: {e}")),
        }
    }
    let Some((reply, provider, model)) = answer else {
        return Err(format!("No model could research the news. {}", tried.join(" ")));
    };
    let elapsed = started.elapsed().as_millis();
    let mut events = news_radar::build(&reply, &inputs, &flows)?;

    // Company data for the radar's stocks, for the cards.
    let radar_symbols: Vec<String> = events.iter().flat_map(|e| e.stocks.iter().map(|s| s.symbol.clone())).collect();
    let radar_fundamentals = stock_picks::fundamentals_for(&radar_symbols, false);
    for s in events.iter_mut().flat_map(|e| e.stocks.iter_mut()) {
        s.fundamentals = radar_fundamentals.get(&s.symbol).cloned();
    }

    let radar = Radar {
        events,
        inflows: flows.iter().take(6).cloned().collect(),
        outflows: flows.iter().rev().take(4).cloned().collect(),
        read: inputs.headlines.clone(),
        provider: if provider == "ollama" { format!("local model ({model})") } else { "Claude".to_string() },
        feedback,
        generated_at: now,
    };

    let conn = lock(&state)?;
    let reach = if provider == "ollama" {
        crate::assistant::permission::Reach::LocalOnly
    } else {
        crate::assistant::permission::Reach::LeavesMachine
    };
    let _ = reasoning::record_use(
        &conn,
        provider,
        &model,
        reach,
        Purpose::Summarise,
        &[ContextCategory::Request],
        &format!("news radar: {} events", radar.events.len()),
        elapsed,
    );
    let nifty = board.indices.iter().find(|i| i.symbol == "^NSEI").map(|n| n.price);
    let mut seen = HashSet::new();
    let entries: Vec<NewEntry> = radar
        .events
        .iter()
        .flat_map(|e| e.stocks.iter().map(move |s| (e, s)))
        // One call per stock a day: the first event to name it.
        .filter(|(_, s)| seen.insert(s.symbol.clone()))
        .map(|(e, s)| {
            let score = news_radar::confidence_score(&e.confidence, &s.direction);
            let mut factors = vec![("confidence".to_string(), Some(match e.confidence.as_str() {
                "high" => 80.0,
                "medium" => 60.0,
                _ => 40.0,
            }))];
            if !s.industry.is_empty() {
                factors.push((format!("sector:{}", s.industry), Some(1.0)));
            }
            NewEntry {
                symbol: s.symbol.clone(),
                score,
                verdict: if s.direction == "down" { "Down" } else { "Up" }.to_string(),
                price: s.price,
                bench: nifty,
                factors,
            }
        })
        .collect();
    record(&conn, KIND_NEWS, day_of(now, IST_OFFSET), &entries);
    put_setting(&conn, RADAR_KEY, &serde_json::to_string(&radar).map_err(|e| e.to_string())?)?;
    Ok(radar)
}

/// Price the stocks still waiting for an outcome, plus their benchmark.
/// Network work: call off the lock.
fn price_pending(pending: &[(String, i64)], bench: &str) -> (Vec<Row>, Option<Row>) {
    if pending.is_empty() {
        return (Vec::new(), None);
    }
    let symbols: Vec<String> = std::iter::once(bench.to_string())
        .chain(pending.iter().map(|(s, _)| format!("{s}.NS")))
        .collect();
    let mut rows = stock_board::fetch_rows(&symbols).into_iter();
    let bench = rows.next().and_then(|r| r.ok()).map(|(r, _)| r);
    (rows.flatten().map(|(r, _)| r).collect(), bench)
}

/// Record stock buys made since the last look, each with what NEXUS said
/// about that stock that day.
fn capture_buys(state: &DbState) -> Result<(), String> {
    let buys = {
        let conn = lock(state)?;
        my_trades::new_buys(&conn)?
    };
    if buys.is_empty() {
        return Ok(());
    }
    let symbols: Vec<String> = std::iter::once("^NSEI".to_string())
        .chain(buys.iter().map(|b| format!("{}.NS", b.holding.ticker())))
        .collect();
    let mut rows = stock_board::fetch_rows(&symbols).into_iter();
    let nifty = rows.next().and_then(|r| r.ok()).map(|(r, _)| r.price);
    let priced: HashMap<String, f64> = rows.flatten().map(|(r, _)| (r.symbol, r.price)).collect();

    let conn = lock(state)?;
    let day = day_of(now_unix(), IST_OFFSET);
    let entries: Vec<NewEntry> = buys
        .iter()
        .filter_map(|b| {
            let ticker = b.holding.ticker();
            let market = *priced.get(&ticker)?;
            // What was paid when it is believable, else today's price.
            let paid = b.paid.map(|p| p / b.added).filter(|p| (p / market - 1.0).abs() < 0.3).unwrap_or(market);
            let (verdict, score) =
                pick_journal::nexus_view(&conn, day, &ticker).unwrap_or_else(|| (NOT_COVERED.to_string(), 0.0));
            Some(NewEntry { symbol: ticker, score, verdict, price: paid, bench: nifty, factors: vec![] })
        })
        .collect();
    // Buys are recorded whatever the day: a weekend entry is still a decision.
    pick_journal::record(&conn, KIND_MINE, day, &entries)?;
    Ok(())
}

/// How past suggestions of every kind turned out, how the investor's own buys
/// did, and what each stock score has learned. Pending outcomes are settled
/// first, at most every half hour, with prices and NAVs fetched off the lock.
#[tauri::command(async)]
pub fn nexus_pick_track_record(state: State<'_, DbState>) -> Result<TrackRecord, String> {
    if let Err(e) = capture_buys(&state) {
        log::warn!("pick journal: {e}");
    }
    let due = LAST_SETTLE.lock().unwrap_or_else(|e| e.into_inner()).map_or(true, |at| at.elapsed() >= SETTLE_EVERY);
    if due {
        let today = day_of(now_unix(), IST_OFFSET);
        let nifty_kinds = [KIND_STOCK, KIND_NEWS, KIND_MINE];
        let small_kinds = [DIVISIONS[0].kind, DIVISIONS[1].kind, DIVISIONS[2].kind, KIND_GROWTH];
        let (nifty_pending, small_pending, funds) = {
            let conn = lock(&state)?;
            let mut a = Vec::new();
            for k in nifty_kinds {
                a.extend(pick_journal::pending(&conn, k, today)?);
            }
            let mut b = Vec::new();
            for k in small_kinds {
                b.extend(pick_journal::pending(&conn, k, today)?);
            }
            (a, b, pick_journal::pending(&conn, KIND_FUND, today)?)
        };
        let dedupe = |v: Vec<(String, i64)>| {
            let mut seen = HashSet::new();
            v.into_iter().filter(|(s, _)| seen.insert(s.clone())).collect::<Vec<_>>()
        };
        let (nifty_rows, nifty) = price_pending(&dedupe(nifty_pending), "^NSEI");
        let (small_rows, smallcap) = price_pending(&dedupe(small_pending), growth_picks::BENCH_SYMBOL);
        let mut navs = HashMap::new();
        for chunk in funds.chunks(6) {
            let batch: Vec<_> = std::thread::scope(|scope| {
                let handles: Vec<_> = chunk
                    .iter()
                    // A week's margin so the NAV on the suggestion day itself is
                    // included even when that day had none published.
                    .map(|(code, day)| scope.spawn(move || (code, crate::assistant::mf_connector::nav_history(code, day - 7))))
                    .collect();
                handles.into_iter().filter_map(|h| h.join().ok()).collect()
            });
            for (code, result) in batch {
                match result {
                    Ok(h) => {
                        navs.insert(code.clone(), h);
                    }
                    Err(e) => log::warn!("pick journal: NAV history for {code}: {e}"),
                }
            }
        }

        let conn = lock(&state)?;
        for k in nifty_kinds {
            settle(&conn, k, &nifty_rows, nifty.as_ref());
        }
        for k in small_kinds {
            settle(&conn, k, &small_rows, smallcap.as_ref());
        }
        pick_journal::settle_funds(&conn, &navs)?;
        *LAST_SETTLE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }

    let conn = lock(&state)?;
    let mut kinds = Vec::new();
    let (_, l) = pick_journal::learned_weights(&conn, KIND_STOCK, &stock_picks::FACTORS, &stock_picks::BASE_WEIGHTS)?;
    kinds.push(pick_journal::kind_record(&conn, KIND_STOCK, "Nifty 50", Some(l))?);
    for d in &DIVISIONS {
        let (_, l) = pick_journal::learned_weights(&conn, d.kind, d.factors, d.base)?;
        kinds.push(pick_journal::kind_record(&conn, d.kind, growth_picks::BENCH_NAME, Some(l))?);
    }
    kinds.push(pick_journal::kind_record(&conn, KIND_FUND, "the average small-cap fund", None)?);
    kinds.push(pick_journal::kind_record(&conn, KIND_NEWS, "Nifty 50", None)?);
    kinds.push(pick_journal::kind_record(&conn, KIND_MINE, "Nifty 50", None)?);
    // The single growth list the divisions replaced, while it has history.
    let legacy = pick_journal::kind_record(&conn, KIND_GROWTH, growth_picks::BENCH_NAME, None)?;
    if legacy.days > 0 {
        kinds.push(legacy);
    }
    Ok(TrackRecord { horizons: pick_journal::HORIZONS.to_vec(), kinds })
}
