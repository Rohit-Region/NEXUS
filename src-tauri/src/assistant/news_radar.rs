//! The news radar: from today's business and tech news to the Indian stocks
//! it is likely to move, with a plan and a prediction for each.
//!
//! # How a radar is built
//!
//! 1. **Read.** The newest business, markets, tech and world headlines from
//!    the News section's own feeds.
//! 2. **Market flow.** Where money is moving: for each NSE industry in the
//!    Midcap 150 and Smallcap 250, the average one-week and one-month return,
//!    volume against usual, and the share of its stocks up this week.
//! 3. **Research.** The headlines, the flows, the stock list and NEXUS's own
//!    track record on past news calls go to a model (Claude, or the local
//!    model when Claude cannot be used), which names up to six events that
//!    should move Indian stocks: what happened, why it matters here, which
//!    way, how soon, how sure, and which sectors and stocks.
//! 4. **Check.** Every sector and stock it names is checked against the real
//!    lists; anything invented is dropped. An event with too few named stocks
//!    is topped up from its sectors: the strongest liquid names for an "up"
//!    call, the weakest for a "down" one.
//! 5. **Plan.** Each stock gets a plan from its own price action (is the news
//!    already priced in, is it still falling, where to buy, where to stop) and
//!    a prediction over the event's horizon, sized by the stock's volatility.
//!
//! Every stock call is written to the pick journal (`news`) and graded like
//! any other suggestion, by whether it moved the way it was called. The
//! model is shown its own hit rate by confidence and sector on every run, so
//! a sector it keeps getting wrong is one it learns to be careful about.
//!
//! # What it cannot do
//!
//! Know what is not yet in the news, or how much of a story the market has
//! already priced. The radar says when a stock has already run, and the
//! journal says how often its calls come true.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::growth_picks::{predict, volatility, Member, Prediction};
use super::pick_journal::Band;
use super::stock_board::Row;
use super::stock_picks::Fundamentals;

/// Headlines older than this are not news for the radar.
const MAX_AGE_HOURS: i64 = 36;
const MAX_HEADLINES: usize = 60;
const MAX_EVENTS: usize = 6;
const STOCKS_PER_EVENT: usize = 5;
const MIN_TURNOVER_CR: f64 = 2.0;

/// A headline as the radar read it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Read {
    pub title: String,
    pub source: String,
    pub link: String,
    pub topic: String,
}

/// Money moving through one industry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SectorFlow {
    pub industry: String,
    pub stocks: usize,
    pub ret_1w: f64,
    pub ret_1m: f64,
    pub volume_ratio: f64,
    /// Share of its stocks up this week, 0 to 1.
    pub breadth: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadarStock {
    pub symbol: String,
    pub name: String,
    pub industry: String,
    /// "up" or "down".
    pub direction: String,
    pub why: String,
    /// Named by the model, or added from a named sector.
    pub named: bool,
    pub price: f64,
    pub ret_1w: Option<f64>,
    pub ret_1m: Option<f64>,
    pub vs_sma50: Option<f64>,
    pub volatility: Option<f64>,
    pub plan: String,
    pub prediction: Option<Prediction>,
    pub fundamentals: Option<Fundamentals>,
    pub held: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub title: String,
    pub summary: String,
    pub why_india: String,
    pub direction: String,
    /// "days", "weeks" or "months".
    pub horizon: String,
    /// "low", "medium" or "high".
    pub confidence: String,
    pub sectors: Vec<String>,
    /// Indexes into `Radar::read` of the headlines behind it.
    pub sources: Vec<usize>,
    pub stocks: Vec<RadarStock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Radar {
    pub events: Vec<Event>,
    /// Strongest inflows first, then the weakest.
    pub inflows: Vec<SectorFlow>,
    pub outflows: Vec<SectorFlow>,
    pub read: Vec<Read>,
    /// Which model did the research.
    pub provider: String,
    pub feedback: Vec<String>,
    pub generated_at: i64,
}

/// Everything the radar works from, gathered by the caller.
pub struct Inputs<'a> {
    pub headlines: Vec<Read>,
    pub members: &'a [Member],
    /// The Nifty 50, which has no NSE industry in our data; Groww's is used.
    pub large: &'a [Row],
    pub prices: &'a [Row],
    pub fundamentals: &'a HashMap<String, Fundamentals>,
    pub feedback: Vec<String>,
    pub held: &'a HashSet<String>,
    pub bands: &'a [Band],
}

