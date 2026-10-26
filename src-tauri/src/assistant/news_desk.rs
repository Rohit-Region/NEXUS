//! The News section: many feeds by topic, newest first, the user's interests
//! flagged.
//!
//! Built on `news_connector`, which reads one feed. This decides which feeds
//! (a built-in catalogue the user can switch off, plus feeds they add), reads
//! them in parallel, puts every item on one timeline, and marks the ones that
//! mention something the user said they care about.
//!
//! Every built-in feed here was fetched the way NEXUS fetches (no redirects,
//! curl's own identity) and returned current items. Moneycontrol's "latest
//! news" feed was left out: its newest item was from 2024.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::news_connector::{fetch_one, NewsItem};

pub struct Source {
    pub id: &'static str,
    pub label: &'static str,
    pub url: &'static str,
    pub topic: &'static str,
    /// Shown unless the user switches it off.
    pub default_on: bool,
}

pub const TOPICS: &[&str] = &["India", "Business", "World", "Tech", "Sports"];

pub const CATALOGUE: &[Source] = &[
    Source { id: "hindu", label: "The Hindu", url: "https://www.thehindu.com/news/national/feeder/default.rss", topic: "India", default_on: true },
    Source { id: "iexpress", label: "Indian Express", url: "https://indianexpress.com/section/india/feed/", topic: "India", default_on: true },
    Source { id: "ndtv", label: "NDTV", url: "https://feeds.feedburner.com/ndtvnews-top-stories", topic: "India", default_on: true },
    Source { id: "et-markets", label: "ET Markets", url: "https://economictimes.indiatimes.com/markets/rssfeeds/1977021501.cms", topic: "Business", default_on: true },
    Source { id: "mint-markets", label: "Mint Markets", url: "https://www.livemint.com/rss/markets", topic: "Business", default_on: true },
    Source { id: "mint-money", label: "Mint Money", url: "https://www.livemint.com/rss/money", topic: "Business", default_on: true },
    Source { id: "bbc-business", label: "BBC Business", url: "https://feeds.bbci.co.uk/news/business/rss.xml", topic: "Business", default_on: false },
    Source { id: "bbc-world", label: "BBC World", url: "https://feeds.bbci.co.uk/news/world/rss.xml", topic: "World", default_on: true },
    Source { id: "et-tech", label: "ET Tech", url: "https://economictimes.indiatimes.com/tech/rssfeeds/13357270.cms", topic: "Tech", default_on: true },
    Source { id: "inc42", label: "Inc42", url: "https://inc42.com/feed/", topic: "Tech", default_on: false },
    Source { id: "hn", label: "Hacker News", url: "https://hnrss.org/frontpage", topic: "Tech", default_on: true },
    Source { id: "verge", label: "The Verge", url: "https://www.theverge.com/rss/index.xml", topic: "Tech", default_on: true },
    Source { id: "ars", label: "Ars Technica", url: "https://feeds.arstechnica.com/arstechnica/index", topic: "Tech", default_on: false },
    Source { id: "hindu-sport", label: "The Hindu Sport", url: "https://www.thehindu.com/sport/feeder/default.rss", topic: "Sports", default_on: false },
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomFeed {
    pub label: String,
    pub url: String,
    pub topic: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsSettings {
    /// Built-in sources switched on that are off by default.
    #[serde(default)]
    pub enabled: Vec<String>,
    /// Built-in sources switched off that are on by default.
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default)]
    pub custom: Vec<CustomFeed>,
    /// Words to flag, such as "Sensex", "gold", "React".
    #[serde(default)]
    pub interests: Vec<String>,
}

impl NewsSettings {
    pub fn is_on(&self, s: &Source) -> bool {
        if s.default_on {
            !self.disabled.iter().any(|d| d == s.id)
        } else {
            self.enabled.iter().any(|e| e == s.id)
        }
    }

    pub fn sanitised(mut self) -> Self {
        let mut seen = HashSet::new();
        self.interests = self
            .interests
            .into_iter()
            .map(|i| i.trim().to_string())
            .filter(|i| i.chars().count() >= 2 && i.chars().count() <= 40 && seen.insert(i.to_lowercase()))
            .take(30)
            .collect();
        self.custom.truncate(20);
        self
    }
}

