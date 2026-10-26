//! Spending commands: connect the bank SMS Sheet, sync it, and read, add,
//! correct and budget expenses.
//!
//! The database lock is never held across a network call: a sync reads what
//! it needs, releases the lock, fetches, then takes it again to write.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{put_setting, setting};
use crate::assistant::bank_sms;
use crate::assistant::spend_lock;
use crate::assistant::http::{forget_keychain_secret, keychain_secret, store_keychain_secret};
use crate::assistant::spend::{
    self, guess_category, guess_kind, kind_for_category, payee_key, read_sms, Expense, Kind,
    MonthView, Outcome, Reading,
};
use crate::db::DbState;

const KEYCHAIN_SERVICE: &str = "nexus-bank-sms";
const KEYCHAIN_ACCOUNT: &str = "key";
const URL_KEY: &str = "bank_sms_url";
const CURSOR_KEY: &str = "bank_sms_cursor";
const LAST_SYNC_KEY: &str = "bank_sms_last_sync";
const LAST_ERROR_KEY: &str = "bank_sms_last_error";
const USUAL_INCOME_KEY: &str = "spend_usual_income_paise";
/// Where the link was saved during setup, picked up once if nothing is
/// connected yet.
const SETUP_FILE: &str = ".nexus-bank-link";

/// One sync at a time: the background timer and the Sync button share it.
static SYNCING: Mutex<()> = Mutex::new(());

fn lock(state: &DbState) -> Result<std::sync::MutexGuard<'_, rusqlite::Connection>, String> {
    state.0.lock().map_err(|e| format!("Lock error: {e}"))
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn today(conn: &rusqlite::Connection) -> String {
    conn.query_row("SELECT date('now','localtime')", [], |r| r.get(0))
        .unwrap_or_default()
}

fn local_date(conn: &rusqlite::Connection, unix: i64) -> String {
    conn.query_row("SELECT date(?1,'unixepoch','localtime')", [unix], |r| r.get(0))
        .unwrap_or_default()
}

// -- Lock ---------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockState {
    pub open: bool,
    /// "touchId" or "password": what the unlock button should say.
    pub method: &'static str,
}

#[tauri::command(async)]
pub fn nexus_spend_lock_state() -> LockState {
    LockState { open: spend_lock::is_open(), method: spend_lock::method() }
}

/// Show the Touch ID prompt; waits for the answer.
#[tauri::command(async)]
pub fn nexus_spend_unlock() -> Result<(), String> {
    spend_lock::unlock()
}

#[tauri::command(async)]
pub fn nexus_spend_lock() {
    spend_lock::lock();
}

// -- Connection ---------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendStatus {
    pub connected: bool,
    pub last_sync: Option<i64>,
    pub last_error: Option<String>,
    pub review: i64,
}

fn status(conn: &rusqlite::Connection) -> SpendStatus {
    let connected = setting(conn, URL_KEY).is_some()
        && keychain_secret(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).is_some();
    SpendStatus {
        connected,
        last_sync: setting(conn, LAST_SYNC_KEY).and_then(|v| v.parse().ok()),
        last_error: setting(conn, LAST_ERROR_KEY),
        review: conn
            .query_row("SELECT COUNT(*) FROM sms_inbox WHERE status = 'review'", [], |r| r.get(0))
            .unwrap_or(0),
    }
}

fn connect(conn: &rusqlite::Connection, link: &str) -> Result<(), String> {
    let (base, key) = bank_sms::split_link(link)?;
    store_keychain_secret(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT, &key)?;
    // A different deployment has its own row numbers.
    if setting(conn, URL_KEY).as_deref() != Some(base.as_str()) {
        put_setting(conn, CURSOR_KEY, "1")?;
    }
    put_setting(conn, URL_KEY, &base)?;
    put_setting(conn, LAST_ERROR_KEY, "")?;
    Ok(())
}

/// The setup file, if it holds a valid link. Read once, when nothing is
/// connected yet; NEXUS never writes to it.
fn link_from_setup_file() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let raw = std::fs::read_to_string(std::path::Path::new(&home).join(SETUP_FILE)).ok()?;
    bank_sms::split_link(&raw).ok().map(|_| raw.trim().to_string())
}

#[tauri::command(async)]
pub fn nexus_spend_status(state: State<'_, DbState>) -> Result<SpendStatus, String> {
    let conn = lock(&state)?;
    let mut current = status(&conn);
    if !current.connected && setting(&conn, URL_KEY).is_none() {
        if let Some(link) = link_from_setup_file() {
            if connect(&conn, &link).is_ok() {
                current = status(&conn);
            }
        }
    }
    Ok(current)
}

