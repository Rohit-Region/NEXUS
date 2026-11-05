//! IPOs: the mainboard issues open now or opening soon, with the numbers that
//! best predict a listing-day gain, and the investor's own allotments.
//!
//! There is no source NEXUS can read directly: NSE refuses scripted requests,
//! and grey market premiums (GMP) are an informal number that only trackers
//! publish. So Claude researches the issues on the web and returns facts
//! only. The verdict is not the model's: it is [`verdict`], a fixed rule over
//! those facts, so every call says how it was made and can be checked.

use serde::{Deserialize, Serialize};

use super::market_connector;

/// One IPO, as researched.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ipo {
    pub name: String,
    /// "open" or "upcoming".
    pub status: String,
    /// Dates as the model found them, `YYYY-MM-DD`; empty when not announced.
    pub open_date: String,
    pub close_date: String,
    pub listing_date: String,
    pub price_low: Option<f64>,
    pub price_high: Option<f64>,
    pub lot_size: Option<u32>,
    pub issue_size_cr: Option<f64>,
    /// Grey market premium in rupees per share.
    pub gmp: Option<f64>,
    /// GMP as a share of the upper price band.
    pub gmp_pct: Option<f64>,
    /// Times subscribed overall and by qualified institutional buyers.
    pub subscribed: Option<f64>,
    pub qib: Option<f64>,
    /// "apply", "consider", "skip" or "watch"; see [`verdict`].
    pub verdict: String,
    /// The rule's reason, then the model's one-line read of the business.
    pub why: String,
    pub business: String,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpoDesk {
    pub ipos: Vec<Ipo>,
    /// [`RULE`], so the screen states the rule the verdicts came from.
    pub rule: String,
    pub provider: String,
    pub generated_at: i64,
}

/// An IPO the investor was allotted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Allotment {
    pub id: i64,
    pub name: String,
    /// The NSE symbol once known, without `.NS`; empty before listing.
    pub symbol: String,
    pub shares: u32,
    pub issue_price: f64,
}

/// An allotment priced against the market.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllotmentRow {
    #[serde(flatten)]
    pub allotment: Allotment,
    pub price: Option<f64>,
    pub gain: Option<f64>,
    pub gain_pct: Option<f64>,
    /// Why there is no price, when there is none.
    pub note: Option<String>,
}

/// The rule behind every verdict, shown above the list.
pub const RULE: &str = "Apply: GMP at least 15% of the upper band, and institutions (QIB) at least 10x subscribed \
     or not yet known. Consider: GMP 5 to 15%. Skip: GMP under 5%, or QIB under 3x once bidding has closed. \
     Watch: no GMP yet.";

/// The verdict and its reason, from the numbers alone.
///
/// GMP is the strongest public signal of listing-day demand; QIB demand is
/// the check on it, since institutions do their own diligence and a hyped
/// GMP without them tends to fade by listing.
pub fn verdict(gmp_pct: Option<f64>, qib: Option<f64>, status: &str) -> (&'static str, String) {
    let Some(g) = gmp_pct else {
        return ("watch", "No grey market premium is reported yet.".to_string());
    };
    if let Some(q) = qib.filter(|_| status != "upcoming") {
        if q < 3.0 {
            return ("skip", format!("Institutions bid only {q:.1}x, so the GMP of {g:.0}% is not backed."));
        }
    }
    if g >= 15.0 && qib.map_or(true, |q| q >= 10.0) {
        let backing = qib.map(|q| format!(" and institutions bid {q:.0}x")).unwrap_or_default();
        return ("apply", format!("GMP is {g:.0}% of the price{backing}."));
    }
    if g >= 5.0 {
        return ("consider", format!("GMP is {g:.0}%, a modest listing gain at best."));
    }
    ("skip", format!("GMP is {g:.0}%, too little to cover the risk of a flat listing."))
}

