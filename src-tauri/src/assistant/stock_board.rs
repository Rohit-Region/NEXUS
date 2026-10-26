//! The Stocks section: how the Nifty 50 is moving today and over recent weeks.
//!
//! # What it is not
//!
//! Advice. Every list on the board is a sort or a filter over published
//! prices with its rule stated on screen: biggest risers, biggest fallers, and
//! stocks in a rising trend. The scored buy and avoid calls are built on top
//! of this board in `stock_picks`.
//!
//! # Source
//!
//! Yahoo's chart endpoint, six months of daily closes per symbol, which is
//! enough for the 50-day average and a three-month return. The day's change
//! comes from Yahoo's own `regularMarketChangePercent` when present, because
//! deriving it from the closes needs to know whether today's bar has printed.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::http::{send, HttpError, Request};

/// Indices shown above the lists, as (Yahoo symbol, label).
const INDICES: &[(&str, &str)] = &[
    ("^NSEI", "Nifty 50"),
    ("^BSESN", "Sensex"),
    ("^NSEBANK", "Bank Nifty"),
];

/// The Nifty 50 as of late 2026. Index membership changes twice a year; a
/// symbol Yahoo stops answering for is reported as skipped, not fatal, so a
/// stale entry shows up as a smaller count rather than a broken screen.
const UNIVERSE: &[&str] = &[
    "ADANIENT", "ADANIPORTS", "APOLLOHOSP", "ASIANPAINT", "AXISBANK", "BAJAJ-AUTO",
    "BAJFINANCE", "BAJAJFINSV", "BEL", "BHARTIARTL", "CIPLA", "COALINDIA", "DRREDDY",
    "EICHERMOT", "ETERNAL", "GRASIM", "HCLTECH", "HDFCBANK", "HDFCLIFE", "HINDALCO",
    "HINDUNILVR", "ICICIBANK", "INDIGO", "INFY", "ITC", "JIOFIN", "JSWSTEEL", "KOTAKBANK",
    "LT", "M&M", "MARUTI", "MAXHEALTH", "NESTLEIND", "NTPC", "ONGC", "POWERGRID",
    "RELIANCE", "SBILIFE", "SBIN", "SHRIRAMFIN", "SUNPHARMA", "TATACONSUM", "TMPV",
    "TATASTEEL", "TCS", "TECHM", "TITAN", "TRENT", "ULTRACEMCO", "WIPRO",
];

/// Closes kept for the row's sparkline.
const SPARK: usize = 30;
/// Requests in flight at once. Fifty at a time reads to Yahoo like a scraper
/// and starts drawing 429s; ten keeps a cold load to a few seconds.
const IN_FLIGHT: usize = 10;
/// Prices move all session, but a re-read every screen open is waste.
const CACHE_FOR: Duration = Duration::from_secs(300);

static CACHE: Mutex<Option<(Instant, Board)>> = Mutex::new(None);

/// One instrument's numbers. Returns are percentages; `None` when the history
/// is too short to say (a recent listing, or a feed gap).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    /// The NSE ticker as a person would type it, without Yahoo's `.NS`.
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub change_pct: Option<f64>,
    pub ret_1w: Option<f64>,
    pub ret_1m: Option<f64>,
    pub ret_3m: Option<f64>,
    /// Price relative to its 20- and 50-day averages, in percent.
    pub vs_sma20: Option<f64>,
    pub vs_sma50: Option<f64>,
    /// How far below the 52-week high, in percent (0 at the high).
    pub from_high: Option<f64>,
    /// Today's volume over the average of the 20 sessions before it.
    pub volume_ratio: Option<f64>,
    pub spark: Vec<f64>,
    /// Daily closes as `(IST day number, close)`, oldest first: what the pick
    /// journal reads a past suggestion's outcome from. Not sent to the screen.
    #[serde(skip)]
    pub history: Vec<(i64, f64)>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub indices: Vec<Row>,
    pub stocks: Vec<Row>,
    /// Symbols that could not be read this time, so the count is honest.
    pub skipped: Vec<String>,
    /// Unix seconds of the latest price on the board.
    pub as_of: i64,
    /// Unix seconds of this read.
    pub fetched_at: i64,
}

