//! The IPOs tab: researched open and upcoming issues, and the investor's own
//! allotments priced against the market.

use std::time::{Duration, Instant};

use tauri::State;

use super::{put_setting, setting};
use crate::assistant::ipo_desk::{self, Allotment, AllotmentRow, IpoDesk};
use crate::assistant::permission::Reach;
use crate::assistant::reasoning::{self, ContextCategory, Purpose};
use crate::assistant::stock_board::{day_of, IST_OFFSET};
use crate::db::DbState;

/// The last research, so reopening the tab does not search again.
const DESK_KEY: &str = "ipo_desk";
/// GMP moves through the day; three hours keeps it close to current
/// without searching on every visit.
const DESK_FOR: i64 = 3 * 3600;
/// Several searches and page reads per IPO.
const DESK_TIMEOUT: Duration = Duration::from_secs(300);
const ALLOTMENTS_KEY: &str = "ipo_allotments";

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn lock(state: &DbState) -> Result<std::sync::MutexGuard<'_, rusqlite::Connection>, String> {
    state.0.lock().map_err(|e| format!("Lock error: {e}"))
}

fn allotments(conn: &rusqlite::Connection) -> Vec<Allotment> {
    setting(conn, ALLOTMENTS_KEY)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_allotments(conn: &rusqlite::Connection, list: &[Allotment]) -> Result<(), String> {
    put_setting(conn, ALLOTMENTS_KEY, &serde_json::to_string(list).map_err(|e| e.to_string())?)
}

/// An NSE symbol as typed (`jioplatforms.ns`) in the form stored
/// (`JIOPLATFORMS`); empty is allowed, for an IPO that has not listed.
fn nse_symbol(raw: &str) -> Result<String, String> {
    let symbol = raw.trim().to_uppercase().trim_end_matches(".NS").to_string();
    if !symbol.is_empty() && crate::assistant::market_connector::valid_symbol(&symbol).is_none() {
        return Err(format!("\"{symbol}\" is not a valid NSE symbol."));
    }
    Ok(symbol)
}

/// Today's IPOs, from cache unless it is stale or `refresh` is set.
///
/// Only Claude can do this: it needs the web, and the local model has none.
#[tauri::command]
pub fn nexus_ipo_desk(state: State<'_, DbState>, refresh: Option<bool>) -> Result<IpoDesk, String> {
    let now = now_unix();
    let (cached, external_ok) = {
        let conn = lock(&state)?;
        let cached: Option<IpoDesk> = setting(&conn, DESK_KEY).and_then(|raw| serde_json::from_str(&raw).ok());
        (cached, reasoning::read_policy(&conn).external_reasoning_allowed)
    };
    if let Some(d) = cached.filter(|d| !refresh.unwrap_or(false) && now - d.generated_at < DESK_FOR) {
        return Ok(d);
    }
    if !external_ok {
        return Err("Researching IPOs needs Claude, and external reasoning is switched off in Settings.".to_string());
    }

    let today = ipo_desk::ymd(day_of(now, IST_OFFSET));
    let started = Instant::now();
    let reply = crate::assistant::claude_provider::research(&ipo_desk::prompt(&today), DESK_TIMEOUT)?;
    let elapsed = started.elapsed().as_millis();
    let desk = IpoDesk {
        ipos: ipo_desk::build(&reply)?,
        rule: ipo_desk::RULE.to_string(),
        provider: "Claude".to_string(),
        generated_at: now,
    };

    let conn = lock(&state)?;
    let _ = reasoning::record_use(
        &conn,
        crate::assistant::claude_provider::PROVIDER_ID,
        "claude-code",
        Reach::LeavesMachine,
        Purpose::Summarise,
        &[ContextCategory::Request],
        &format!("IPO research: {} issues", desk.ipos.len()),
        elapsed,
    );
    put_setting(&conn, DESK_KEY, &serde_json::to_string(&desk).map_err(|e| e.to_string())?)?;
    Ok(desk)
}

/// The investor's allotments, each priced if it has listed.
#[tauri::command]
pub fn nexus_ipo_allotments(state: State<'_, DbState>) -> Result<Vec<AllotmentRow>, String> {
    // Read, then release the lock before the quotes go over the network.
    let list = allotments(&*lock(&state)?);
    Ok(ipo_desk::price(list))
}

#[tauri::command]
pub fn nexus_add_ipo_allotment(
    state: State<'_, DbState>,
    name: String,
    symbol: String,
    shares: u32,
    issue_price: f64,
) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give the IPO a name.".to_string());
    }
    if shares == 0 || !issue_price.is_finite() || issue_price <= 0.0 {
        return Err("Shares and issue price must both be above zero.".to_string());
    }
    let symbol = nse_symbol(&symbol)?;
    let conn = lock(&state)?;
    let mut list = allotments(&conn);
    // The clock is the id, bumped past the newest so two adds in one second
    // stay distinct.
    let id = list.iter().map(|a| a.id + 1).max().unwrap_or(0).max(now_unix());
    list.push(Allotment { id, name, symbol, shares, issue_price });
    save_allotments(&conn, &list)
}

/// Set or change the NSE symbol, for an allotment added before it listed.
#[tauri::command]
pub fn nexus_set_ipo_symbol(state: State<'_, DbState>, id: i64, symbol: String) -> Result<(), String> {
    let symbol = nse_symbol(&symbol)?;
    let conn = lock(&state)?;
    let mut list = allotments(&conn);
    let item = list.iter_mut().find(|a| a.id == id).ok_or("That allotment no longer exists.")?;
    item.symbol = symbol;
    save_allotments(&conn, &list)
}

#[tauri::command]
pub fn nexus_remove_ipo_allotment(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    let conn = lock(&state)?;
    let mut list = allotments(&conn);
    list.retain(|a| a.id != id);
    save_allotments(&conn, &list)
}
