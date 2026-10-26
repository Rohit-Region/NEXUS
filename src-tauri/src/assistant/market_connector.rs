//! Live market prices, for the Money section.
//!
//! # Source
//!
//! Yahoo Finance's public chart endpoint. It needs no key and answers in the
//! instrument's own currency, so an NSE stock (`RELIANCE.NS`) and a gold ETF
//! (`GOLDBEES.NS`) both come back in INR with no conversion. That is why the
//! portfolio tracks anything with a Yahoo symbol rather than special-casing
//! gold: a gold ETF is priced the same way a share is.
//!
//! # What it is not
//!
//! Advice. This fetches a number and a previous close. It does not rank, rate,
//! predict, or recommend anything. The Money section informs; the scored
//! suggestions live in the Stocks section (`stock_picks`, `fund_picks`), where
//! each one shows the rules behind it.
//!
//! # A note on the corporate proxy
//!
//! An earlier feature found the app's own requests to some hosts silently
//! intercepted by a corporate proxy while the same request worked in a browser.
//! Yahoo is reachable today; if it ever starts failing only inside the app, the
//! fallback is the same one the stock watcher uses: read the number out of an
//! open browser tab. Noted so the next person does not re-diagnose it.

use serde::Serialize;

use super::http::{send, HttpError, Request};

/// One instrument's current price.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub symbol: String,
    pub price: f64,
    /// Yesterday's close, for the day's change. `None` when the feed omits it.
    pub prev_close: Option<f64>,
    pub currency: String,
    /// The instrument's own name, used as a default label for a new holding.
    pub name: Option<String>,
}

/// A symbol cleaned to what may sit in the request URL, or `None`.
///
/// Yahoo symbols are letters, digits, and a few punctuation marks
/// (`RELIANCE.NS`, `GC=F`, `^NSEI`, `BRK-B`). Anything outside that set is
/// refused rather than escaped, because the symbol goes straight into the URL
/// path and a stray character there is a malformed request, not a lookup.
pub fn valid_symbol(raw: &str) -> Option<String> {
    let s = raw.trim().to_uppercase();
    if s.is_empty() || s.len() > 24 {
        return None;
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '=' | '^'))
    {
        return None;
    }
    Some(s)
}

/// Fetch one symbol's quote.
pub fn fetch_quote(symbol: &str) -> Result<Quote, String> {
    let symbol = valid_symbol(symbol)
        .ok_or_else(|| format!("\"{symbol}\" is not a valid market symbol."))?;

    let url = format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?interval=1d&range=1d"
    );
    // Yahoo answers curl's default agent with 403 often enough to matter, so
    // the request presents a browser agent. No credentials are involved; this
    // is a public endpoint.
    let request = Request::get(&url).with_header("User-Agent", "Mozilla/5.0 (NEXUS)");

    let resp = send(request).map_err(|e: HttpError| match e {
        HttpError::Unreachable { detail } => format!("Could not reach the market data: {detail}"),
        other => other.to_string(),
    })?;
    if !resp.ok() {
        return Err(format!("Market data returned status {}.", resp.status));
    }

    parse_quote(&resp.body, &symbol)
}

fn parse_quote(body: &str, symbol: &str) -> Result<Quote, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Unreadable market data: {e}"))?;

    // Yahoo reports a lookup failure inside the body, not as an HTTP status.
    if let Some(err) = value
        .pointer("/chart/error")
        .filter(|e| !e.is_null())
    {
        let desc = err
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("symbol not found");
        return Err(format!("{symbol}: {desc}"));
    }

    let meta = value
        .pointer("/chart/result/0/meta")
        .ok_or_else(|| format!("No quote for {symbol}."))?;

    let price = meta
        .get("regularMarketPrice")
        .and_then(|p| p.as_f64())
        .ok_or_else(|| format!("No price for {symbol}."))?;

    let prev_close = meta
        .get("chartPreviousClose")
        .or_else(|| meta.get("previousClose"))
        .and_then(|p| p.as_f64());

    let currency = meta
        .get("currency")
        .and_then(|c| c.as_str())
        .unwrap_or("INR")
        .to_string();

    let name = meta
        .get("longName")
        .or_else(|| meta.get("shortName"))
        .and_then(|n| n.as_str())
        .map(|n| n.to_string())
        .filter(|n| !n.trim().is_empty());

    Ok(Quote {
        symbol: symbol.to_string(),
        price,
        prev_close,
        currency,
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_are_restricted_to_what_a_url_path_allows() {
        for good in ["RELIANCE.NS", "GOLDBEES.NS", "GC=F", "^NSEI", "BRK-B", "tcs.ns"] {
            assert!(valid_symbol(good).is_some(), "{good}");
        }
        // Uppercased on the way in, so matching downstream is stable.
        assert_eq!(valid_symbol("reliance.ns").as_deref(), Some("RELIANCE.NS"));
        for bad in ["", "a b", "x/y", "drop;table", "sym#1", "   "] {
            assert!(valid_symbol(bad).is_none(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn a_real_quote_body_is_parsed() {
        let body = r#"{"chart":{"result":[{"meta":{
            "regularMarketPrice":1209.0,"chartPreviousClose":1202.0,
            "currency":"INR","symbol":"RELIANCE.NS","longName":"Reliance Industries"}}],
            "error":null}}"#;
        let q = parse_quote(body, "RELIANCE.NS").expect("parse");
        assert_eq!(q.price, 1209.0);
        assert_eq!(q.prev_close, Some(1202.0));
        assert_eq!(q.currency, "INR");
        assert_eq!(q.name.as_deref(), Some("Reliance Industries"));
    }

    #[test]
    fn a_yahoo_error_body_is_surfaced_not_swallowed() {
        let body = r#"{"chart":{"result":null,"error":{"code":"Not Found","description":"No data found, symbol may be delisted"}}}"#;
        let err = parse_quote(body, "NOPE.NS").expect_err("must error");
        assert!(err.contains("delisted"), "{err}");
    }

    #[test]
    fn a_missing_price_is_an_error_not_a_zero() {
        // A zero price would read as "worth nothing" in the portfolio, which is
        // a wrong answer, not a missing one.
        let body = r#"{"chart":{"result":[{"meta":{"currency":"INR"}}],"error":null}}"#;
        assert!(parse_quote(body, "X.NS").is_err());
    }

    /// Live, ignored by default: needs the network and Yahoo reachable.
    /// `cargo test -- --ignored --nocapture live_quote`.
    #[test]
    #[ignore]
    fn live_quote() {
        match fetch_quote("RELIANCE.NS") {
            Ok(q) => println!("{} = {} {}", q.symbol, q.price, q.currency),
            Err(e) => println!("no quote: {e}"),
        }
    }
}