/// The board, from cache when fresh unless `refresh`.
pub fn board(refresh: bool, now: i64) -> Result<Board, String> {
    if !refresh {
        if let Some((at, b)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if at.elapsed() < CACHE_FOR {
                return Ok(b.clone());
            }
        }
    }

    let symbols: Vec<String> = INDICES
        .iter()
        .map(|(s, _)| s.to_string())
        .chain(UNIVERSE.iter().map(|s| format!("{s}.NS")))
        .collect();

    let mut results: Vec<Result<(Row, i64), String>> = Vec::with_capacity(symbols.len());
    for chunk in symbols.chunks(IN_FLIGHT) {
        let batch: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk.iter().map(|s| scope.spawn(move || fetch_row(s))).collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|_| Err("Failed".to_string())))
                .collect()
        });
        results.extend(batch);
    }

    let mut indices = Vec::new();
    let mut stocks = Vec::new();
    let mut skipped = Vec::new();
    let mut as_of = 0;
    for (i, (symbol, result)) in symbols.iter().zip(results).enumerate() {
        match result {
            Ok((mut row, t)) => {
                as_of = as_of.max(t);
                if let Some((_, label)) = INDICES.get(i) {
                    row.name = label.to_string();
                    indices.push(row);
                } else {
                    stocks.push(row);
                }
            }
            Err(e) => {
                log::warn!("stock_board: {symbol}: {e}");
                skipped.push(display_symbol(symbol));
            }
        }
    }

    if stocks.is_empty() {
        return Err("Could not read any market prices. Is the network up?".to_string());
    }

    let b = Board { indices, stocks, skipped, as_of, fetched_at: now };
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), b.clone()));
    Ok(b)
}

/// India Standard Time, which every NSE timestamp is a day in.
pub const IST_OFFSET: i64 = 19_800;

/// A unix time as a day number in the given UTC offset, so a session's date
/// is the date it traded on in Mumbai, not in UTC.
pub fn day_of(unix: i64, offset: i64) -> i64 {
    (unix + offset).div_euclid(86_400)
}

/// `RELIANCE.NS` -> `RELIANCE`; an index keeps its caret.
fn display_symbol(yahoo: &str) -> String {
    yahoo.strip_suffix(".NS").unwrap_or(yahoo).to_string()
}

fn fetch_row(symbol: &str) -> Result<(Row, i64), String> {
    // `&` (M&M) is the one character in the universe that is not path-safe.
    let path = symbol.replace('&', "%26").replace('^', "%5E");
    let url = format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{path}?interval=1d&range=6mo"
    );
    // Same browser agent as the quote fetch, for the same reason: Yahoo
    // answers curl's default agent with 403 often enough to matter.
    let request = Request::get(&url).with_header("User-Agent", "Mozilla/5.0 (NEXUS)");
    let resp = send(request).map_err(|e: HttpError| e.to_string())?;
    if !resp.ok() {
        return Err(format!("status {}", resp.status));
    }
    parse_row(&resp.body, symbol)
}

