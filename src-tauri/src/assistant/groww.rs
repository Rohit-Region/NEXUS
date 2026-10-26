//! Groww's public web API, for company fundamentals and fund statistics.
//!
//! # Why Groww
//!
//! It is the one free source found that serves, as JSON and without a key,
//! both what the stock suggestions need (ROE, P/E against the industry,
//! debt, revenue and profit growth, quarterly shareholding by FII and DII)
//! and what the fund suggestions need (category rank, Sharpe, Sortino, alpha,
//! expense ratio, AUM, minimum SIP, manager history, holdings). Yahoo's
//! fundamentals endpoint now wants a session crumb and rate-limits hard.
//!
//! # Fragility
//!
//! These are the endpoints Groww's own website calls, not a published API.
//! They can change shape without notice. Every reader here treats a missing
//! field as unknown rather than failing, so a renamed field degrades one
//! factor of a score instead of the whole screen.

use serde_json::Value;

use super::http::{send, HttpError, Request};

const BASE: &str = "https://groww.in/v1/api";
/// Groww serves its web API to browsers; a bare agent is sometimes refused.
const AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124 Safari/537.36";

fn get(url: &str) -> Result<Value, String> {
    let resp = send(Request::get(url).with_header("User-Agent", AGENT)).map_err(|e: HttpError| e.to_string())?;
    if !resp.ok() {
        return Err(format!("Groww returned status {}", resp.status));
    }
    serde_json::from_str(&resp.body).map_err(|e| format!("unreadable Groww data: {e}"))
}

/// Percent-encode a query value. Only unreserved characters pass through.
pub fn encode(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Search entries of one type, as `(id, code)`: the Groww page id and the
/// exchange symbol.
fn search(query: &str, entity: &str) -> Result<Vec<(String, String)>, String> {
    let url = format!(
        "{BASE}/search/v1/entity?app=false&entity_type={entity}&page=0&q={}&size=10",
        encode(query)
    );
    let v = get(&url)?;
    Ok(v.get("content")
        .and_then(|c| c.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let id = r.get("id")?.as_str()?.to_string();
                    let code = r.get("nse_scrip_code")?.as_str()?.to_string();
                    Some((id, code))
                })
                .collect()
        })
        .unwrap_or_default())
}

/// The Groww page id of the stock with this NSE symbol, matched exactly so
/// `M&M` does not resolve to `M&MFIN`.
pub fn stock_id(nse_symbol: &str) -> Result<String, String> {
    search(nse_symbol, "stocks")?
        .into_iter()
        .find(|(_, code)| code.eq_ignore_ascii_case(nse_symbol))
        .map(|(id, _)| id)
        .ok_or_else(|| format!("{nse_symbol} not found on Groww"))
}

/// Every open direct-plan fund in one sub-category ("Small Cap"), as
/// `(page id, fund name)`.
pub fn category_funds(sub_category: &str) -> Result<Vec<(String, String)>, String> {
    let url = format!(
        "{BASE}/search/v1/derived/scheme?available_for_investment=true&doc_type=scheme&page=0&plan_type=Direct&q=&size=200&sub_category={}",
        encode(sub_category)
    );
    let v = get(&url)?;
    Ok(v.get("content")
        .and_then(|c| c.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let id = r.get("search_id")?.as_str()?.to_string();
                    let name = r.get("scheme_name").or_else(|| r.get("fund_name"))?.as_str()?.to_string();
                    Some((id, name))
                })
                .collect()
        })
        .unwrap_or_default())
}

pub fn company(id: &str) -> Result<Value, String> {
    get(&format!("{BASE}/stocks_data/v1/company/search_id/{}?page=0&size=10", encode(id)))
}

pub fn scheme(id: &str) -> Result<Value, String> {
    get(&format!("{BASE}/data/mf/web/v4/scheme/search/{}", encode(id)))
}

/// A number out of Groww's display strings: "₹16,10,372Cr", "8.94%", "18.27".
pub fn number(raw: &str) -> Option<f64> {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    cleaned.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_strings_become_numbers() {
        assert_eq!(number("₹16,10,372Cr"), Some(1610372.0));
        assert_eq!(number("8.94%"), Some(8.94));
        assert_eq!(number("-3.5"), Some(-3.5));
        assert_eq!(number("NA"), None);
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(encode("M&M"), "M%26M");
        assert_eq!(encode("Quant Small Cap"), "Quant%20Small%20Cap");
    }
}
