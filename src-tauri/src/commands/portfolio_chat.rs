//! The Portfolio chat box: read what the user typed, show it back, apply it
//! only once confirmed.

use tauri::State;

use crate::assistant::policy_plan;
use crate::assistant::portfolio_chat::{read, ulip_fund_name, Action, Reading, PNB_METLIFE_GROUP, TATA_AIA_GROUP};
use crate::assistant::ulip_switch::{self, day_of, History, SwitchReport};
use crate::db::DbState;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub actions: Vec<Action>,
    /// Each action in words, in the same order, for the user to confirm.
    pub lines: Vec<String>,
    pub not_understood: Vec<String>,
}

/// Read without changing anything.
#[tauri::command(async)]
pub fn nexus_portfolio_chat(text: String) -> Preview {
    let Reading { actions, not_understood } = read(&text);
    Preview { lines: actions.iter().map(Action::describe).collect(), actions, not_understood }
}

fn today(conn: &rusqlite::Connection) -> String {
    conn.query_row("SELECT date('now','localtime')", [], |r| r.get(0)).unwrap_or_default()
}

/// Update each ULIP holding's NAV when a newer one is published, PNB
/// MetLife's from the insurer and Tata AIA's from Morningstar, and record
/// every Tata AIA fund's daily NAV, since Tata AIA publishes no history of
/// its own. The lock is released while the pages are fetched. Returns how
/// many holdings changed.
pub fn refresh_ulip_navs(db: &DbState) -> Result<usize, String> {
    use crate::assistant::{ulip_nav, ulip_switch};
    let held: Vec<(i64, String, String, String)> = {
        let conn = db.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let mut stmt = conn
            .prepare("SELECT id, symbol, broker, COALESCE(price_as_of, '') FROM holdings WHERE kind = 'ulip'")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        rows
    };
    let fund_of = |code: &str, group: &str| ulip_fund_name(code, group).and_then(|name| ulip_switch::find(group, name));

    let mut updates = Vec::new();
    let now = now();
    for (id, code, _, as_of) in held.iter().filter(|(_, _, broker, _)| broker == PNB_METLIFE_GROUP) {
        let Some(fund) = fund_of(code, PNB_METLIFE_GROUP) else { continue };
        // One fund failing to price leaves its last NAV and the others go on.
        match ulip_switch::latest_pnb(fund.name, now) {
            Ok((nav, date)) if date.as_str() > as_of.as_str() => updates.push((*id, nav, date)),
            Ok(_) => {}
            Err(e) => log::warn!("{code} NAV: {e}"),
        }
    }

    let mut logged = Vec::new();
    if held.iter().any(|(_, _, broker, _)| broker == TATA_AIA_GROUP) {
        let navs = ulip_nav::fetch(ulip_nav::TATA_LIST)?;
        let priced = |fund: &ulip_switch::Fund| {
            let name = fund.morningstar?;
            navs.iter().find(|n| ulip_nav::same_name(&n.name, name))
        };
        for (id, code, _, as_of) in held.iter().filter(|(_, _, broker, _)| broker == TATA_AIA_GROUP) {
            if let Some(n) = fund_of(code, TATA_AIA_GROUP).and_then(priced) {
                if n.date.as_str() > as_of.as_str() {
                    updates.push((*id, n.nav, n.date.clone()));
                }
            }
        }
        for fund in ulip_switch::TATA_FUNDS {
            if let Some(n) = priced(fund) {
                logged.push((fund.label, n.date.clone(), n.nav));
            }
        }
    }

    let conn = db.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    for (id, nav, date) in &updates {
        conn.execute(
            "UPDATE holdings SET unit_price = ?2, price_as_of = ?3 WHERE id = ?1",
            rusqlite::params![id, nav, date],
        )
        .map_err(|e| e.to_string())?;
    }
    for (label, date, nav) in &logged {
        conn.execute(
            "INSERT OR IGNORE INTO ulip_nav_log (policy, fund, day, nav) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![TATA_AIA_GROUP, label, date, nav],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(updates.len())
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// A policy's recorded daily NAVs, by fund label, oldest first.
fn read_log(conn: &rusqlite::Connection, group: &str) -> Result<Vec<(String, History)>, String> {
    let mut stmt = conn
        .prepare("SELECT fund, day, nav FROM ulip_nav_log WHERE policy = ?1 ORDER BY fund, day")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([group], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, f64>(2)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok);
    let mut out: Vec<(String, History)> = Vec::new();
    for (fund, day, nav) in rows {
        let Some(day) = day_of(&day) else { continue };
        match out.last_mut() {
            Some((f, h)) if *f == fund => h.push((day, nav)),
            _ => out.push((fund, vec![(day, nav)])),
        }
    }
    Ok(out)
}

/// A policy's switch report for the funds held in it, with advice for its
/// terms once they are saved. The fetch takes a few seconds, so the lock is
/// released first.
#[tauri::command(async)]
pub fn nexus_ulip_switch_report(state: State<'_, DbState>, group: String, refresh: bool) -> Result<SwitchReport, String> {
    let (held, prices, logged, plan) = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        let mut stmt = conn
            .prepare("SELECT symbol, quantity, COALESCE(unit_price, 0), COALESCE(price_as_of, '') FROM holdings WHERE kind = 'ulip' AND broker = ?1")
            .map_err(|e| e.to_string())?;
        let rows: Vec<(String, f64, f64, String)> = stmt
            .query_map([&group], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?, r.get::<_, f64>(2)?, r.get::<_, String>(3)?))
            })
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .filter_map(|(code, units, price, as_of)| {
                let fund = ulip_fund_name(&code, &group).and_then(|name| ulip_switch::find(&group, name))?;
                Some((fund.label.to_string(), units, price, as_of))
            })
            .collect();
        let held: Vec<(String, f64)> = rows.iter().map(|r| (r.0.clone(), r.1)).collect();
        let prices: Vec<(String, f64, String)> =
            rows.into_iter().filter(|r| r.2 > 0.0 && !r.3.is_empty()).map(|r| (r.0, r.2, r.3)).collect();
        (held, prices, read_log(&conn, &group)?, read_plan(&conn, &group)?)
    };
    let now = now();
    let mut report = ulip_switch::build(&group, &held, &logged, &prices, refresh, now)?;
    if let Some(plan) = plan {
        let value: f64 = report.held.iter().map(|h| h.value).sum();
        report.plan = Some(policy_plan::advise(&plan, &report, value, india_day(now)));
    }
    Ok(report)
}