/// Newest business, markets, tech and world headlines from the desk.
pub fn headlines(desk: &super::news_desk::Desk, now: i64) -> Vec<Read> {
    let cutoff = now - MAX_AGE_HOURS * 3600;
    desk.items
        .iter()
        // Undated items are kept: the feed did not say, which is not "old".
        .filter(|i| i.published.map_or(true, |p| p >= cutoff))
        .filter(|i| matches!(i.topic.as_str(), "Business" | "Tech" | "World" | "India"))
        .take(MAX_HEADLINES)
        .map(|i| Read { title: i.title.clone(), source: i.source.clone(), link: i.link.clone(), topic: i.topic.clone() })
        .collect()
}

/// Sector flows from the mid and small-cap prices, strongest first.
pub fn sector_flows(members: &[Member], prices: &[Row]) -> Vec<SectorFlow> {
    let industry: HashMap<&str, &str> = members.iter().map(|m| (m.symbol.as_str(), m.industry.as_str())).collect();
    let mut groups: HashMap<&str, Vec<&Row>> = HashMap::new();
    for r in prices {
        if let Some(i) = industry.get(r.symbol.as_str()).filter(|i| !i.is_empty()) {
            groups.entry(i).or_default().push(r);
        }
    }
    let mean = |v: Vec<f64>| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    let mut flows: Vec<SectorFlow> = groups
        .into_iter()
        .filter(|(_, rows)| rows.len() >= 3)
        .map(|(i, rows)| SectorFlow {
            industry: i.to_string(),
            stocks: rows.len(),
            ret_1w: mean(rows.iter().filter_map(|r| r.ret_1w).collect()),
            ret_1m: mean(rows.iter().filter_map(|r| r.ret_1m).collect()),
            volume_ratio: mean(rows.iter().filter_map(|r| r.volume_ratio).collect()),
            breadth: rows.iter().filter(|r| r.ret_1w.is_some_and(|w| w > 0.0)).count() as f64 / rows.len() as f64,
        })
        .collect();
    flows.sort_by(|a, b| b.ret_1w.total_cmp(&a.ret_1w));
    flows
}

/// The research brief. `full` includes the stock list, which a small local
/// model cannot hold in its context; without it the model names sectors and
/// NEXUS picks the stocks.
pub fn prompt(inputs: &Inputs, flows: &[SectorFlow], full: bool) -> String {
    let mut p = String::new();
    p.push_str(
        "You are an equity research analyst covering Indian listed companies (NSE). Read today's headlines and \
         the sector money flows below and find up to 6 events likely to move Indian stocks over the next days, \
         weeks or months: new technologies, product launches, deals, regulation, global company news (for \
         example a Microsoft or Nvidia announcement and what it means for Indian IT services, data centres or \
         chip suppliers), commodity and rate moves. Think through second-order effects: suppliers, customers, \
         competitors. Skip routine market wrap-ups and stories with no Indian listed angle.\n\n",
    );
    p.push_str("HEADLINES (index, topic, source, title):\n");
    for (i, h) in inputs.headlines.iter().enumerate() {
        p.push_str(&format!("{i}. [{}] {}: {}\n", h.topic, h.source, h.title));
    }
    p.push_str("\nSECTOR FLOWS in mid and small caps (industry: 1-week %, 1-month %, volume vs usual, share of stocks up this week):\n");
    for f in flows {
        p.push_str(&format!(
            "{}: {:+.1}%, {:+.1}%, {:.2}x, {:.0}% up\n",
            f.industry,
            f.ret_1w,
            f.ret_1m,
            f.volume_ratio,
            f.breadth * 100.0
        ));
    }
    let industries: Vec<&str> = {
        let mut v: Vec<&str> = inputs.members.iter().map(|m| m.industry.as_str()).collect::<HashSet<_>>().into_iter().collect();
        v.sort_unstable();
        v
    };
    p.push_str(&format!("\nALLOWED SECTORS (use these exact names): {}\n", industries.join("; ")));
    if full {
        p.push_str("\nALLOWED STOCKS (symbol: name, sector). Name only symbols from this list:\n");
        for r in inputs.large {
            let industry = inputs.fundamentals.get(&r.symbol).and_then(|f| f.industry.clone()).unwrap_or_default();
            p.push_str(&format!("{}: {}, {}\n", r.symbol, r.name, industry));
        }
        for m in inputs.members {
            p.push_str(&format!("{}: {}, {}\n", m.symbol, m.name, m.industry));
        }
    }
    if !inputs.feedback.is_empty() {
        p.push_str("\nYOUR TRACK RECORD on past news calls (did the stock beat the Nifty in the direction you called, 20 sessions later). Be more careful where it is weak:\n");
        for line in &inputs.feedback {
            p.push_str(line);
            p.push('\n');
        }
    }
    p.push_str(&format!(
        "\nAnswer with ONLY a JSON object, no prose before or after:\n\
         {{\"events\": [{{\"title\": \"short title\", \"summary\": \"what happened, one sentence\", \
         \"why_india\": \"why it moves Indian stocks, one or two sentences\", \"direction\": \"up\" or \"down\", \
         \"horizon\": \"days\" or \"weeks\" or \"months\", \"confidence\": \"low\" or \"medium\" or \"high\", \
         \"sectors\": [\"allowed sector names\"], \"sources\": [headline indexes], \
         \"stocks\": [{{\"symbol\": \"{}\", \"direction\": \"up\" or \"down\", \"why\": \"one sentence\"}}]}}]}}\n\
         At most {MAX_EVENTS} events and {STOCKS_PER_EVENT} stocks each. If nothing in the news is likely to move \
         Indian stocks, return {{\"events\": []}}.",
        if full { "symbol from the list" } else { "NSE symbol if you are sure, else leave stocks empty" }
    ));
    p
}

