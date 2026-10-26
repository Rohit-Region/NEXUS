//! The Portfolio chat box: read what the user typed, show it back, apply it
//! only once confirmed.

use tauri::State;

use crate::assistant::portfolio_chat::{read, Action, Reading};
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
        Action::UlipFund { code, name, units, nav } => {
            let group = crate::assistant::portfolio_chat::TATA_AIA_GROUP;
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
