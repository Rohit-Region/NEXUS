//! Telling NEXUS about the portfolio in plain words.
//!
//! "bought 5 goldbees in icici direct at 117", "MUP units 156.1421 nav
//! 9.5085", "paid 8000 tata aia premium". Each line becomes an `Action` the
//! user sees in words and confirms before anything changes.
//!
//! # Rules, not a model
//!
//! Every number here is money. The local model has been seen inventing facts
//! when it writes, and a misread "8000" turned into a holding is a silent
//! error in every total after it. So this reads a small, fixed grammar, says
//! plainly what it understood, and says "didn't understand" for the rest
//! rather than guessing.

use serde::{Deserialize, Serialize};

/// The ULIP policy the fund codes below belong to.
pub const TATA_AIA_GROUP: &str = "Tata AIA Smart Pension Secure";

/// Tata AIA pension fund codes, as the policy page names them.
const ULIP_FUNDS: &[(&str, &str)] = &[
    ("MUP", "Multicap Opportunities Pension Fund"),
    ("NAP", "Alpha 50 Index Pension Fund"),
    ("MVF", "Momentum Value 50 Index Fund"),
    ("FEP", "Future Equity Pension Fund"),
    ("IPF", "Income Pension Fund"),
    ("MQP", "Multicap Momentum Quality Index Pension Fund"),
    ("TBP", "Life Tax Bonanza Consumption Pension Fund"),
    ("TAP", "Top 200 Alpha 30 Index Pension Fund"),
    ("SLP", "Sector Leaders Index Pension Fund"),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    /// Units bought; `price` per unit when given, which adds to invested.
    #[serde(rename_all = "camelCase")]
    Buy { symbol: String, units: f64, broker: Option<String>, price: Option<f64> },
    #[serde(rename_all = "camelCase")]
    Sell { symbol: String, units: f64 },
    /// What was paid for a holding in total.
    #[serde(rename_all = "camelCase")]
    SetInvested { symbol: String, amount: f64 },
    /// A ULIP fund's units and/or NAV, as on the policy page.
    #[serde(rename_all = "camelCase")]
    UlipFund { code: String, name: String, units: Option<f64>, nav: Option<f64> },
    /// A premium paid into a ULIP policy.
    #[serde(rename_all = "camelCase")]
    PremiumPaid { group: String, amount: f64 },
    #[serde(rename_all = "camelCase")]
    Remove { symbol: String },
}

impl Action {
    /// What the user is asked to confirm.
    pub fn describe(&self) -> String {
        match self {
            Action::Buy { symbol, units, broker, price } => {
                let mut s = format!("Add {} unit{} of {symbol}", fmt(*units), if *units == 1.0 { "" } else { "s" });
                if let Some(b) = broker {
                    s.push_str(&format!(" in {b}"));
                }
                if let Some(p) = price {
                    s.push_str(&format!(" at ₹{} each (₹{} invested)", fmt(*p), fmt(p * units)));
                }
                s
            }
            Action::Sell { symbol, units } => format!("Remove {} unit(s) of {symbol} (sold)", fmt(*units)),
            Action::SetInvested { symbol, amount } => format!("Set what you paid for {symbol} to ₹{}", fmt(*amount)),
            Action::UlipFund { code, name, units, nav } => {
                let mut parts = Vec::new();
                if let Some(u) = units {
                    parts.push(format!("{} units", fmt(*u)));
                }
                if let Some(n) = nav {
                    parts.push(format!("NAV ₹{}", fmt(*n)));
                }
                format!("{name} ({code}): {}", parts.join(", "))
            }
            Action::PremiumPaid { group, amount } => format!("Add a premium of ₹{} paid into {group}", fmt(*amount)),
            Action::Remove { symbol } => format!("Remove {symbol} from the portfolio"),
        }
    }
}