fn parse_row(body: &str, symbol: &str) -> Result<(Row, i64), String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("unreadable: {e}"))?;
    if let Some(err) = value.pointer("/chart/error").filter(|e| !e.is_null()) {
        let desc = err.get("description").and_then(|d| d.as_str()).unwrap_or("not found");
        return Err(desc.to_string());
    }
    let result = value.pointer("/chart/result/0").ok_or("no result")?;
    let meta = result.get("meta").ok_or("no meta")?;
    let num = |k: &str| meta.get(k).and_then(|v| v.as_f64());

    let price = num("regularMarketPrice").ok_or("no price")?;
    let as_of = meta.get("regularMarketTime").and_then(|v| v.as_i64()).unwrap_or(0);

    // Closes with their volumes, nulls (holidays, feed gaps) dropped together
    // so the two series stay aligned.
    let closes_raw = result.pointer("/indicators/quote/0/close").and_then(|v| v.as_array());
    let volumes_raw = result.pointer("/indicators/quote/0/volume").and_then(|v| v.as_array());
    let stamps = result.get("timestamp").and_then(|v| v.as_array());
    let offset = meta.get("gmtoffset").and_then(|v| v.as_i64()).unwrap_or(IST_OFFSET);
    let mut closes = Vec::new();
    let mut volumes = Vec::new();
    let mut history = Vec::new();
    if let Some(cs) = closes_raw {
        for (i, c) in cs.iter().enumerate() {
            if let Some(c) = c.as_f64() {
                if let Some(t) = stamps.and_then(|ts| ts.get(i)).and_then(|t| t.as_i64()) {
                    history.push((day_of(t, offset), c));
                }
                closes.push(c);
                volumes.push(
                    volumes_raw
                        .and_then(|vs| vs.get(i))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0),
                );
            }
        }
    }
    // Yahoo's last bar can lag the live price by a tick; the live price wins.
    if let Some(last) = closes.last_mut() {
        *last = price;
    }
    if let Some(last) = history.last_mut() {
        last.1 = price;
    }

    let change_pct = num("regularMarketChangePercent").or_else(|| {
        let n = closes.len();
        (n >= 2).then(|| pct(price, closes[n - 2])).flatten()
    });

    let back = |sessions: usize| -> Option<f64> {
        let n = closes.len();
        (n > sessions).then(|| pct(price, closes[n - 1 - sessions])).flatten()
    };
    let sma = |len: usize| -> Option<f64> {
        let n = closes.len();
        (n >= len).then(|| closes[n - len..].iter().sum::<f64>() / len as f64)
    };

    let volume_ratio = {
        let n = volumes.len();
        if n > 20 {
            let prior = volumes[n - 21..n - 1].iter().sum::<f64>() / 20.0;
            (prior > 0.0 && volumes[n - 1] > 0.0).then(|| volumes[n - 1] / prior)
        } else {
            None
        }
    };

    let name = meta
        .get("longName")
        .or_else(|| meta.get("shortName"))
        .and_then(|n| n.as_str())
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| display_symbol(symbol));

    let row = Row {
        symbol: display_symbol(symbol),
        name,
        price,
        change_pct,
        ret_1w: back(5),
        ret_1m: back(21),
        ret_3m: back(63),
        vs_sma20: sma(20).and_then(|a| pct(price, a)),
        vs_sma50: sma(50).and_then(|a| pct(price, a)),
        from_high: num("fiftyTwoWeekHigh").and_then(|h| pct(price, h)).map(|p| p.min(0.0)),
        volume_ratio,
        spark: closes[closes.len().saturating_sub(SPARK)..].to_vec(),
        history,
    };
    Ok((row, as_of))
}