/// A day number (days since 1970-01-01) as `YYYY-MM-DD`, by Howard Hinnant's
/// civil-from-days algorithm.
pub fn ymd(day: i64) -> String {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The research brief. Facts only: the verdict is ours.
pub fn prompt(today: &str) -> String {
    format!(
        "Today is {today}. Search the web for Indian mainboard IPOs (NSE/BSE, not SME) that are open for \
bidding now or open within the next 14 days. For each, find the price band, lot size, issue size, dates, \
the latest grey market premium (GMP) in rupees, and, if bidding has started, the latest overall and QIB \
subscription (times). Prefer chittorgarh.com, investorgain.com, ipowatch.in, moneycontrol.com and the \
exchanges; when trackers disagree on GMP, use the lower figure. Use null for anything you cannot find; never guess.

Answer with only this JSON, no prose:
{{\"ipos\": [{{\"name\": \"\", \"status\": \"open|upcoming\", \"openDate\": \"YYYY-MM-DD\", \"closeDate\": \"YYYY-MM-DD\", \
\"listingDate\": \"YYYY-MM-DD\", \"priceLow\": 0, \"priceHigh\": 0, \"lotSize\": 0, \"issueSizeCr\": 0, \"gmp\": 0, \
\"subscribed\": 0, \"qib\": 0, \"business\": \"one sentence: what the company does and one strength or risk\", \
\"sources\": [\"https://...\"]}}]}}"
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawIpo {
    name: Option<String>,
    status: Option<String>,
    open_date: Option<String>,
    close_date: Option<String>,
    listing_date: Option<String>,
    price_low: Option<f64>,
    price_high: Option<f64>,
    lot_size: Option<f64>,
    issue_size_cr: Option<f64>,
    gmp: Option<f64>,
    subscribed: Option<f64>,
    qib: Option<f64>,
    business: Option<String>,
    #[serde(default)]
    sources: Vec<String>,
}

#[derive(Deserialize)]
struct RawAnswer {
    ipos: Vec<RawIpo>,
}

/// The JSON object in a model's reply, tolerating prose or a code fence
/// around it.
fn json_object(reply: &str) -> Option<&str> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    (end > start).then(|| &reply[start..=end])
}

/// Models write `0` where they mean "not found", and a zero price, lot or
/// subscription is never a real reading. GMP is the exception: trackers do
/// report a GMP of zero, so it is filtered separately.
fn positive(n: Option<f64>) -> Option<f64> {
    n.filter(|v| v.is_finite() && *v > 0.0)
}

fn date(raw: Option<String>) -> String {
    raw.map(|s| s.trim().to_string())
        .filter(|s| s.len() == 10 && s.chars().all(|c| c.is_ascii_digit() || c == '-'))
        .unwrap_or_default()
}

/// Turn a reply into checked IPOs, best verdict and earliest close first.
pub fn build(reply: &str) -> Result<Vec<Ipo>, String> {
    let raw: RawAnswer = serde_json::from_str(json_object(reply).ok_or("The model's answer had no JSON in it.")?)
        .map_err(|e| format!("The model's answer was not the expected JSON: {e}"))?;

    let mut ipos: Vec<Ipo> = raw
        .ipos
        .into_iter()
        .filter_map(|r| {
            let name = r.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty())?;
            let status = match r.status.as_deref().map(str::trim) {
                Some("open") => "open",
                _ => "upcoming",
            };
            let price_high = positive(r.price_high);
            let gmp = r.gmp.filter(|v| v.is_finite() && *v >= 0.0);
            let gmp_pct = gmp.zip(price_high).map(|(g, p)| g / p * 100.0);
            let qib = positive(r.qib);
            let (verdict, why) = verdict(gmp_pct, qib, status);
            Some(Ipo {
                name,
                status: status.to_string(),
                open_date: date(r.open_date),
                close_date: date(r.close_date),
                listing_date: date(r.listing_date),
                price_low: positive(r.price_low),
                price_high,
                lot_size: positive(r.lot_size).map(|n| n.round() as u32),
                issue_size_cr: positive(r.issue_size_cr),
                gmp,
                gmp_pct,
                subscribed: positive(r.subscribed),
                qib,
                verdict: verdict.to_string(),
                why,
                business: r.business.unwrap_or_default().trim().to_string(),
                // Only links the UI can open safely, and few enough to read.
                sources: r.sources.into_iter().filter(|s| s.starts_with("https://")).take(3).collect(),
            })
        })
        .collect();

    let rank = |v: &str| ["apply", "consider", "watch", "skip"].iter().position(|x| *x == v).unwrap_or(4);
    ipos.sort_by(|a, b| {
        rank(&a.verdict)
            .cmp(&rank(&b.verdict))
            .then_with(|| a.close_date.cmp(&b.close_date))
    });
    Ok(ipos)
}

/// Price each allotment that has a symbol. A failed quote is a note on that
/// row, not an error for the list.
pub fn price(allotments: Vec<Allotment>) -> Vec<AllotmentRow> {
    allotments
        .into_iter()
        .map(|a| {
            if a.symbol.is_empty() {
                return AllotmentRow {
                    allotment: a,
                    price: None,
                    gain: None,
                    gain_pct: None,
                    note: Some("Add the NSE symbol once it lists to see the price.".to_string()),
                };
            }
            match market_connector::fetch_quote(&format!("{}.NS", a.symbol)) {
                Ok(q) => {
                    let gain = (q.price - a.issue_price) * f64::from(a.shares);
                    let gain_pct = (q.price / a.issue_price - 1.0) * 100.0;
                    AllotmentRow { allotment: a, price: Some(q.price), gain: Some(gain), gain_pct: Some(gain_pct), note: None }
                }
                Err(e) => AllotmentRow { allotment: a, price: None, gain: None, gain_pct: None, note: Some(e) },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_strong_gmp_backed_by_institutions_is_apply() {
        assert_eq!(verdict(Some(22.0), Some(40.0), "open").0, "apply");
        assert_eq!(verdict(Some(22.0), None, "upcoming").0, "apply");
    }

    #[test]
    fn a_strong_gmp_without_institutions_is_not_apply() {
        assert_eq!(verdict(Some(25.0), Some(2.0), "open").0, "skip");
        assert_eq!(verdict(Some(25.0), Some(6.0), "open").0, "consider");
    }

    #[test]
    fn weak_or_missing_gmp() {
        assert_eq!(verdict(Some(8.0), None, "open").0, "consider");
        assert_eq!(verdict(Some(1.0), None, "open").0, "skip");
        assert_eq!(verdict(None, Some(50.0), "open").0, "watch");
    }

    #[test]
    fn a_reply_is_parsed_checked_and_ranked() {
        let reply = r#"Here you go:
```json
{"ipos": [
  {"name": "Flat Co", "status": "open", "closeDate": "2026-10-29", "priceHigh": 100, "gmp": 2, "sources": ["http://x", "https://a"]},
  {"name": "Hot Co", "status": "open", "closeDate": "2026-10-30", "priceLow": 0, "priceHigh": 150, "gmp": 30, "qib": 0, "lotSize": 100},
  {"name": "", "status": "open"}
]}
```"#;
        let ipos = build(reply).unwrap();
        assert_eq!(ipos.len(), 2, "a nameless entry is dropped");
        assert_eq!(ipos[0].name, "Hot Co");
        assert_eq!(ipos[0].verdict, "apply");
        assert_eq!(ipos[0].gmp_pct, Some(20.0));
        assert_eq!(ipos[0].price_low, None, "a zero price is not a reading");
        assert_eq!(ipos[0].qib, None, "a zero QIB is not a reading");
        assert_eq!(ipos[1].sources, vec!["https://a"], "plain http links are dropped");
    }

    #[test]
    fn day_numbers_become_dates() {
        assert_eq!(ymd(0), "1970-01-01");
        assert_eq!(ymd(11_016), "2000-02-29");
        assert_eq!(ymd(20_753), "2026-10-27");
    }

    #[test]
    fn a_reply_without_json_is_an_error() {
        assert!(build("No IPOs this week.").is_err());
    }
}
