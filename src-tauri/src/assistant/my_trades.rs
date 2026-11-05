//! What the investor holds and buys, read from the Money section's holdings.
//!
//! # Holdings for suggestions
//!
//! Stocks already held are left out of suggestions, and a sector that is
//! already over 30% of the stock holdings' value is marked down (see
//! `growth_picks`). Funds and ULIPs are not stocks and do not count toward
//! a sector.
//!
//! # Buys for the Report
//!
//! The holdings table keeps a quantity, not a trade history, so a buy is
//! detected as a quantity that went up since the last look. The first look
//! records a baseline and reports nothing: what was held before NEXUS
//! started watching is not a decision it can grade. A buy is dated the day
//! it is noticed, which is the day it was entered unless the app was closed
//! in between.
//!
//! Stock and ETF buys are graded; fund buys are not yet, because a fund
//! bought today is priced at a NAV not published until tonight.

use std::collections::HashMap;

use rusqlite::Connection;

use super::growth_picks::{Investor, HEAVY_SECTOR};

const SNAPSHOT_KEY: &str = "holding_quantities";

#[derive(Debug, Clone, PartialEq)]
pub struct Holding {
    /// As the user entered it ("GOLDBEES.NS").
    pub symbol: String,
    pub kind: String,
    pub quantity: f64,
    pub invested: Option<f64>,
}

impl Holding {
    /// The NSE ticker without an exchange suffix, as the suggestion lists
    /// name it.
    pub fn ticker(&self) -> String {
        let s = self.symbol.trim().to_uppercase();
        s.strip_suffix(".NS").or_else(|| s.strip_suffix(".BO")).unwrap_or(&s).to_string()
    }

    pub fn is_stock(&self) -> bool {
        self.kind == "stock"
    }
}

/// A quantity increase noticed since the last look.
#[derive(Debug, Clone, PartialEq)]
pub struct Buy {
    pub holding: Holding,
    pub added: f64,
    /// What the added quantity cost, when the invested figure moved with it.
    pub paid: Option<f64>,
}

pub fn holdings(conn: &Connection) -> Vec<Holding> {
    let Ok(mut stmt) = conn.prepare("SELECT symbol, kind, quantity, invested FROM holdings") else {
        return Vec::new();
    };
    stmt.query_map([], |r| {
        Ok(Holding { symbol: r.get(0)?, kind: r.get(1)?, quantity: r.get(2)?, invested: r.get(3)? })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// What to leave out and spread away from. `sector_value` gives a held
/// ticker's industry and price, for those NEXUS can price.
pub fn investor(held: &[Holding], sector_value: &HashMap<String, (String, f64)>) -> Investor {
    let stocks: Vec<&Holding> = held.iter().filter(|h| h.is_stock() && h.quantity > 0.0).collect();
    let mut by_sector: HashMap<String, f64> = HashMap::new();
    let mut total = 0.0;
    for h in &stocks {
        if let Some((industry, price)) = sector_value.get(&h.ticker()) {
            let value = h.quantity * price;
            total += value;
            *by_sector.entry(industry.clone()).or_default() += value;
        }
    }
    Investor {
        owned: stocks.iter().map(|h| h.ticker()).collect(),
        // One holding is always 100% of itself; heaviness needs a spread.
        heavy_sectors: if stocks.len() >= 2 && total > 0.0 {
            by_sector.into_iter().filter(|(_, v)| v / total > HEAVY_SECTOR).map(|(s, _)| s).collect()
        } else {
            Default::default()
        },
    }
}

/// Stock buys since the last call, and the new baseline stored. The first
/// call only stores the baseline.
pub fn new_buys(conn: &Connection) -> Result<Vec<Buy>, String> {
    let held = holdings(conn);
    let previous: Option<HashMap<String, (f64, Option<f64>)>> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [SNAPSHOT_KEY], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok());
    let now: HashMap<String, (f64, Option<f64>)> =
        held.iter().map(|h| (format!("{}|{}", h.kind, h.symbol), (h.quantity, h.invested))).collect();
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        rusqlite::params![SNAPSHOT_KEY, serde_json::to_string(&now).map_err(|e| e.to_string())?],
    )
    .map_err(|e| format!("Could not store the holdings baseline: {e}"))?;

    let Some(previous) = previous else { return Ok(Vec::new()) };
    Ok(diff(&held, &previous))
}

fn diff(held: &[Holding], previous: &HashMap<String, (f64, Option<f64>)>) -> Vec<Buy> {
    held.iter()
        .filter(|h| h.is_stock())
        .filter_map(|h| {
            let (qty_before, invested_before) = previous.get(&format!("{}|{}", h.kind, h.symbol)).copied().unwrap_or((0.0, None));
            let added = h.quantity - qty_before;
            if added <= 1e-9 {
                return None;
            }
            let paid = match (h.invested, invested_before) {
                (Some(now), Some(before)) if now > before => Some(now - before),
                (Some(now), None) if qty_before == 0.0 => Some(now),
                _ => None,
            };
            Some(Buy { holding: h.clone(), added, paid })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(symbol: &str, kind: &str, quantity: f64, invested: Option<f64>) -> Holding {
        Holding { symbol: symbol.into(), kind: kind.into(), quantity, invested }
    }

    #[test]
    fn tickers_drop_the_exchange_suffix() {
        assert_eq!(h("goldbees.ns", "stock", 1.0, None).ticker(), "GOLDBEES");
        assert_eq!(h("HFCL.BO", "stock", 1.0, None).ticker(), "HFCL");
        assert_eq!(h("HFCL", "stock", 1.0, None).ticker(), "HFCL");
    }

    #[test]
    fn buys_are_quantity_increases_on_stocks_only() {
        let mut before = HashMap::new();
        before.insert("stock|HFCL.NS".to_string(), (2.0, Some(480.0)));
        before.insert("mf|120828".to_string(), (10.0, None));
        let held = vec![
            h("HFCL.NS", "stock", 4.0, Some(980.0)),
            h("CUPID.NS", "stock", 1.0, Some(314.0)),
            h("120828", "mf", 20.0, None),
            h("TCS.NS", "stock", 0.0, None),
        ];
        let buys = diff(&held, &before);
        assert_eq!(buys.len(), 2);
        assert_eq!((buys[0].holding.ticker(), buys[0].added, buys[0].paid), ("HFCL".into(), 2.0, Some(500.0)));
        assert_eq!((buys[1].holding.ticker(), buys[1].added, buys[1].paid), ("CUPID".into(), 1.0, Some(314.0)));
    }

    #[test]
    fn a_sector_is_heavy_only_across_several_holdings() {
        let mut sv = HashMap::new();
        sv.insert("A".to_string(), ("Capital Goods".to_string(), 100.0));
        sv.insert("B".to_string(), ("Capital Goods".to_string(), 100.0));
        sv.insert("C".to_string(), ("Healthcare".to_string(), 100.0));
        let held = vec![h("A.NS", "stock", 5.0, None), h("B.NS", "stock", 5.0, None), h("C.NS", "stock", 1.0, None)];
        let inv = investor(&held, &sv);
        assert!(inv.owned.contains("A") && inv.owned.contains("C"));
        assert!(inv.heavy_sectors.contains("Capital Goods"));
        assert!(!inv.heavy_sectors.contains("Healthcare"));
        // A single holding is not "heavy" in anything.
        let one = investor(&held[..1], &sv);
        assert!(one.heavy_sectors.is_empty());
    }
}
