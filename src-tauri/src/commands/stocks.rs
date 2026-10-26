//! The Stocks section: the market board, stock suggestions, fund suggestions,
//! and the journal that checks how past suggestions turned out.

use std::collections::HashMap;

use tauri::State;

use super::{put_setting, setting};
use crate::assistant::fund_picks::{self, FlowHistory, FundPicks};
use crate::assistant::pick_journal::{self, NewEntry, TrackRecord, KIND_FUND, KIND_STOCK};
use crate::assistant::stock_board::{self, day_of, Board, IST_OFFSET};
use crate::assistant::stock_picks::{self, Picks};
use crate::db::DbState;

/// Daily AUM and NAV per fund, the history implied fund flows are read from.
const FLOW_KEY: &str = "fund_flow_history";

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Indices and the Nifty 50 with their returns. Cached for five minutes
/// unless `refresh`.
#[tauri::command(async)]
pub fn nexus_stock_board(refresh: Option<bool>) -> Result<Board, String> {
    stock_board::board(refresh.unwrap_or(false), now_unix())
}

/// The Nifty 50 scored with the journal's current weights. The day's scores
/// are recorded, and past ones settled, as a side effect; a journal failure
/// is logged rather than costing the screen its suggestions.
#[tauri::command(async)]
pub fn nexus_stock_picks(state: State<'_, DbState>, refresh: Option<bool>) -> Result<Picks, String> {
    let weights = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        pick_journal::learned_weights(&conn).map(|(w, _)| w).unwrap_or(stock_picks::BASE_WEIGHTS)
    };
    let (picks, board) = stock_picks::picks(refresh.unwrap_or(false), now_unix(), &weights)?;

    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    // Recorded under the date of the last session priced, so a weekend or
    // holiday visit files Friday's scores under Friday, once.
    let day = day_of(board.as_of, IST_OFFSET);
    if board.as_of > 0 && pick_journal::is_weekday(day) {
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
        if let Err(e) = pick_journal::record(&conn, KIND_STOCK, day, &entries) {
            log::warn!("pick journal: {e}");
        }
    }
    if let Err(e) = pick_journal::settle_stocks(&conn, &board) {
        log::warn!("pick journal: {e}");
    }
    Ok(picks)
}

/// Small-cap funds scored for the Suits-for-you tab. Each call records the
/// day's AUM snapshot and, on a weekday, the day's ranking; the lock is
/// released while the funds are fetched.
#[tauri::command(async)]
pub fn nexus_fund_picks(state: State<'_, DbState>, refresh: Option<bool>) -> Result<FundPicks, String> {
    let mut history: FlowHistory = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        setting(&conn, FLOW_KEY)
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    };
    let now = now_unix();
    let picks = fund_picks::picks(refresh.unwrap_or(false), now, &mut history)?;
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    put_setting(&conn, FLOW_KEY, &serde_json::to_string(&history).map_err(|e| e.to_string())?)?;

    let day = day_of(now, IST_OFFSET);
    if pick_journal::is_weekday(day) {
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
        if let Err(e) = pick_journal::record(&conn, KIND_FUND, day, &entries) {
            log::warn!("pick journal: {e}");
        }
    }
    Ok(picks)
}

/// How past suggestions turned out, and what the stock weights learned.
/// Fund outcomes are settled here first, from AMFI NAV history fetched off
/// the lock.
#[tauri::command(async)]
pub fn nexus_pick_track_record(state: State<'_, DbState>) -> Result<TrackRecord, String> {
    let today = day_of(now_unix(), IST_OFFSET);
    let pending = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        pick_journal::pending_funds(&conn, today)?
    };

    let mut navs = HashMap::new();
    for chunk in pending.chunks(6) {
        let batch: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                // A few days' margin so the NAV on the suggestion day itself is
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

    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    pick_journal::settle_funds(&conn, &navs)?;
    pick_journal::track_record(&conn)
}