fn fmt(n: f64) -> String {
    let s = format!("{n:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub actions: Vec<Action>,
    /// Lines that matched nothing, quoted back.
    pub not_understood: Vec<String>,
}

/// Read every line (or ";"-separated part) of what the user typed.
pub fn read(text: &str) -> Reading {
    let mut actions = Vec::new();
    let mut not_understood = Vec::new();
    for part in without_digit_commas(text).split(['\n', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match read_one(part) {
            Some(a) => actions.push(a),
            None => not_understood.push(part.to_string()),
        }
    }
    Reading { actions, not_understood }
}

/// "7,365.88" as "7365.88", so a comma between digits is never a separator.
fn without_digit_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars
        .iter()
        .enumerate()
        .filter(|(i, c)| {
            !(**c == ','
                && *i > 0
                && chars[i - 1].is_ascii_digit()
                && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit()))
        })
        .map(|(_, c)| *c)
        .collect()
}

fn words(part: &str) -> Vec<String> {
    part.to_lowercase()
        .replace('₹', " ")
        .split(|c: char| c.is_whitespace() || c == ':' || c == '=')
        .map(|w| w.trim_matches(|c: char| c == ',' || c == '(' || c == ')').to_string())
        .map(|w| w.strip_prefix("rs.").or_else(|| w.strip_prefix("rs")).filter(|r| r.parse::<f64>().is_ok()).map(str::to_string).unwrap_or(w))
        .filter(|w| !w.is_empty())
        .collect()
}

fn number(w: &str) -> Option<f64> {
    w.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0)
}

/// The number right after any of `keys`.
fn after(ws: &[String], keys: &[&str]) -> Option<f64> {
    ws.windows(2).find(|p| keys.contains(&p[0].as_str())).and_then(|p| number(&p[1]))
}

fn broker_in(part: &str) -> Option<String> {
    let l = part.to_lowercase();
    if l.contains("icici direct") || l.contains("icicidirect") || l.contains("icici") {
        Some("ICICI Direct".to_string())
    } else if l.contains("zerodha") || l.contains("kite") {
        Some("Zerodha".to_string())
    } else {
        None
    }
}

const NOT_A_SYMBOL: &[&str] = &[
    "bought", "buy", "add", "added", "sold", "sell", "remove", "delete", "units", "unit", "qty",
    "quantity", "shares", "share", "of", "in", "at", "on", "via", "from", "price", "each", "rs",
    "inr", "invested", "cost", "paid", "for", "the", "my", "icici", "direct", "icicidirect",
    "zerodha", "kite", "nav", "and", "a", "i", "have",
];

/// A market symbol from the words: the first word that is not grammar, a
/// number or a broker. NSE is assumed when no exchange is given.
fn symbol_in(ws: &[String]) -> Option<String> {
    let w = ws.iter().find(|w| {
        !NOT_A_SYMBOL.contains(&w.as_str())
            && number(w).is_none()
            && w.chars().any(|c| c.is_ascii_alphabetic())
            && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '&')
    })?;
    let upper = w.to_uppercase();
    Some(if upper.contains('.') { upper } else { format!("{upper}.NS") })
}