#[tauri::command(async)]
pub fn nexus_spend_connect(state: State<'_, DbState>, link: String) -> Result<SpendStatus, String> {
    spend_lock::check()?;
    let conn = lock(&state)?;
    connect(&conn, &link)?;
    Ok(status(&conn))
}

#[tauri::command(async)]
pub fn nexus_spend_disconnect(state: State<'_, DbState>) -> Result<SpendStatus, String> {
    spend_lock::check()?;
    let conn = lock(&state)?;
    let _ = forget_keychain_secret(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT);
    put_setting(&conn, URL_KEY, "")?;
    Ok(status(&conn))
}

// -- Sync ---------------------------------------------------------------------

#[derive(Serialize, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub added: u32,
    pub review: u32,
    pub ignored: u32,
}

/// Fetch new rows from the Sheet and book them. The error, if any, is also
/// remembered so the tab can show why the last automatic sync failed.
pub fn sync_once(state: &DbState) -> Result<SyncReport, String> {
    let _one_at_a_time = SYNCING.lock().unwrap_or_else(|e| e.into_inner());
    let (base, cursor) = {
        let conn = lock(state)?;
        if setting(&conn, URL_KEY).is_none() {
            if let Some(link) = link_from_setup_file() {
                let _ = connect(&conn, &link);
            }
        }
        let base = setting(&conn, URL_KEY).ok_or("Bank SMS is not connected yet.")?;
        let cursor: i64 = setting(&conn, CURSOR_KEY).and_then(|v| v.parse().ok()).unwrap_or(1);
        (base, cursor)
    };
    let key = keychain_secret(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
        .ok_or("The bank SMS key is missing from the Keychain. Connect the link again.")?;

    let fetched = bank_sms::fetch(&base, &key, cursor);

    let conn = lock(state)?;
    let (last, rows) = match fetched {
        Ok(page) => page,
        Err(why) => {
            let _ = put_setting(&conn, LAST_ERROR_KEY, &why);
            return Err(why);
        }
    };
    let mut report = SyncReport::default();
    for row in &rows {
        book_row(&conn, row, &mut report)?;
    }
    put_setting(&conn, CURSOR_KEY, &last.max(cursor).to_string())?;
    put_setting(&conn, LAST_SYNC_KEY, &now_unix().to_string())?;
    put_setting(&conn, LAST_ERROR_KEY, "")?;
    Ok(report)
}

fn book_row(
    conn: &rusqlite::Connection,
    row: &bank_sms::Row,
    report: &mut SyncReport,
) -> Result<(), String> {
    let received = row.received.map(|ms| ms / 1000);
    let fresh = conn
        .execute(
            "INSERT OR IGNORE INTO sms_inbox (sms_id, sender, text, received_at, status)
             VALUES (?1, ?2, ?3, ?4, 'new')",
            rusqlite::params![row.id, row.from, row.text, received],
        )
        .map_err(|e| e.to_string())?;
    if fresh == 0 {
        return Ok(());
    }
    let mark = |status: &str, reason: &str, suggestion: &str| {
        conn.execute(
            "UPDATE sms_inbox SET status = ?2, reason = ?3, suggestion = ?4 WHERE sms_id = ?1",
            rusqlite::params![row.id, status, reason, suggestion],
        )
        .map_err(|e| e.to_string())
    };
    let received_on = received.map(|t| local_date(conn, t)).unwrap_or_else(|| today(conn));
    if let (Some(balance), Some(at)) = (spend::stated_balance(&row.text), received) {
        record_balance(conn, &spend::account_of(&row.from, &row.text), balance, at, "sms")?;
    }
    match read_sms(&row.from, &row.text, &received_on) {
        Outcome::Booked(reading) => {
            insert_from_reading(conn, &row.id, &row.text, received, &reading, &received_on)?;
            mark("done", "", "")?;
            report.added += 1;
        }
        Outcome::Review { reading, reason } => {
            let suggestion = serde_json::to_string(&reading).unwrap_or_default();
            mark("review", &reason, &suggestion)?;
            report.review += 1;
        }
        Outcome::Ignored { reason } => {
            mark("ignored", &reason, "")?;
            report.ignored += 1;
        }
    }
    Ok(())
}

struct Classified {
    category: String,
    kind: Kind,
    /// The user's name for this payee, if they gave one.
    display: Option<String>,
}

/// Keep the newest known balance per account; an older message arriving late
/// never overwrites a newer balance.
fn record_balance(
    conn: &rusqlite::Connection,
    account: &str,
    balance_paise: i64,
    as_of: i64,
    source: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO account_balances (account, balance_paise, as_of, source) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(account) DO UPDATE SET balance_paise = ?2, as_of = ?3, source = ?4
          WHERE ?3 >= account_balances.as_of",
        rusqlite::params![account, balance_paise, as_of, source],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Every account's balance carried forward to now.
fn balances(conn: &rusqlite::Connection) -> Result<Vec<spend::Balance>, String> {
    let mut stmt = conn
        .prepare("SELECT account, balance_paise, as_of, source FROM account_balances ORDER BY account")
        .map_err(|e| e.to_string())?;
    let known: Vec<(String, i64, i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    let mut out = Vec::new();
    for (account, paise, as_of, source) in known {
        // Payments booked after the balance was stated, matched by when the
        // SMS arrived. Hand-added ones have no time and are left out rather
        // than guessed at.
        let mut since = conn
            .prepare("SELECT kind, amount_paise FROM expenses WHERE account = ?1 AND occurred_at > ?2")
            .map_err(|e| e.to_string())?;
        let moves: Vec<(Kind, i64)> = since
            .query_map(rusqlite::params![account, as_of], |r| {
                Ok((Kind::parse(&r.get::<_, String>(0)?).unwrap_or(Kind::Spend), r.get(1)?))
            })
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        out.push(spend::Balance {
            balance_paise: spend::carried_balance(paise, &moves),
            adjusted: moves.len() as u32,
            account,
            as_of,
            source,
        });
    }
    Ok(out)
}

/// Set an account's balance as it stands now (from the bank app, say).
#[tauri::command(async)]
pub fn nexus_spend_set_balance(state: State<'_, DbState>, account: String, amount: f64) -> Result<(), String> {
    spend_lock::check()?;
    let account = account.trim();
    if account.is_empty() || !amount.is_finite() || amount.abs() > 1_000_000_000.0 {
        return Err("Enter the account and its balance.".to_string());
    }
    let conn = lock(&state)?;
    record_balance(&conn, account, (amount * 100.0).round() as i64, now_unix(), "manual")
}

/// Category, kind and name for a payee as the bank names it: what the user
/// taught NEXUS first, else a guess from the name and the message.
fn classify(
    conn: &rusqlite::Connection,
    payee: &str,
    debit: bool,
    card: bool,
    text: &str,
) -> Classified {
    let learned: Option<(String, String, String)> = conn
        .query_row(
            "SELECT category, kind, display FROM payee_rules WHERE payee_key = ?1",
            [payee_key(payee)],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    if let Some((category, kind, display)) = learned {
        if let Some(kind) = Kind::parse(&kind) {
            return Classified { category, kind, display: Some(display).filter(|d| !d.is_empty()) };
        }
    }
    let category = guess_category(payee).to_string();
    let kind = kind_for_category(&category, guess_kind(debit, card, payee, text));
    Classified { category, kind, display: None }
}

fn insert_from_reading(
    conn: &rusqlite::Connection,
    sms_id: &str,
    text: &str,
    received: Option<i64>,
    reading: &Reading,
    fallback_date: &str,
) -> Result<i64, String> {
    let bank_payee = reading.payee.clone().unwrap_or_else(|| "Unknown".to_string());
    let debit = reading.debit.unwrap_or(true);
    let known = classify(conn, &bank_payee, debit, reading.card, text);
    insert(
        conn,
        &NewExpense {
            source: "sms",
            sms_id: Some(sms_id),
            occurred_on: reading.date.clone().unwrap_or_else(|| fallback_date.to_string()),
            occurred_at: received,
            amount_paise: reading.amount_paise.unwrap_or(0),
            kind: known.kind,
            payee_key: payee_key(&bank_payee),
            payee: known.display.unwrap_or(bank_payee),
            category: known.category,
            account: reading.account.clone(),
            channel: reading.channel.clone(),
            note: String::new(),
            raw: text.to_string(),
        },
    )
}

struct NewExpense<'a> {
    source: &'a str,
    sms_id: Option<&'a str>,
    occurred_on: String,
    occurred_at: Option<i64>,
    amount_paise: i64,
    kind: Kind,
    payee: String,
    /// The bank's name for the payee, which rules match on.
    payee_key: String,
    category: String,
    account: String,
    channel: String,
    note: String,
    raw: String,
}

fn insert(conn: &rusqlite::Connection, e: &NewExpense<'_>) -> Result<i64, String> {
    if e.amount_paise <= 0 {
        return Err("The amount must be more than zero.".to_string());
    }
    conn.execute(
        "INSERT OR IGNORE INTO expenses
           (source, sms_id, occurred_on, occurred_at, amount_paise, kind, payee, payee_key,
            category, account, channel, note, raw)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            e.source,
            e.sms_id,
            e.occurred_on,
            e.occurred_at,
            e.amount_paise,
            e.kind.as_str(),
            e.payee,
            e.payee_key,
            e.category,
            e.account,
            e.channel,
            e.note,
            e.raw
        ],
    )
    .map_err(|err| format!("Could not save the expense: {err}"))?;
    Ok(conn.last_insert_rowid())
}

#[tauri::command(async)]
pub fn nexus_spend_sync(state: State<'_, DbState>) -> Result<SyncReport, String> {
    spend_lock::check()?;
    sync_once(&state)
}

/// Book bank messages pasted by hand: ones from before the phone forwarded
/// anything, or that it missed. Same reading, same review rules; a message
/// pasted twice is booked once. Several can be pasted together, separated by
/// a blank line.
#[tauri::command(async)]
pub fn nexus_spend_paste(state: State<'_, DbState>, text: String) -> Result<SyncReport, String> {
    spend_lock::check()?;
    let conn = lock(&state)?;
    let mut report = SyncReport::default();
    let now = now_unix();
    for message in split_pasted(&text) {
        // The phone may already have forwarded this exact message.
        let seen: i64 = conn
            .query_row("SELECT COUNT(*) FROM sms_inbox WHERE trim(text) = ?1", [&message], |r| r.get(0))
            .unwrap_or(0);
        if seen > 0 {
            continue;
        }
        let sender = pasted_sender(&message);
        let row = bank_sms::Row {
            row: 0,
            received: Some(now * 1000),
            from: sender.to_string(),
            id: paste_id(&message),
            text: message,
        };
        book_row(&conn, &row, &mut report)?;
    }
    if report == SyncReport::default() {
        return Err("Nothing new in that text. Paste the whole bank message.".to_string());
    }
    Ok(report)
}

/// A stable id for a pasted message from all of its text (FNV-1a), so the
/// same message pasted twice is one entry and two different ones never collide.
fn paste_id(message: &str) -> String {
    let hash = message
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    format!("paste-{hash:016x}")
}

/// Messages split on blank lines; HDFC alerts span several lines each.
fn split_pasted(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .split("\n\n")
        .map(|m| m.trim().to_string())
        .filter(|m| m.chars().count() >= 10)
        .take(50)
        .collect()
}

/// A pasted message has no sender, so the bank is taken from its wording.
fn pasted_sender(message: &str) -> &'static str {
    let upper = message.to_uppercase();
    if upper.contains("HDFC") {
        "PASTED-HDFC"
    } else if upper.contains("ICICI") {
        "PASTED-ICICI"
    } else {
        "PASTED"
    }
}

// -- The month ----------------------------------------------------------------

fn expenses_between(
    conn: &rusqlite::Connection,
    from: &str,
    to: &str,
) -> Result<Vec<Expense>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, source, occurred_on, occurred_at, amount_paise, kind, payee, category,
                    account, channel, note
               FROM expenses WHERE occurred_on BETWEEN ?1 AND ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([from, to], |r| {
            Ok(Expense {
                id: r.get(0)?,
                source: r.get(1)?,
                occurred_on: r.get(2)?,
                occurred_at: r.get(3)?,
                amount_paise: r.get(4)?,
                kind: Kind::parse(&r.get::<_, String>(5)?).unwrap_or(Kind::Spend),
                payee: r.get(6)?,
                category: r.get(7)?,
                account: r.get(8)?,
                channel: r.get(9)?,
                note: r.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

fn month_bounds((y, m): (i64, u32), last_day: u32) -> (String, String) {
    (format!("{y:04}-{m:02}-01"), format!("{y:04}-{m:02}-{last_day:02}"))
}

#[tauri::command(async)]
pub fn nexus_spend_month(state: State<'_, DbState>, month: Option<String>) -> Result<MonthView, String> {
    spend_lock::check()?;
    let conn = lock(&state)?;
    let today = today(&conn);
    let ym = month
        .as_deref()
        .and_then(spend::parse_month)
        .or_else(|| spend::parse_month(today.get(..7).unwrap_or("")))
        .ok_or("Pick a month like 2026-10.")?;

    let (from, to) = month_bounds(ym, spend::days_in_month(ym.0, ym.1));
    let rows = expenses_between(&conn, &from, &to)?;

    // "vs last month": the same stretch of days, so the 24th compares with
    // last month's 1st to 24th, not with all of last month.
    let prev = spend::shift_month(ym, -1);
    let prev_dim = spend::days_in_month(prev.0, prev.1);
    let current = today.get(..7) == Some(from.get(..7).unwrap_or(""));
    let through = if current {
        today.get(8..10).and_then(|d| d.parse::<u32>().ok()).unwrap_or(prev_dim).min(prev_dim)
    } else {
        prev_dim
    };
    let (pf, pt) = month_bounds(prev, through);
    let previous: i64 = expenses_between(&conn, &pf, &pt)?
        .iter()
        .filter(|e| e.kind == Kind::Spend)
        .map(|e| e.amount_paise)
        .sum();

    let mut budgets = HashMap::new();
    let mut stmt = conn
        .prepare("SELECT category, monthly_paise FROM spend_budgets")
        .map_err(|e| e.to_string())?;
    for row in stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| e.to_string())?
        .flatten()
    {
        budgets.insert(row.0, row.1);
    }

    let usual: Option<i64> = setting(&conn, USUAL_INCOME_KEY).and_then(|v| v.parse().ok());
    let history = savings_history(&conn, ym, usual)?;
    let mut view = spend::summarize(ym, &today, rows, &budgets, previous, usual, history);
    view.balances = balances(&conn)?;
    Ok(view)
}

/// Six months of savings ending with `ym`, oldest first, from one grouped query.
fn savings_history(
    conn: &rusqlite::Connection,
    ym: (i64, u32),
    usual: Option<i64>,
) -> Result<Vec<spend::MonthSavings>, String> {
    let first = spend::shift_month(ym, -5);
    let from = format!("{:04}-{:02}-01", first.0, first.1);
    let to = format!("{:04}-{:02}-31", ym.0, ym.1);
    // (month, kind) -> (sum, largest single amount)
    let mut totals: HashMap<(String, String), (i64, i64)> = HashMap::new();
    let mut stmt = conn
        .prepare(
            "SELECT substr(occurred_on, 1, 7), kind, SUM(amount_paise), MAX(amount_paise) FROM expenses
              WHERE occurred_on BETWEEN ?1 AND ?2 GROUP BY 1, 2",
        )
        .map_err(|e| e.to_string())?;
    for row in stmt
        .query_map([&from, &to], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))
        })
        .map_err(|e| e.to_string())?
        .flatten()
    {
        totals.insert((row.0, row.1), (row.2, row.3));
    }
    Ok((0..6)
        .map(|i| {
            let (y, m) = spend::shift_month(first, i);
            let month = format!("{y:04}-{m:02}");
            let get = |kind: &str| totals.get(&(month.clone(), kind.to_string())).copied().unwrap_or((0, 0));
            let spend_paise = get("spend").0;
            let (income, largest) = get("income");
            spend::MonthSavings {
                savings: spend::savings(income, largest, get("refund").0, spend_paise, usual),
                spend_paise,
                month,
            }
        })
        .collect())
}

/// Set (or clear) the usual monthly income, used for months with no income
/// recorded.
#[tauri::command(async)]
pub fn nexus_spend_set_income(state: State<'_, DbState>, amount: Option<f64>) -> Result<(), String> {
    spend_lock::check()?;
    let conn = lock(&state)?;
    match amount.filter(|a| a.is_finite() && *a > 0.0 && *a < 100_000_000.0) {
        Some(a) => put_setting(&conn, USUAL_INCOME_KEY, &((a * 100.0).round() as i64).to_string()),
        None => put_setting(&conn, USUAL_INCOME_KEY, ""),
    }
}

// -- Adding and correcting ------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpenseInput {
    pub date: String,
    /// Rupees, as typed.
    pub amount: f64,
    pub payee: String,
    pub category: String,
    pub account: String,
    pub kind: String,
    #[serde(default)]
    pub note: String,
}