/// A custom feed must be an https URL with no spaces, under a name.
pub fn check_custom(feed: &CustomFeed) -> Result<(), String> {
    if feed.label.trim().is_empty() || feed.label.chars().count() > 40 {
        return Err("Give the feed a short name.".to_string());
    }
    let url = feed.url.trim();
    if !url.starts_with("https://") || url.len() < 12 || url.chars().any(char::is_whitespace) {
        return Err("Use the feed's https:// address.".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeskItem {
    pub title: String,
    pub link: String,
    pub source: String,
    pub topic: String,
    /// Unix seconds, when the feed's date could be read.
    pub published: Option<i64>,
    pub summary: Option<String>,
    /// The user's interests this item mentions.
    pub matches: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    pub id: String,
    pub label: String,
    pub topic: String,
    pub on: bool,
    /// On unless switched off (built-in sources only).
    pub default_on: bool,
    pub custom: bool,
    /// Why it returned nothing on the last read, if it did not.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Desk {
    pub items: Vec<DeskItem>,
    pub sources: Vec<SourceView>,
    pub interests: Vec<String>,
    pub topics: Vec<String>,
    /// Unix seconds of this read.
    pub fetched_at: i64,
}

/// One read per this long unless the user presses refresh: feeds update
/// every few minutes at most, and a dozen requests per screen open is waste.
const CACHE_FOR: Duration = Duration::from_secs(600);
/// Most items kept per feed, so a feed of 200 does not drown the timeline.
const PER_FEED: usize = 30;

static CACHE: Mutex<Option<(Instant, String, Desk)>> = Mutex::new(None);

/// Build the desk, from cache when fresh and the settings are unchanged.
pub fn desk(settings: &NewsSettings, refresh: bool, now: i64) -> Desk {
    let key = serde_json::to_string(settings).unwrap_or_default();
    if !refresh {
        if let Some((at, k, d)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if *k == key && at.elapsed() < CACHE_FOR {
                return d.clone();
            }
        }
    }

    // (id, label, url, topic, custom)
    let mut feeds: Vec<(String, String, String, String, bool)> = CATALOGUE
        .iter()
        .filter(|s| settings.is_on(s))
        .map(|s| (s.id.to_string(), s.label.to_string(), s.url.to_string(), s.topic.to_string(), false))
        .collect();
    for (i, c) in settings.custom.iter().enumerate() {
        feeds.push((format!("custom-{i}"), c.label.clone(), c.url.clone(), c.topic.clone(), true));
    }

    // In parallel: a dozen feeds one after another would keep the screen
    // waiting for the slowest of each.
    let results: Vec<Result<Vec<NewsItem>, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = feeds
            .iter()
            .map(|(_, label, url, _, _)| scope.spawn(move || fetch_one(label, url).map_err(|e| e.to_string())))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("Failed".to_string()))).collect()
    });

    let mut errors = std::collections::HashMap::new();
    let mut items = Vec::new();
    for ((id, label, _, topic, _), result) in feeds.iter().zip(results) {
        match result {
            Ok(list) if !list.is_empty() => {
                for n in list.into_iter().take(PER_FEED) {
                    let published = n.published.as_deref().and_then(parse_date);
                    // A feed's own date far in the future is a bad date, not news.
                    let published = published.filter(|p| *p <= now + 3600);
                    let matches = interests_in(&n.title, n.summary.as_deref(), &settings.interests);
                    items.push(DeskItem {
                        title: n.title,
                        link: n.link,
                        source: label.clone(),
                        topic: topic.clone(),
                        published,
                        summary: n.summary,
                        matches,
                    });
                }
            }
            Ok(_) => {
                errors.insert(id.clone(), "No headlines in the feed.".to_string());
            }
            Err(e) => {
                errors.insert(id.clone(), e);
            }
        }
    }
    let items = timeline(items);

    let mut sources: Vec<SourceView> = CATALOGUE
        .iter()
        .map(|s| SourceView {
            id: s.id.to_string(),
            label: s.label.to_string(),
            topic: s.topic.to_string(),
            on: settings.is_on(s),
            default_on: s.default_on,
            custom: false,
            error: errors.get(s.id).cloned(),
        })
        .collect();
    for (i, c) in settings.custom.iter().enumerate() {
        let id = format!("custom-{i}");
        sources.push(SourceView {
            error: errors.get(&id).cloned(),
            id,
            label: c.label.clone(),
            topic: c.topic.clone(),
            on: true,
            default_on: false,
            custom: true,
        });
    }

    let mut topics: Vec<String> = TOPICS.iter().map(|t| t.to_string()).collect();
    for c in &settings.custom {
        if !topics.contains(&c.topic) {
            topics.push(c.topic.clone());
        }
    }

    let d = Desk { items, sources, interests: settings.interests.clone(), topics, fetched_at: now };
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), key, d.clone()));
    d
}