#[derive(Debug, Deserialize)]
struct RawEvent {
    title: Option<String>,
    summary: Option<String>,
    why_india: Option<String>,
    direction: Option<String>,
    horizon: Option<String>,
    confidence: Option<String>,
    #[serde(default)]
    sectors: Vec<String>,
    #[serde(default)]
    sources: Vec<serde_json::Value>,
    #[serde(default)]
    stocks: Vec<RawStock>,
}

#[derive(Debug, Deserialize)]
struct RawStock {
    symbol: Option<String>,
    direction: Option<String>,
    why: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawAnswer {
    #[serde(default)]
    events: Vec<RawEvent>,
}

/// The JSON object in a model's reply, tolerating prose or a code fence
/// around it.
fn json_object(reply: &str) -> Option<&str> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    (end > start).then(|| &reply[start..=end])
}

fn one_of(raw: Option<String>, allowed: &[&str], default: &str) -> String {
    raw.map(|s| s.trim().to_lowercase())
        .filter(|s| allowed.contains(&s.as_str()))
        .unwrap_or_else(|| default.to_string())
}

/// Turn a reply into checked events with planned stocks.
pub fn build(reply: &str, inputs: &Inputs, flows: &[SectorFlow]) -> Result<Vec<Event>, String> {
    let raw: RawAnswer = serde_json::from_str(json_object(reply).ok_or("The model's answer had no JSON in it.")?)
        .map_err(|e| format!("The model's answer was not the expected JSON: {e}"))?;

    let price: HashMap<&str, &Row> = inputs.prices.iter().chain(inputs.large).map(|r| (r.symbol.as_str(), r)).collect();
    let member: HashMap<&str, &Member> = inputs.members.iter().map(|m| (m.symbol.as_str(), m)).collect();
    let sectors: HashSet<&str> = inputs.members.iter().map(|m| m.industry.as_str()).collect();
    let flow: HashMap<&str, &SectorFlow> = flows.iter().map(|f| (f.industry.as_str(), f)).collect();

    let industry_of = |symbol: &str| -> String {
        member
            .get(symbol)
            .map(|m| m.industry.clone())
            .or_else(|| inputs.fundamentals.get(symbol).and_then(|f| f.industry.clone()))
            .unwrap_or_default()
    };

    let mut events = Vec::new();
    for e in raw.events.into_iter().take(MAX_EVENTS) {
        let direction = one_of(e.direction, &["up", "down"], "up");
        let horizon = one_of(e.horizon, &["days", "weeks", "months"], "weeks");
        let confidence = one_of(e.confidence, &["low", "medium", "high"], "low");
        let event_sectors: Vec<String> =
            e.sectors.into_iter().filter(|s| sectors.contains(s.as_str())).take(3).collect();

        let mut seen = HashSet::new();
        let mut picks: Vec<(String, String, String, bool)> = Vec::new();
        for s in e.stocks {
            let Some(symbol) = s.symbol.map(|x| x.trim().to_uppercase()) else { continue };
            if !price.contains_key(symbol.as_str()) || !seen.insert(symbol.clone()) {
                continue;
            }
            let d = one_of(s.direction, &["up", "down"], &direction);
            picks.push((symbol, d, s.why.unwrap_or_default(), true));
        }
        // Top up from the named sectors with the most liquid names moving the
        // called way, so an event is never just one stock.
        for sector in &event_sectors {
            if picks.len() >= STOCKS_PER_EVENT {
                break;
            }
            let mut in_sector: Vec<&Row> = inputs
                .prices
                .iter()
                .filter(|r| member.get(r.symbol.as_str()).is_some_and(|m| &m.industry == sector))
                .filter(|r| r.turnover_cr.is_some_and(|t| t >= MIN_TURNOVER_CR))
                .filter(|r| !seen.contains(&r.symbol))
                .collect();
            in_sector.sort_by(|a, b| {
                let k = |r: &Row| r.ret_1m.unwrap_or(0.0);
                if direction == "up" { k(b).total_cmp(&k(a)) } else { k(a).total_cmp(&k(b)) }
            });
            for r in in_sector.into_iter().take(2) {
                if picks.len() >= STOCKS_PER_EVENT {
                    break;
                }
                seen.insert(r.symbol.clone());
                picks.push((r.symbol.clone(), direction.clone(), format!("In {sector}, which this news affects"), false));
            }
        }
        if picks.is_empty() {
            continue;
        }

        let sessions = match horizon.as_str() {
            "days" => 5,
            "months" => 60,
            _ => 20,
        };
        let stocks = picks
            .into_iter()
            .take(STOCKS_PER_EVENT)
            .filter_map(|(symbol, d, why, named)| {
                let r = price.get(symbol.as_str())?;
                let industry = industry_of(&symbol);
                let held = inputs.held.contains(&symbol);
                let vol = volatility(r);
                let score = confidence_score(&confidence, &d);
                let prediction = vol.map(|(_, daily)| predict(r.price, score, daily, sessions, inputs.bands));
                Some(RadarStock {
                    plan: plan(r, &d, held, prediction.as_ref(), flow.get(industry.as_str()).copied()),
                    name: inputs.fundamentals.get(&symbol).and_then(|f| f.display_name.clone()).unwrap_or_else(|| r.name.clone()),
                    symbol,
                    industry,
                    direction: d,
                    why,
                    named,
                    price: r.price,
                    ret_1w: r.ret_1w,
                    ret_1m: r.ret_1m,
                    vs_sma50: r.vs_sma50,
                    volatility: vol.map(|v| v.0),
                    prediction,
                    fundamentals: None,
                    held,
                })
            })
            .collect();

        events.push(Event {
            title: e.title.unwrap_or_else(|| "Untitled event".into()),
            summary: e.summary.unwrap_or_default(),
            why_india: e.why_india.unwrap_or_default(),
            direction,
            horizon,
            confidence,
            sectors: event_sectors,
            sources: e
                .sources
                .iter()
                .filter_map(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
                .map(|i| i as usize)
                .filter(|i| *i < inputs.headlines.len())
                .collect(),
            stocks,
        });
    }
    Ok(events)
}

/// A confidence and direction as a score for the journal and the prediction:
/// high "up" 80, low "up" 58; "down" mirrored below 50.
pub fn confidence_score(confidence: &str, direction: &str) -> f64 {
    let strength = match confidence {
        "high" => 30.0,
        "medium" => 18.0,
        _ => 8.0,
    };
    if direction == "down" { 50.0 - strength } else { 50.0 + strength }
}

/// What to do about one stock, given the call and its own price action.
fn plan(r: &Row, direction: &str, held: bool, p: Option<&Prediction>, flow: Option<&SectorFlow>) -> String {
    let sma20 = r.vs_sma20.map(|v| r.price / (1.0 + v / 100.0));
    if direction == "down" {
        return if held {
            format!(
                "You hold this. Consider trimming or setting a stop near ₹{:.0}; revisit once the news settles.",
                p.map(|p| p.stop).unwrap_or(r.price * 0.93)
            )
        } else {
            "Avoid for now. If you want it, wait until the price stops falling after the news.".to_string()
        };
    }
    if r.ret_1w.is_some_and(|w| w > 8.0) {
        return format!(
            "Already up {:.1}% this week: the news may be priced in. Wait for a dip toward ₹{:.0} (20-day average).",
            r.ret_1w.unwrap_or(0.0),
            sma20.unwrap_or(r.price * 0.95)
        );
    }
    if r.vs_sma50.is_some_and(|v| v < -10.0) {
        return "Still in a downtrend. Good news alone rarely turns it; wait for the price to climb back above its 50-day average.".to_string();
    }
    let flowing = flow.is_some_and(|f| f.ret_1w > 0.0 && f.breadth > 0.55);
    match p {
        Some(p) => {
            // A dip entry only when it sits above the stop; otherwise the
            // "better price" would already be a stopped-out trade.
            let dip = sma20
                .filter(|s| *s < r.price && *s > p.stop)
                .map(|s| format!(", or better on a dip to ₹{s:.0}"))
                .unwrap_or_default();
            format!(
                "Buy around ₹{:.0}{dip}; stop-loss ₹{:.0}, target ₹{:.0} over {} sessions.{}",
                r.price,
                p.stop,
                p.target,
                p.sessions,
                if flowing { " Its sector already has money flowing in." } else { "" }
            )
        }
        None => "Too little price history to plan a trade; watch it.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(symbol: &str, price: f64, ret_1w: f64, ret_1m: f64, turnover: f64) -> Row {
        // 61 sessions alternating ±1%, enough for a volatility.
        let mut p = price;
        let history = (0..61)
            .map(|d| {
                p *= if d % 2 == 0 { 1.01 } else { 1.0 / 1.01 };
                (d, p)
            })
            .collect();
        Row {
            symbol: symbol.into(),
            name: format!("{symbol} Ltd"),
            price,
            change_pct: Some(0.5),
            ret_1w: Some(ret_1w),
            ret_1m: Some(ret_1m),
            ret_3m: None,
            vs_sma20: Some(1.0),
            vs_sma50: Some(2.0),
            from_high: None,
            volume_ratio: Some(1.2),
            turnover_cr: Some(turnover),
            spark: vec![],
            history,
        }
    }

    fn member(symbol: &str, industry: &str) -> Member {
        Member { symbol: symbol.into(), name: format!("{symbol} Ltd"), industry: industry.into(), index: "Smallcap 250" }
    }

    #[test]
    fn replies_are_checked_against_the_real_lists() {
        let members = vec![
            member("AAA", "Information Technology"),
            member("BBB", "Information Technology"),
            member("CCC", "Information Technology"),
            member("DDD", "Healthcare"),
        ];
        let prices = vec![
            row("AAA", 100.0, 2.0, 5.0, 10.0),
            row("BBB", 200.0, 1.0, 9.0, 10.0),
            row("CCC", 50.0, 1.0, 1.0, 0.5),
            row("DDD", 80.0, -1.0, -2.0, 10.0),
        ];
        let large = vec![];
        let held = HashSet::from(["AAA".to_string()]);
        let fundamentals = HashMap::new();
        let inputs = Inputs {
            headlines: vec![Read { title: "AI deal".into(), source: "X".into(), link: "l".into(), topic: "Tech".into() }],
            members: &members,
            large: &large,
            prices: &prices,
            fundamentals: &fundamentals,
            feedback: vec![],
            held: &held,
            bands: &[],
        };
        let reply = r#"Sure! ```json
        {"events": [{"title": "Big AI deal", "summary": "s", "why_india": "w", "direction": "UP",
          "horizon": "weeks", "confidence": "high", "sectors": ["Information Technology", "Made Up Sector"],
          "sources": [0, "0", 7],
          "stocks": [{"symbol": "aaa", "direction": "up", "why": "named"}, {"symbol": "FAKE", "why": "invented"}]}]}
        ```"#;
        let flows = sector_flows(&members, &prices);
        let events = build(reply, &inputs, &flows).expect("build");
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!((e.direction.as_str(), e.confidence.as_str()), ("up", "high"));
        // The invented sector and stock are dropped; out-of-range sources too.
        assert_eq!(e.sectors, vec!["Information Technology"]);
        assert_eq!(e.sources, vec![0, 0]);
        let symbols: Vec<&str> = e.stocks.iter().map(|s| s.symbol.as_str()).collect();
        // AAA as named, then BBB from the sector; CCC is too illiquid.
        assert_eq!(symbols, vec!["AAA", "BBB"]);
        assert!(e.stocks[0].named && !e.stocks[1].named);
        assert!(e.stocks[0].held);
        assert!(e.stocks[1].prediction.as_ref().is_some_and(|p| p.expected > 0.0));
    }

    #[test]
    fn a_reply_without_json_is_an_error_not_a_panic() {
        let members = vec![];
        let fundamentals = HashMap::new();
        let held = HashSet::new();
        let inputs = Inputs {
            headlines: vec![],
            members: &members,
            large: &[],
            prices: &[],
            fundamentals: &fundamentals,
            feedback: vec![],
            held: &held,
            bands: &[],
        };
        assert!(build("I could not find anything.", &inputs, &[]).is_err());
        assert_eq!(build(r#"{"events": []}"#, &inputs, &[]).unwrap().len(), 0);
    }

    /// Live, ignored by default: reads the news, asks Claude, prints the radar.
    /// `cargo test -- --ignored --nocapture live_radar`.
    #[test]
    #[ignore]
    fn live_radar() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let desk = super::super::news_desk::desk(&Default::default(), false, now);
        let headlines = headlines(&desk, now);
        let members = super::super::growth_picks::members();
        let prices = super::super::growth_picks::prices(false).expect("prices");
        let board = super::super::stock_board::board(false, now).expect("board");
        let fundamentals = HashMap::new();
        let held = HashSet::new();
        let flows = sector_flows(&members, &prices.rows);
        let inputs = Inputs {
            headlines,
            members: &members,
            large: &board.stocks,
            prices: &prices.rows,
            fundamentals: &fundamentals,
            feedback: vec![],
            held: &held,
            bands: &[],
        };
        let brief = prompt(&inputs, &flows, true);
        println!("{} headlines, prompt {} chars; top flows {:?}", inputs.headlines.len(), brief.len(),
            flows.iter().take(3).map(|f| (&f.industry, (f.ret_1w * 10.0).round() / 10.0)).collect::<Vec<_>>());
        let t = std::time::Instant::now();
        let reply = super::super::claude_provider::complete(&brief, std::time::Duration::from_secs(180)).expect("claude");
        let events = build(&reply, &inputs, &flows).expect("build");
        println!("{:?}, {} events", t.elapsed(), events.len());
        for e in &events {
            println!("\n# {} [{} {} {}] sectors {:?}\n  {}\n  {}", e.title, e.direction, e.horizon, e.confidence, e.sectors, e.summary, e.why_india);
            for s in &e.stocks {
                let p = s.prediction.as_ref();
                println!("  - {} {} ({}) named={} exp {:?}: {} | {}", s.symbol, s.direction, s.industry, s.named,
                    p.map(|p| (p.expected * 10.0).round() / 10.0), s.why, s.plan);
            }
        }
    }

    #[test]
    fn confidence_maps_symmetrically_around_neutral() {
        assert_eq!(confidence_score("high", "up"), 80.0);
        assert_eq!(confidence_score("high", "down"), 20.0);
        assert_eq!(confidence_score("low", "up"), 58.0);
    }

    #[test]
    fn plans_warn_when_the_news_is_priced_in() {
        let hot = row("A", 100.0, 12.0, 20.0, 10.0);
        assert!(plan(&hot, "up", false, None, None).contains("priced in"));
        let calm = row("B", 100.0, 1.0, 3.0, 10.0);
        let p = predict(100.0, 80.0, 0.02, 20, &[]);
        let text = plan(&calm, "up", false, Some(&p), None);
        assert!(text.starts_with("Buy around"), "{text}");
        // A dip level below the stop is never offered as an entry.
        let mut sagging = row("C", 100.0, 1.0, 3.0, 10.0);
        sagging.vs_sma20 = Some(40.0);
        assert!(!plan(&sagging, "up", false, Some(&p), None).contains("dip to"));
        assert!(plan(&calm, "down", true, Some(&p), None).starts_with("You hold this"));
    }
}