/// Today's day number in India, where the policies and their NAVs are.
fn india_day(now: i64) -> i64 {
    (now + 5 * 3600 + 1800).div_euclid(86_400)
}

fn read_plan(conn: &rusqlite::Connection, group: &str) -> Result<Option<policy_plan::Plan>, String> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT started, monthly_premium, pay_years, term_years, birth_year, retire_age FROM policy_plans WHERE name = ?1",
        [group],
        |r| {
            Ok(policy_plan::Plan {
                started: r.get(0)?,
                monthly_premium: r.get(1)?,
                pay_years: r.get(2)?,
                term_years: r.get(3)?,
                birth_year: r.get(4)?,
                retire_age: r.get(5)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Save a policy's terms and the holder's retirement.
#[tauri::command(async)]
pub fn nexus_policy_plan_save(state: State<'_, DbState>, group: String, plan: policy_plan::Plan) -> Result<(), String> {
    if ulip_switch::policy(&group).is_none() {
        return Err(format!("{group} has no fund report."));
    }
    plan.check(crate::assistant::mf_connector::civil_from_days(india_day(now())).0)?;
    let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    conn.execute(
        "INSERT INTO policy_plans (name, started, monthly_premium, pay_years, term_years, birth_year, retire_age)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(name) DO UPDATE SET started = excluded.started, monthly_premium = excluded.monthly_premium,
           pay_years = excluded.pay_years, term_years = excluded.term_years, birth_year = excluded.birth_year,
           retire_age = excluded.retire_age",
        rusqlite::params![group, plan.started, plan.monthly_premium, plan.pay_years, plan.term_years, plan.birth_year, plan.retire_age],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(serde::Serialize)]
pub struct FundHistory {
    pub label: String,
    /// (yyyy-mm-dd, NAV), oldest first.
    pub navs: Vec<(String, f64)>,
}

/// NAVs of a policy's named funds, for its page's charts.
#[tauri::command(async)]
pub fn nexus_ulip_fund_history(state: State<'_, DbState>, group: String, labels: Vec<String>) -> Result<Vec<FundHistory>, String> {
    let logged = {
        let conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
        read_log(&conn, &group)?
    };
    Ok(ulip_switch::histories_of(&group, &labels, &logged, now())
        .into_iter()
        .map(|(label, navs)| FundHistory { label, navs })
        .collect())
}

/// Apply confirmed actions in one transaction: all of them or none.
#[tauri::command(async)]
pub fn nexus_portfolio_chat_apply(state: State<'_, DbState>, actions: Vec<Action>) -> Result<Vec<String>, String> {
    let mut conn = state.0.lock().map_err(|e| format!("Lock error: {e}"))?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut done = Vec::new();
    for action in &actions {
        done.push(apply(&tx, action)?);
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(done)
}

fn apply(conn: &rusqlite::Connection, action: &Action) -> Result<String, String> {
    let err = |e: rusqlite::Error| e.to_string();
    match action {
        Action::Buy { symbol, units, broker, price } => {
            let broker = broker.clone().unwrap_or_default();
            let existing: Option<(i64, Option<f64>)> = conn
                .query_row(
                    "SELECT id, invested FROM holdings WHERE kind = 'stock' AND symbol = ?1 AND broker = ?2",
                    rusqlite::params![symbol, broker],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();
            let cost = price.map(|p| p * units);
            match existing {
                Some((id, invested)) => {
                    // A known total plus a known cost stays known; otherwise
                    // it becomes unknown rather than a partial figure.
                    let invested = match (invested, cost) {
                        (Some(i), Some(c)) => Some(i + c),
                        _ => None,
                    };
                    conn.execute(
                        "UPDATE holdings SET quantity = quantity + ?2, invested = ?3 WHERE id = ?1",
                        rusqlite::params![id, units, invested],
                    )
                    .map_err(err)?;
                }
                None => {
                    conn.execute(
                        "INSERT INTO holdings (symbol, label, quantity, kind, broker, invested)
                         VALUES (?1, '', ?2, 'stock', ?3, ?4)",
                        rusqlite::params![symbol, units, broker, cost],
                    )
                    .map_err(err)?;
                }
            }
        }
        Action::Sell { symbol, units } => {
            let (id, qty, invested): (i64, f64, Option<f64>) = conn
                .query_row(
                    "SELECT id, quantity, invested FROM holdings WHERE kind = 'stock' AND symbol = ?1 ORDER BY id LIMIT 1",
                    [symbol],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(|_| format!("You have no {symbol} to sell."))?;
            if *units >= qty - 1e-9 {
                conn.execute("DELETE FROM holdings WHERE id = ?1", [id]).map_err(err)?;
            } else {
                // What was paid shrinks in proportion to what was sold.
                let invested = invested.map(|i| i * (qty - units) / qty);
                conn.execute(
                    "UPDATE holdings SET quantity = ?2, invested = ?3 WHERE id = ?1",
                    rusqlite::params![id, qty - units, invested],
                )
                .map_err(err)?;
            }
        }
        Action::SetInvested { symbol, amount } => {
            let changed = conn
                .execute("UPDATE holdings SET invested = ?2 WHERE symbol = ?1", rusqlite::params![symbol, amount])
                .map_err(err)?;
            if changed == 0 {
                return Err(format!("{symbol} is not in your portfolio yet."));
            }
        }
        Action::UlipFund { code, name, group, units, nav } => {
            let id: Option<i64> = conn
                .query_row(
                    "SELECT id FROM holdings WHERE kind = 'ulip' AND symbol = ?1 AND broker = ?2",
                    rusqlite::params![code, group],
                    |r| r.get(0),
                )
                .ok();
            let as_of = today(conn);
            match id {
                Some(id) => {
                    conn.execute(
                        "UPDATE holdings
                            SET quantity = COALESCE(?2, quantity),
                                unit_price = COALESCE(?3, unit_price),
                                price_as_of = CASE WHEN ?3 IS NULL THEN price_as_of ELSE ?4 END
                          WHERE id = ?1",
                        rusqlite::params![id, units, nav, as_of],
                    )
                    .map_err(err)?;
                }
                None => {
                    let units = units.ok_or(format!("Give the units for {code} the first time, e.g. \"{code} units 156.14 nav 9.5\"."))?;
                    conn.execute(
                        "INSERT INTO holdings (symbol, label, quantity, kind, broker, unit_price, price_as_of)
                         VALUES (?1, ?2, ?3, 'ulip', ?4, ?5, CASE WHEN ?5 IS NULL THEN NULL ELSE ?6 END)",
                        rusqlite::params![code, name, units, group, nav, as_of],
                    )
                    .map_err(err)?;
                }
            }
        }
        Action::PremiumPaid { group, amount } => {
            conn.execute(
                "INSERT INTO portfolio_groups (name, invested) VALUES (?1, ?2)
                 ON CONFLICT(name) DO UPDATE SET invested = invested + ?2",
                rusqlite::params![group, amount],
            )
            .map_err(err)?;
        }
        Action::Remove { symbol } => {
            let changed = conn.execute("DELETE FROM holdings WHERE symbol = ?1", [symbol]).map_err(err)?;
            if changed == 0 {
                return Err(format!("{symbol} is not in your portfolio."));
            }
        }
    }
    Ok(action.describe())
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

    fn run(conn: &rusqlite::Connection, text: &str) {
        for a in read(text).actions {
            apply(conn, &a).unwrap();
        }
    }

    #[test]
    fn buying_twice_adds_up_and_selling_shrinks_what_was_paid() {
        let conn = db();
        run(&conn, "bought 5 goldbees in icici direct at 117");
        run(&conn, "bought 5 goldbees in icici direct at 119");
        let (qty, invested): (f64, Option<f64>) = conn
            .query_row("SELECT quantity, invested FROM holdings", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((qty, invested), (10.0, Some(1180.0)));
        run(&conn, "sold 5 goldbees");
        let (qty, invested): (f64, Option<f64>) = conn
            .query_row("SELECT quantity, invested FROM holdings", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((qty, invested), (5.0, Some(590.0)));
    }

    #[test]
    fn a_buy_without_a_price_makes_what_was_paid_unknown_not_wrong() {
        let conn = db();
        run(&conn, "bought 5 goldbees at 117");
        run(&conn, "bought 5 goldbees");
        let invested: Option<f64> = conn.query_row("SELECT invested FROM holdings", [], |r| r.get(0)).unwrap();
        assert_eq!(invested, None);
    }

    #[test]
    fn ulip_funds_are_created_then_their_nav_updated() {
        let conn = db();
        run(&conn, "MUP units 156.1421 nav 9.5085");
        run(&conn, "MUP nav 9.7");
        let (units, nav): (f64, f64) = conn
            .query_row("SELECT quantity, unit_price FROM holdings WHERE symbol = 'MUP'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((units, nav), (156.1421, 9.7));
        assert!(apply(&conn, &read("NAP nav 10.5").actions[0]).is_err(), "a new fund needs its units");
        run(&conn, "paid 8000 tata aia premium");
        run(&conn, "paid 8000 tata aia premium");
        let paid: f64 = conn.query_row("SELECT invested FROM portfolio_groups", [], |r| r.get(0)).unwrap();
        assert_eq!(paid, 16_000.0);
    }

    #[test]
    fn selling_what_you_do_not_have_is_refused() {
        let conn = db();
        assert!(apply(&conn, &read("sold 2 goldbees").actions[0]).is_err());
    }
}