/// Percent change from `base` to `now`, or `None` for a zero or missing base.
fn pct(now: f64, base: f64) -> Option<f64> {
    (base > 0.0).then(|| (now - base) / base * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chart body with `n` closes rising by 1 from 100, last volume doubled.
    fn body(n: usize, with_change: bool) -> String {
        let closes: Vec<String> = (0..n).map(|i| format!("{}", 100 + i)).collect();
        let mut vols: Vec<String> = (0..n).map(|_| "1000".to_string()).collect();
        if let Some(v) = vols.last_mut() {
            *v = "2000".to_string();
        }
        let price = 100 + n - 1;
        let change = if with_change { r#""regularMarketChangePercent":1.5,"# } else { "" };
        format!(
            r#"{{"chart":{{"result":[{{"meta":{{"regularMarketPrice":{price},{change}
            "regularMarketTime":1700000000,"fiftyTwoWeekHigh":{high},"longName":"Test Ltd"}},
            "indicators":{{"quote":[{{"close":[{c}],"volume":[{v}]}}]}}}}],"error":null}}}}"#,
            high = price + 10,
            c = closes.join(","),
            v = vols.join(","),
        )
    }

    #[test]
    fn returns_and_averages_come_from_the_closes() {
        let (row, t) = parse_row(&body(70, true), "TEST.NS").expect("parse");
        assert_eq!(t, 1700000000);
        assert_eq!(row.symbol, "TEST");
        assert_eq!(row.name, "Test Ltd");
        assert_eq!(row.price, 169.0);
        assert_eq!(row.change_pct, Some(1.5));
        // 169 vs the close five sessions back (164).
        let w = row.ret_1w.unwrap();
        assert!((w - (5.0 / 164.0 * 100.0)).abs() < 1e-9, "{w}");
        assert!(row.ret_3m.is_some());
        // A steady climb sits above both averages.
        assert!(row.vs_sma20.unwrap() > 0.0 && row.vs_sma50.unwrap() > 0.0);
        assert!(row.from_high.unwrap() < 0.0);
        assert_eq!(row.volume_ratio, Some(2.0));
        assert_eq!(row.spark.len(), SPARK);
    }

    #[test]
    fn a_short_history_leaves_the_long_numbers_empty_not_zero() {
        let (row, _) = parse_row(&body(10, false), "NEW.NS").expect("parse");
        assert!(row.ret_1w.is_some());
        assert_eq!(row.ret_1m, None);
        assert_eq!(row.ret_3m, None);
        assert_eq!(row.vs_sma50, None);
        assert_eq!(row.volume_ratio, None);
        // No feed change: derived from the last two closes.
        let c = row.change_pct.unwrap();
        assert!((c - (1.0 / 108.0 * 100.0)).abs() < 1e-9, "{c}");
    }

    #[test]
    fn null_closes_are_dropped_with_their_volumes() {
        let b = r#"{"chart":{"result":[{"meta":{"regularMarketPrice":12},
            "indicators":{"quote":[{"close":[10,null,11,12],"volume":[5,6,null,7]}]}}],"error":null}}"#;
        let (row, _) = parse_row(b, "X.NS").expect("parse");
        assert_eq!(row.spark, vec![10.0, 11.0, 12.0]);
    }

    #[test]
    fn closes_are_dated_by_the_session_day_in_india() {
        // 03:45 UTC on 26 Oct 2026 is 09:15 IST the same day; 20:00 UTC is
        // 01:30 IST the next day.
        let b = r#"{"chart":{"result":[{"meta":{"regularMarketPrice":12,"gmtoffset":19800},
            "timestamp":[1792986300,1793044800],
            "indicators":{"quote":[{"close":[10,11],"volume":[1,1]}]}}],"error":null}}"#;
        let (row, _) = parse_row(b, "X.NS").expect("parse");
        let oct26 = crate::assistant::spend::days_from_civil(2026, 10, 26);
        // The live price replaces the last close.
        assert_eq!(row.history, vec![(oct26, 10.0), (oct26 + 1, 12.0)]);
    }

    #[test]
    fn a_yahoo_error_is_an_error() {
        let b = r#"{"chart":{"result":null,"error":{"description":"No data found, symbol may be delisted"}}}"#;
        assert!(parse_row(b, "GONE.NS").unwrap_err().contains("delisted"));
    }

    #[test]
    fn display_symbols_drop_the_exchange_suffix_only() {
        assert_eq!(display_symbol("M&M.NS"), "M&M");
        assert_eq!(display_symbol("^NSEI"), "^NSEI");
    }

    /// Live, ignored by default. `cargo test -- --ignored --nocapture live_board`.
    #[test]
    #[ignore]
    fn live_board() {
        let b = board(true, 0).expect("board");
        println!("{} stocks, {} skipped {:?}", b.stocks.len(), b.skipped.len(), b.skipped);
        for r in b.indices.iter().chain(b.stocks.iter().take(3)) {
            println!("{} {} {:?} {:?}", r.symbol, r.price, r.change_pct, r.ret_1m);
        }
    }
}