fn read_one(part: &str) -> Option<Action> {
    let ws = words(part);
    let has = |k: &str| ws.iter().any(|w| w == k);
    let lower = part.to_lowercase();

    // A ULIP fund line: a known fund code plus units and/or NAV.
    if let Some((code, name)) = ULIP_FUNDS.iter().find(|(code, _)| has(&code.to_lowercase())) {
        let units = after(&ws, &["units", "unit"]);
        let nav = after(&ws, &["nav", "price"]);
        if units.is_some() || nav.is_some() {
            return Some(Action::UlipFund { code: code.to_string(), name: name.to_string(), units, nav });
        }
        return None;
    }

    // A premium into the Tata AIA policy.
    if (lower.contains("tata aia") || lower.contains("tataaia")) && (has("premium") || has("paid")) {
        let amount = ws.iter().find_map(|w| number(w).filter(|n| *n > 0.0))?;
        return Some(Action::PremiumPaid { group: TATA_AIA_GROUP.to_string(), amount });
    }

    if has("remove") || has("delete") {
        return symbol_in(&ws).map(|symbol| Action::Remove { symbol });
    }

    if has("invested") || has("cost") {
        let symbol = symbol_in(&ws)?;
        let amount = after(&ws, &["invested", "cost"]).or_else(|| ws.iter().find_map(|w| number(w).filter(|n| *n > 0.0)))?;
        return Some(Action::SetInvested { symbol, amount });
    }

    let buying = ["bought", "buy", "add", "added", "have"].iter().any(|k| has(k));
    let selling = has("sold") || has("sell");
    if buying || selling {
        let symbol = symbol_in(&ws)?;
        // The units: the first number not following "at"/"price".
        let units = ws
            .iter()
            .enumerate()
            .find(|(i, w)| number(w).is_some_and(|n| n > 0.0) && !(*i > 0 && ["at", "price", "@"].contains(&ws[i - 1].as_str())))
            .and_then(|(_, w)| number(w))?;
        if selling {
            return Some(Action::Sell { symbol, units });
        }
        let price = after(&ws, &["at", "price", "@"]).filter(|p| *p > 0.0);
        return Some(Action::Buy { symbol, units, broker: broker_in(part), price });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Action {
        let r = read(text);
        assert!(r.not_understood.is_empty(), "{text}: {:?}", r.not_understood);
        assert_eq!(r.actions.len(), 1, "{text}");
        r.actions[0].clone()
    }

    #[test]
    fn buying_and_selling() {
        assert_eq!(
            one("bought 5 goldbees in icici direct"),
            Action::Buy { symbol: "GOLDBEES.NS".into(), units: 5.0, broker: Some("ICICI Direct".into()), price: None }
        );
        assert_eq!(
            one("buy 10 RELIANCE at 1,250.50 on zerodha"),
            Action::Buy { symbol: "RELIANCE.NS".into(), units: 10.0, broker: Some("Zerodha".into()), price: Some(1250.5) }
        );
        assert_eq!(one("sold 2 goldbees"), Action::Sell { symbol: "GOLDBEES.NS".into(), units: 2.0 });
    }

    #[test]
    fn what_was_paid_and_removal() {
        assert_eq!(one("goldbees invested 586.25"), Action::SetInvested { symbol: "GOLDBEES.NS".into(), amount: 586.25 });
        assert_eq!(one("remove goldbees"), Action::Remove { symbol: "GOLDBEES.NS".into() });
    }

    #[test]
    fn ulip_funds_from_the_policy_page() {
        assert_eq!(
            one("MUP units 156.1421 nav 9.5085"),
            Action::UlipFund { code: "MUP".into(), name: "Multicap Opportunities Pension Fund".into(), units: Some(156.1421), nav: Some(9.5085) }
        );
        assert_eq!(
            one("nap nav 10.6"),
            Action::UlipFund { code: "NAP".into(), name: "Alpha 50 Index Pension Fund".into(), units: None, nav: Some(10.6) }
        );
        assert_eq!(one("paid 8,000 tata aia premium"), Action::PremiumPaid { group: TATA_AIA_GROUP.into(), amount: 8000.0 });
    }

    #[test]
    fn several_lines_at_once_and_the_rest_quoted_back() {
        let r = read("MUP units 156.1421 nav 9.5085\nNAP units 204.07 nav 10.4883; hello there");
        assert_eq!(r.actions.len(), 2);
        assert_eq!(r.not_understood, vec!["hello there".to_string()]);
    }

    #[test]
    fn nothing_is_guessed_from_vague_text() {
        assert!(read("how is my portfolio doing").actions.is_empty());
        assert!(read("MUP is doing well").actions.is_empty(), "a fund code with no number changes nothing");
        assert!(read("bought some goldbees").actions.is_empty(), "no quantity, no action");
    }

    #[test]
    fn confirmations_read_plainly() {
        let a = Action::Buy { symbol: "GOLDBEES.NS".into(), units: 5.0, broker: Some("ICICI Direct".into()), price: Some(117.25) };
        assert_eq!(a.describe(), "Add 5 units of GOLDBEES.NS in ICICI Direct at ₹117.25 each (₹586.25 invested)");
    }
}