/// Newest first, the same story once: by link, and by title across feeds
/// (one wire story is often carried by several papers).
pub fn timeline(mut items: Vec<DeskItem>) -> Vec<DeskItem> {
    items.sort_by(|a, b| b.published.cmp(&a.published));
    let mut links = HashSet::new();
    let mut titles = HashSet::new();
    items.retain(|i| {
        let t: String = i.title.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
        links.insert(i.link.clone()) && titles.insert(t)
    });
    items
}

/// The interests an item mentions, as whole words, case-insensitive.
pub fn interests_in(title: &str, summary: Option<&str>, interests: &[String]) -> Vec<String> {
    let text = format!("{} {}", title, summary.unwrap_or("")).to_lowercase();
    interests
        .iter()
        .filter(|i| {
            let w = i.to_lowercase();
            text.match_indices(&w).any(|(at, _)| {
                let before = text[..at].chars().next_back();
                let after = text[at + w.len()..].chars().next();
                !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
            })
        })
        .cloned()
        .collect()
}

/// A feed date as unix seconds: RFC 822 ("Sun, 25 Oct 2026 15:32:51 +0530")
/// or RFC 3339 ("2026-10-23T16:42:34-04:00").
pub fn parse_date(raw: &str) -> Option<i64> {
    let s = raw.trim();
    if s.len() >= 19 && s.as_bytes().get(4) == Some(&b'-') && s.as_bytes().get(10).is_some_and(|b| *b == b'T' || *b == b' ') {
        return rfc3339(s);
    }
    rfc822(s)
}

fn civil_to_unix(y: i64, mo: u32, d: u32, h: i64, mi: i64, se: i64) -> Option<i64> {
    if !(1..=12).contains(&mo) || d == 0 || d > super::spend::days_in_month(y, mo) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    Some(super::spend::days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se)
}

fn rfc3339(s: &str) -> Option<i64> {
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: u32 = s.get(5..7)?.parse().ok()?;
    let d: u32 = s.get(8..10)?.parse().ok()?;
    let h: i64 = s.get(11..13)?.parse().ok()?;
    let mi: i64 = s.get(14..16)?.parse().ok()?;
    let se: i64 = s.get(17..19)?.parse().ok()?;
    let mut rest = &s[19..];
    if rest.starts_with('.') {
        rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    }
    let offset = zone(rest)?;
    Some(civil_to_unix(y, mo, d, h, mi, se)? - offset)
}

fn rfc822(s: &str) -> Option<i64> {
    // Drop the weekday, if any: "Sun, 25 Oct 2026 ..." -> "25 Oct 2026 ...".
    let s = s.split_once(',').map(|(_, r)| r).unwrap_or(s).trim();
    let parts: Vec<&str> = s.split_whitespace().collect();
    let d: u32 = parts.first()?.parse().ok()?;
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let mon = parts.get(1)?.to_lowercase();
    let mo = MONTHS.iter().position(|m| mon.starts_with(m))? as u32 + 1;
    let mut y: i64 = parts.get(2)?.parse().ok()?;
    if y < 100 {
        y += 2000;
    }
    let t: Vec<i64> = parts.get(3)?.split(':').filter_map(|p| p.parse().ok()).collect();
    let (h, mi, se) = (*t.first()?, *t.get(1)?, t.get(2).copied().unwrap_or(0));
    let offset = zone(parts.get(4).copied().unwrap_or("+0000"))?;
    Some(civil_to_unix(y, mo, d, h, mi, se)? - offset)
}