struct Checked {
    date: String,
    paise: i64,
    payee: String,
    category: String,
    account: String,
    kind: Kind,
    note: String,
}

fn check(input: &ExpenseInput) -> Result<Checked, String> {
    if !spend::valid_date(&input.date) {
        return Err("Pick a date.".to_string());
    }
    if !input.amount.is_finite() || input.amount <= 0.0 || input.amount > 10_000_000.0 {
        return Err("Enter an amount above zero.".to_string());
    }
    let payee = input.payee.trim();
    if payee.is_empty() {
        return Err("Say where the money went.".to_string());
    }
    if !spend::valid_category(&input.category) {
        return Err("Pick a category from the list.".to_string());
    }
    let kind = Kind::parse(&input.kind).ok_or("Pick spending, income, refund or transfer.")?;
    let account = input.account.trim();
    Ok(Checked {
        date: input.date.clone(),
        paise: (input.amount * 100.0).round() as i64,
        payee: payee.chars().take(80).collect(),
        category: input.category.clone(),
        account: if account.is_empty() { "Cash".to_string() } else { account.chars().take(40).collect() },
        kind,
        note: input.note.trim().chars().take(200).collect(),
    })
}

/// Teach NEXUS a payee: its name, category and kind, keyed on the bank's
/// name so it keeps matching new messages, and applied to that payee's other
/// entries so one correction fixes the whole history.
fn remember(
    conn: &rusqlite::Connection,
    key: &str,
    display: &str,
    category: &str,
    kind: Kind,
) -> Result<(), String> {
    if key.is_empty() {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO payee_rules (payee_key, category, kind, display) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(payee_key) DO UPDATE SET category = ?2, kind = ?3, display = ?4",
        rusqlite::params![key, category, kind.as_str(), display],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE expenses SET payee = ?2, category = ?3, kind = ?4 WHERE payee_key = ?1",
        rusqlite::params![key, display, category, kind.as_str()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn nexus_spend_add(state: State<'_, DbState>, input: ExpenseInput) -> Result<i64, String> {
    spend_lock::check()?;
    let c = check(&input)?;
    let conn = lock(&state)?;
    insert(
        &conn,
        &NewExpense {
            source: "manual",
            sms_id: None,
            occurred_on: c.date,
            occurred_at: None,
            amount_paise: c.paise,
            kind: c.kind,
            payee_key: payee_key(&c.payee),
            payee: c.payee,
            category: c.category,
            account: c.account,
            channel: String::new(),
            note: c.note,
            raw: String::new(),
        },
    )
}

#[tauri::command(async)]
pub fn nexus_spend_update(
    state: State<'_, DbState>,
    id: i64,
    input: ExpenseInput,
    remember_payee: bool,
) -> Result<(), String> {
    spend_lock::check()?;
    let c = check(&input)?;
    let conn = lock(&state)?;
    // The key stays the bank's name for the payee, whatever it is renamed to,
    // so a remembered rule keeps matching that payee's future messages.
    let key: String = conn
        .query_row("SELECT payee_key FROM expenses WHERE id = ?1", [id], |r| r.get(0))
        .map_err(|_| "That expense no longer exists.".to_string())?;
    conn.execute(
        "UPDATE expenses
            SET occurred_on = ?2, amount_paise = ?3, kind = ?4, payee = ?5,
                category = ?6, account = ?7, note = ?8
          WHERE id = ?1",
        rusqlite::params![id, c.date, c.paise, c.kind.as_str(), c.payee, c.category, c.account, c.note],
    )
    .map_err(|e| e.to_string())?;
    if remember_payee {
        remember(&conn, &key, &c.payee, &c.category, c.kind)?;
    }
    Ok(())
}

#[tauri::command(async)]
pub fn nexus_spend_delete(state: State<'_, DbState>, id: i64) -> Result<(), String> {
    spend_lock::check()?;
    let conn = lock(&state)?;
    conn.execute("DELETE FROM expenses WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command(async)]
pub fn nexus_spend_set_budget(
    state: State<'_, DbState>,
    category: String,
    amount: Option<f64>,
) -> Result<(), String> {
    spend_lock::check()?;
    if !spend::valid_category(&category) {
        return Err("Pick a category from the list.".to_string());
    }
    let conn = lock(&state)?;
    match amount.filter(|a| a.is_finite() && *a > 0.0) {
        Some(a) => conn.execute(
            "INSERT INTO spend_budgets (category, monthly_paise) VALUES (?1, ?2)
             ON CONFLICT(category) DO UPDATE SET monthly_paise = ?2",
            rusqlite::params![category, (a * 100.0).round() as i64],
        ),
        None => conn.execute("DELETE FROM spend_budgets WHERE category = ?1", [category]),
    }
    .map_err(|e| e.to_string())?;
    Ok(())
}

// -- Messages that need a look ---------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewItem {
    pub sms_id: String,
    pub sender: String,
    pub text: String,
    pub received_at: Option<i64>,
    pub reason: String,
    pub suggestion: Option<Reading>,
    /// The suggestion's category and kind, as booking it would set them.
    pub category: String,
    pub kind: Kind,
}

#[tauri::command(async)]
pub fn nexus_spend_review(state: State<'_, DbState>) -> Result<Vec<ReviewItem>, String> {
    spend_lock::check()?;
    let conn = lock(&state)?;
    let mut stmt = conn
        .prepare(
            "SELECT sms_id, sender, text, received_at, reason FROM sms_inbox
              WHERE status = 'review' ORDER BY received_at DESC LIMIT 100",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String, String, Option<i64>, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows
        .into_iter()
        .map(|(sms_id, sender, text, received_at, reason)| {
            // Re-read rather than trusting the stored suggestion, so a reader
            // improved since the message arrived is used.
            let received_on = received_at.map(|t| local_date(&conn, t)).unwrap_or_else(|| today(&conn));
            let suggestion = match read_sms(&sender, &text, &received_on) {
                Outcome::Booked(r) | Outcome::Review { reading: r, .. } => Some(r),
                Outcome::Ignored { .. } => None,
            };
            let (category, kind, display) = match &suggestion {
                Some(r) => {
                    let k = classify(&conn, r.payee.as_deref().unwrap_or(""), r.debit.unwrap_or(true), r.card, &text);
                    (k.category, k.kind, k.display)
                }
                None => (spend::UNCATEGORISED.to_string(), Kind::Spend, None),
            };
            let suggestion = suggestion.map(|mut r| {
                if display.is_some() {
                    r.payee = display;
                }
                r
            });
            ReviewItem { sms_id, sender, text, received_at, reason, suggestion, category, kind }
        })
        .collect())
}

#[tauri::command(async)]
pub fn nexus_spend_accept(
    state: State<'_, DbState>,
    sms_id: String,
    input: ExpenseInput,
    remember_payee: bool,
) -> Result<i64, String> {
    spend_lock::check()?;
    let c = check(&input)?;
    let conn = lock(&state)?;
    let (text, received): (String, Option<i64>) = conn
        .query_row(
            "SELECT text, received_at FROM sms_inbox WHERE sms_id = ?1 AND status = 'review'",
            [&sms_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "That message has already been handled.".to_string())?;
    let (channel, bank_payee) = match read_sms("", &text, &c.date) {
        Outcome::Booked(r) | Outcome::Review { reading: r, .. } => (r.channel, r.payee),
        Outcome::Ignored { .. } => (String::new(), None),
    };
    let key = payee_key(bank_payee.as_deref().unwrap_or(&c.payee));
    let id = insert(
        &conn,
        &NewExpense {
            source: "sms",
            sms_id: Some(&sms_id),
            occurred_on: c.date,
            occurred_at: received,
            amount_paise: c.paise,
            kind: c.kind,
            payee_key: key.clone(),
            payee: c.payee.clone(),
            category: c.category.clone(),
            account: c.account,
            channel,
            note: c.note,
            raw: text,
        },
    )?;
    conn.execute("UPDATE sms_inbox SET status = 'done' WHERE sms_id = ?1", [&sms_id])
        .map_err(|e| e.to_string())?;
    if remember_payee {
        remember(&conn, &key, &c.payee, &c.category, c.kind)?;
    }
    Ok(id)
}

#[tauri::command(async)]
pub fn nexus_spend_ignore(state: State<'_, DbState>, sms_id: String) -> Result<(), String> {
    spend_lock::check()?;
    let conn = lock(&state)?;
    conn.execute(
        "UPDATE sms_inbox SET status = 'ignored', reason = 'Set aside by you' WHERE sms_id = ?1",
        [sms_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        for (_, sql) in crate::db::migrations::MIGRATIONS {
            conn.execute_batch(sql).unwrap();
        }
        conn
    }

    fn row(id: &str, text: &str) -> bank_sms::Row {
        bank_sms::Row {
            row: 2,
            received: Some(1_761_300_000_000),
            from: "AX-ICICIT-S".into(),
            text: text.into(),
            id: id.into(),
        }
    }

    const UPI: &str = "ICICI Bank Acct XX225 debited for Rs 1.00 on 24-Oct-26; godisalwaysgrea credited. UPI:123456789012. Call 18002662 for dispute.";

    #[test]
    fn a_message_is_booked_once_however_often_it_arrives() {
        let conn = db();
        let mut report = SyncReport::default();
        book_row(&conn, &row("a", UPI), &mut report).unwrap();
        book_row(&conn, &row("a", UPI), &mut report).unwrap();
        assert_eq!(report, SyncReport { added: 1, review: 0, ignored: 0 });
        let (paise, on, kind): (i64, String, String) = conn
            .query_row("SELECT amount_paise, occurred_on, kind FROM expenses", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!((paise, on.as_str(), kind.as_str()), (100, "2026-10-24", "spend"));
    }

    #[test]
    fn unreadable_and_non_payment_messages_are_kept_but_not_counted() {
        let conn = db();
        let mut report = SyncReport::default();
        book_row(&conn, &row("r", "Rs.450.00 spent on HDFC Bank Card x4321 at SWIGGY on 2026-10-24"), &mut report).unwrap();
        book_row(&conn, &row("i", "Rs 5,000 will be debited on 01-Nov-26"), &mut report).unwrap();
        assert_eq!(report, SyncReport { added: 0, review: 1, ignored: 1 });
        let expenses: i64 = conn.query_row("SELECT COUNT(*) FROM expenses", [], |r| r.get(0)).unwrap();
        assert_eq!(expenses, 0);
        let kept: i64 = conn.query_row("SELECT COUNT(*) FROM sms_inbox", [], |r| r.get(0)).unwrap();
        assert_eq!(kept, 2);
    }

    #[test]
    fn a_taught_payee_applies_to_history_and_future_messages() {
        let conn = db();
        let mut report = SyncReport::default();
        book_row(&conn, &row("a", UPI), &mut report).unwrap();
        remember(&conn, &payee_key("godisalwaysgrea"), "Mom", "Family", Kind::Transfer).unwrap();
        let first: (String, String, String) = conn
            .query_row("SELECT payee, category, kind FROM expenses", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!(first, ("Mom".into(), "Family".into(), "transfer".into()));

        // A renamed payee still matches the bank's name on the next message.
        book_row(&conn, &row("b", &UPI.replace("Rs 1.00", "Rs 50.00")), &mut report).unwrap();
        let later: (String, String, String) = conn
            .query_row(
                "SELECT payee, category, kind FROM expenses WHERE sms_id = 'b'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(later, ("Mom".into(), "Family".into(), "transfer".into()));
    }

    #[test]
    fn pasted_messages_are_read_like_forwarded_ones_and_only_once() {
        let pasted = "Txn Rs.925.00\nOn HDFC Bank Card 5131\nAt *u766@pty \nby UPI 131673777866\nOn 22-10\nNot You?\nCall 18002586161/SMS BLOCK CC 5131 to 7308080808\n\nICICI Bank Acct XX225 debited for Rs 1.00 on 24-Oct-26; godisalwaysgrea credited. UPI:123456789012.";
        let parts = split_pasted(pasted);
        assert_eq!(parts.len(), 2, "split on the blank line, multi-line HDFC kept whole");
        assert_eq!(pasted_sender(&parts[0]), "PASTED-HDFC");
        assert_eq!(pasted_sender(&parts[1]), "PASTED-ICICI");

        let conn = db();
        let mut report = SyncReport::default();
        for m in &parts {
            let row = bank_sms::Row { row: 0, received: Some(1_761_300_000_000), from: pasted_sender(m).into(), id: paste_id(m), text: m.clone() };
            book_row(&conn, &row, &mut report).unwrap();
            book_row(&conn, &row, &mut report).unwrap();
        }
        assert_eq!(report.added, 2, "both booked, each once");

        let same_day_same_amount = UPI.replace("godisalwaysgrea", "someoneelse");
        assert_ne!(paste_id(UPI), paste_id(&same_day_same_amount), "ids come from the whole message");
    }

    #[test]
    fn a_stated_balance_is_kept_and_carried_through_later_payments() {
        let conn = db();
        let mut report = SyncReport::default();
        let bill = "ICICI Bank Acc XX225 debited Rs. 2,295.99 on 23-Oct-26 InfoBIL*INFT*FJR0.Avl Bal Rs. 34,709.16.To dispute call 18002662";
        let mut first = row("bill", bill);
        first.received = Some(1_000_000_000_000); // milliseconds, as the Sheet sends
        book_row(&conn, &first, &mut report).unwrap();
        let mut later = row("upi", UPI);
        later.received = Some(1_000_000_600_000);
        book_row(&conn, &later, &mut report).unwrap();

        let b = balances(&conn).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].account, "ICICI XX225");
        assert_eq!(b[0].balance_paise, 3_470_916 - 100, "the later ₹1 UPI payment is taken off");
        assert_eq!(b[0].adjusted, 1, "the bill itself is already in the stated balance");

        // An older message arriving late does not replace a newer balance.
        record_balance(&conn, "ICICI XX225", 1, 999, "sms").unwrap();
        assert_eq!(balances(&conn).unwrap()[0].as_of, 1_000_000_000);
    }

    #[test]
    fn input_is_checked_before_saving() {
        let good = ExpenseInput {
            date: "2026-10-24".into(),
            amount: 99.995,
            payee: " Tea stall ".into(),
            category: "Food".into(),
            account: "".into(),
            kind: "spend".into(),
            note: String::new(),
        };
        let c = check(&good).unwrap();
        assert_eq!((c.paise, c.payee.as_str(), c.account.as_str()), (10_000, "Tea stall", "Cash"));
        assert!(check(&ExpenseInput { amount: 0.0, ..good_clone(&good) }).is_err());
        assert!(check(&ExpenseInput { category: "Snacks".into(), ..good_clone(&good) }).is_err());
        assert!(check(&ExpenseInput { date: "24/10/2026".into(), ..good_clone(&good) }).is_err());
    }

    fn good_clone(g: &ExpenseInput) -> ExpenseInput {
        ExpenseInput {
            date: g.date.clone(),
            amount: g.amount,
            payee: g.payee.clone(),
            category: g.category.clone(),
            account: g.account.clone(),
            kind: g.kind.clone(),
            note: g.note.clone(),
        }
    }
}
