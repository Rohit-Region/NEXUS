//! Indian mutual-fund NAVs, for the Money section.
//!
//! # Source
//!
//! AMFI publishes every Indian mutual fund's NAV daily, for free. This reads it
//! through `api.mfapi.in`, a long-standing free mirror that serves it per
//! scheme as JSON and offers search by name, which the raw AMFI text file does
//! not. AMFI is the source of truth; the mirror is the convenient shape. If the
//! mirror ever goes away, the fallback is AMFI's own `NAVAll.txt`.
//!
//! # Why a mutual fund is not a stock here
//!
//! A fund has no live intraday price: it is valued once a day at its NAV. So it
//! is priced by scheme code against the day's NAV, not by a ticker against a
//! market feed. Holding it separately keeps that honest, a fund's "day change"
//! is yesterday's NAV to today's, not a live tick.
//!
//! # What it does not do
//!
//! Advice, or transactions. It fetches a NAV. It cannot buy, sell, or rate a
//! fund; fund ratings live in `fund_picks`, for the Stocks section.

use serde::Serialize;

use super::http::{send, HttpError, Request};

/// A fund the user might be looking for.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FundMatch {
    /// AMFI scheme code, the stable id used to price it later.
    pub code: String,
    pub name: String,
}

/// A fund's latest valuation.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MfNav {
    pub code: String,
    pub name: String,
    pub nav: f64,
    /// The previous published NAV, for the day's change. `None` when the fund
    /// has only one data point.
    pub prev_nav: Option<f64>,
    /// The date of the latest NAV, as AMFI wrote it (dd-mm-yyyy).
    pub date: String,
}

const BASE: &str = "https://api.mfapi.in/mf";

/// A scheme code cleaned to digits, or `None`. Codes are numeric; anything else
/// would be a malformed request, not a lookup.
pub fn valid_code(raw: &str) -> Option<String> {
    let c = raw.trim();
    if c.is_empty() || c.len() > 12 || !c.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(c.to_string())
}

/// Search funds by name. Returns the top matches so the user picks the exact
/// plan (Direct vs Regular, Growth vs IDCW all share a name but differ).
pub fn search(query: &str) -> Result<Vec<FundMatch>, String> {
    let q = query.trim();
    if q.len() < 3 {
        return Err("Type at least three letters to search funds.".to_string());
    }
    let url = format!("{BASE}/search?q={}", encode(q));
    let resp = fetch(&url)?;
    let value: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("Unreadable fund list: {e}"))?;
    let arr = value.as_array().ok_or("The fund search returned nothing usable.")?;
    let out: Vec<FundMatch> = arr
        .iter()
        .filter_map(|v| {
            let code = v.get("schemeCode")?;
            // schemeCode comes as a number; render it as the string code.
            let code = code.as_i64().map(|n| n.to_string()).or_else(|| {
                code.as_str().map(|s| s.to_string())
            })?;
            let name = v.get("schemeName")?.as_str()?.to_string();
            Some(FundMatch { code, name })
        })
        .take(25)
        .collect();
    Ok(out)
}

/// The latest NAV for a scheme code.
pub fn fetch_nav(code: &str) -> Result<MfNav, String> {
    let code = valid_code(code).ok_or_else(|| format!("\"{code}\" is not a scheme code."))?;
    let url = format!("{BASE}/{code}");
    let body = fetch(&url)?;
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("Unreadable NAV data: {e}"))?;

    // mfapi reports a bad code as status "FAIL" with an empty data array.
    if value.get("status").and_then(|s| s.as_str()) == Some("FAIL") {
        return Err(format!("No fund with scheme code {code}."));
    }

    let name = value
        .pointer("/meta/scheme_name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();

    let data = value
        .get("data")
        .and_then(|d| d.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| format!("No NAV history for scheme {code}."))?;

    let latest = &data[0];
    let nav = latest
        .get("nav")
        .and_then(|n| n.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .ok_or_else(|| format!("No NAV for scheme {code}."))?;
    let date = latest
        .get("date")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();

    let prev_nav = data
        .get(1)
        .and_then(|p| p.get("nav"))
        .and_then(|n| n.as_str())
        .and_then(|s| s.parse::<f64>().ok());

    Ok(MfNav {
        code,
        name,
        nav,
        prev_nav,
        date,
    })
}

/// Daily NAVs from `since_day` (a day number) to today, oldest first, as
/// `(day number, nav)`. For checking how a past fund suggestion turned out.
pub fn nav_history(code: &str, since_day: i64) -> Result<Vec<(i64, f64)>, String> {
    let code = valid_code(code).ok_or_else(|| format!("\"{code}\" is not a scheme code."))?;
    let (y, m, d) = civil_from_days(since_day);
    // Bounded by a start date: a fund's full history can run past the
    // response cap.
    let body = fetch(&format!("{BASE}/{code}?startDate={d:02}-{m:02}-{y}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("Unreadable NAV data: {e}"))?;
    let mut out: Vec<(i64, f64)> = value
        .get("data")
        .and_then(|d| d.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let date = r.get("date")?.as_str()?;
                    let nav = r.get("nav")?.as_str()?.parse::<f64>().ok()?;
                    Some((day_of_date(date)?, nav))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|r| r.0);
    Ok(out)
}

/// "26-10-2026" as a day number.
fn day_of_date(raw: &str) -> Option<i64> {
    let mut parts = raw.split('-');
    let d: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let y: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || d == 0 || d > 31 {
        return None;
    }
    Some(super::spend::days_from_civil(y, m, d))
}

/// A day number as (year, month, day); the inverse of `days_from_civil`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn fetch(url: &str) -> Result<String, String> {
    let request = Request::get(url).with_header("User-Agent", "Mozilla/5.0 (NEXUS)");
    let resp = send(request).map_err(|e: HttpError| match e {
        HttpError::Unreachable { detail } => format!("Could not reach the fund data: {detail}"),
        other => other.to_string(),
    })?;
    if !resp.ok() {
        return Err(format!("Fund data returned status {}.", resp.status));
    }
    Ok(resp.body)
}

/// Percent-encode a query for the URL. Only the characters a search term
/// actually uses need handling; anything outside the safe set becomes %XX.
fn encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_numbers_round_trip_through_amfi_dates() {
        let day = day_of_date("26-10-2026").expect("date");
        assert_eq!(civil_from_days(day), (2026, 10, 26));
        assert_eq!(civil_from_days(day_of_date("29-02-2024").unwrap()), (2024, 2, 29));
        assert_eq!(day_of_date("2026-10-26"), None);
    }

    #[test]
    fn scheme_codes_are_digits_only() {
        assert_eq!(valid_code(" 135797 ").as_deref(), Some("135797"));
        for bad in ["", "abc", "12a", "12.3", "   "] {
            assert!(valid_code(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn a_search_term_is_url_encoded() {
        assert_eq!(encode("tata digital"), "tata%20digital");
        assert_eq!(encode("s&p 500"), "s%26p%20500");
    }

    /// Live, ignored by default. `cargo test -- --ignored --nocapture live_mf`.
    #[test]
    #[ignore]
    fn live_mf() {
        match search("tata digital") {
            Ok(matches) => {
                for m in matches.iter().take(3) {
                    println!("{} - {}", m.code, m.name);
                }
                if let Some(first) = matches.first() {
                    match fetch_nav(&first.code) {
                        Ok(nav) => println!("NAV {} on {} ({})", nav.nav, nav.date, nav.name),
                        Err(e) => println!("nav err: {e}"),
                    }
                }
            }
            Err(e) => println!("search err: {e}"),
        }
    }
}