/// "+0530", "+05:30", "-04:00", "Z", "GMT", "UTC", "IST" as seconds east of UTC.
fn zone(raw: &str) -> Option<i64> {
    let z = raw.trim();
    match z.to_uppercase().as_str() {
        "" | "Z" | "GMT" | "UT" | "UTC" => return Some(0),
        "IST" => return Some(19_800),
        "EST" => return Some(-18_000),
        "EDT" => return Some(-14_400),
        "PST" => return Some(-28_800),
        "PDT" => return Some(-25_200),
        _ => {}
    }
    let sign = match z.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let digits: String = z[1..].chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 4 {
        return None;
    }
    let h: i64 = digits[..2].parse().ok()?;
    let m: i64 = digits[2..].parse().ok()?;
    Some(sign * (h * 3600 + m * 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_from_every_feed_style_agree() {
        // The same instant, written the ways the catalogue's feeds write it.
        let utc = parse_date("Sun, 25 Oct 2026 10:02:51 +0000").unwrap();
        assert_eq!(parse_date("Sun, 25 Oct 2026 15:32:51 +0530"), Some(utc), "Indian papers");
        assert_eq!(parse_date("2026-10-25T06:02:51-04:00"), Some(utc), "The Verge");
        assert_eq!(parse_date("2026-10-25T10:02:51.000Z"), Some(utc));
        assert_eq!(parse_date("25 Oct 2026 10:02:51 GMT"), Some(utc));
        assert_eq!(parse_date("Sun, 25 Oct 2026 15:32:51 IST"), Some(utc));
        assert_eq!(parse_date("yesterday"), None);
        assert_eq!(parse_date("Sun, 31 Feb 2026 10:00:00 +0000"), None);
    }

    fn item(title: &str, link: &str, published: Option<i64>) -> DeskItem {
        DeskItem {
            title: title.into(),
            link: link.into(),
            source: "S".into(),
            topic: "India".into(),
            published,
            summary: None,
            matches: vec![],
        }
    }

    #[test]
    fn the_timeline_is_newest_first_and_carries_a_story_once() {
        let t = timeline(vec![
            item("Old", "a", Some(1)),
            item("New", "b", Some(3)),
            item("Undated", "c", None),
            item("NEW!", "d", Some(2)),
            item("Again", "b", Some(1)),
        ]);
        let titles: Vec<&str> = t.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["New", "Old", "Undated"], "same link or same title is one story");
    }

    #[test]
    fn interests_match_whole_words() {
        let interests = vec!["gold".to_string(), "React".to_string(), "Sensex".to_string()];
        assert_eq!(interests_in("Gold hits record", None, &interests), vec!["gold".to_string()]);
        assert_eq!(interests_in("Goldman Sachs reacts", None, &interests), Vec::<String>::new());
        assert_eq!(interests_in("Markets", Some("The sensex rose"), &interests), vec!["Sensex".to_string()]);
    }

    #[test]
    fn sources_switch_on_and_off_from_their_defaults() {
        let s = NewsSettings { enabled: vec!["inc42".into()], disabled: vec!["ndtv".into()], ..Default::default() };
        let by = |id: &str| CATALOGUE.iter().find(|c| c.id == id).unwrap();
        assert!(s.is_on(by("inc42")) && !s.is_on(by("ndtv")) && s.is_on(by("hindu")) && !s.is_on(by("ars")));
    }

    /// Live, ignored by default: the default desk against the real feeds.
    /// `cargo test --lib news_desk::tests::live_desk -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_desk() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let settings = NewsSettings { interests: vec!["gold".into(), "Sensex".into()], ..Default::default() };
        let d = desk(&settings, true, now);
        for s in d.sources.iter().filter(|s| s.on) {
            let n = d.items.iter().filter(|i| i.source == s.label).count();
            println!("{:<16} {:>3} items {}", s.label, n, s.error.as_deref().unwrap_or(""));
        }
        let dated = d.items.iter().filter(|i| i.published.is_some()).count();
        println!("total {} items, {} dated, {} match interests", d.items.len(), dated, d.items.iter().filter(|i| !i.matches.is_empty()).count());
        if let Some(i) = d.items.first() {
            println!("newest: [{}] {} ({}s ago)", i.source, i.title, now - i.published.unwrap_or(now));
        }
    }

    #[test]
    fn custom_feeds_must_be_https() {
        let ok = CustomFeed { label: "CSS Tricks".into(), url: "https://css-tricks.com/feed/".into(), topic: "Tech".into() };
        assert!(check_custom(&ok).is_ok());
        assert!(check_custom(&CustomFeed { url: "http://x.com/feed".into(), ..ok.clone() }).is_err());
        assert!(check_custom(&CustomFeed { label: " ".into(), ..ok }).is_err());
    }
}
