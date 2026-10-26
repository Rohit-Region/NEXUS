//! Informative news, from public RSS/Atom feeds.
//!
//! # Why RSS, and why hand-parsed
//!
//! News is the one assistant pillar that needs no account, no key, and no
//! personal data: a feed is a public URL that answers with a list of
//! headlines. That keeps "informative news" free and private, which is the
//! whole reason it is the first pillar built.
//!
//! The XML is parsed by hand rather than with a crate. A feed reader pulls a
//! title, a link, and a date out of a handful of `<item>`/`<entry>` blocks;
//! that is not worth the 10-plus crates a full XML stack drags in, the same
//! trade [`super::http`] makes against `reqwest`. The parse is deliberately
//! forgiving: feeds are third-party markup that varies, so this reads what it
//! recognises and skips what it does not, rather than failing a whole feed over
//! one odd entry.
//!
//! # What it does not do
//!
//! Reads. It fetches public feeds over HTTPS and returns headlines. It sends
//! nothing about the user anywhere; a feed request carries no identity.

use serde::Serialize;

use super::http::{send, HttpError, Request};

/// One headline, flattened to what a briefing shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsItem {
    pub title: String,
    pub link: String,
    /// The feed's own label, so a headline is attributable at a glance.
    pub source: String,
    /// The published date as the feed wrote it, left as a string because feeds
    /// disagree on format and the card only needs to show it, not compute on it.
    pub published: Option<String>,
    /// The feed's own short description, as plain text, when it has one.
    pub summary: Option<String>,
}

/// A feed to read and the label shown as its source.
pub struct Feed {
    pub label: &'static str,
    pub url: &'static str,
}

/// The default feeds. A small, broad set: world news and tech, all HTTPS and
/// all open. Kept here as one list so changing what NEXUS reads is one edit,
/// and later a user-configured list can replace it.
pub const DEFAULT_FEEDS: &[Feed] = &[
    Feed {
        label: "BBC World",
        url: "https://feeds.bbci.co.uk/news/world/rss.xml",
    },
    Feed {
        label: "Hacker News",
        url: "https://hnrss.org/frontpage",
    },
    Feed {
        label: "The Verge",
        url: "https://www.theverge.com/rss/index.xml",
    },
    Feed {
        label: "Ars Technica",
        url: "https://feeds.arstechnica.com/arstechnica/index",
    },
];

/// Fetch one feed and parse its headlines. A feed that cannot be reached or
/// read yields an error the caller can skip past, so one dead feed never blanks
/// the rest.
pub fn fetch_one(label: &str, url: &str) -> Result<Vec<NewsItem>, HttpError> {
    let resp = send(Request::get(url))?;
    if !resp.ok() {
        return Err(HttpError::Malformed {
            detail: format!("{label} returned status {}", resp.status),
        });
    }
    Ok(parse_feed(&resp.body, label))
}

/// Fetch every default feed, merge, and return the newest `limit` headlines.
///
/// Feeds are read in order; a failing one is logged and skipped rather than
/// aborting the batch. The merged list is de-duplicated by link, because the
/// same story syndicated across two feeds is one headline, not two.
pub fn fetch_all(limit: usize) -> Vec<NewsItem> {
    let mut all: Vec<NewsItem> = Vec::new();
    for feed in DEFAULT_FEEDS {
        match fetch_one(feed.label, feed.url) {
            Ok(mut items) => all.append(&mut items),
            Err(e) => log::warn!("news: {} unreadable: {e}", feed.label),
        }
    }

    // De-duplicate by link, keeping first seen (feeds are appended in order).
    let mut seen = std::collections::HashSet::new();
    all.retain(|item| seen.insert(item.link.clone()));

    // Interleave so one chatty feed does not crowd out the rest: take from each
    // source round-robin. A feed's items arrive newest-first already, so this
    // keeps that order within a source while mixing sources.
    let interleaved = round_robin_by_source(all);
    interleaved.into_iter().take(limit).collect()
}

/// Reorder a merged list so sources alternate, preserving each source's own
/// order. Without this, four feeds appended in turn would show all of the
/// first feed before any of the second.
fn round_robin_by_source(items: Vec<NewsItem>) -> Vec<NewsItem> {
    let mut buckets: Vec<Vec<NewsItem>> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for item in items {
        let at = *index.entry(item.source.clone()).or_insert_with(|| {
            buckets.push(Vec::new());
            buckets.len() - 1
        });
        buckets[at].push(item);
    }
    let mut out = Vec::new();
    let mut row = 0;
    loop {
        let mut added = false;
        for bucket in &buckets {
            if let Some(item) = bucket.get(row) {
                out.push(item.clone());
                added = true;
            }
        }
        if !added {
            break;
        }
        row += 1;
    }
    out
}

// -- Parsing ------------------------------------------------------------------

/// Pull headlines out of one feed's XML. Handles RSS `<item>` and Atom
/// `<entry>`; a feed is one or the other, so whichever yields entries wins.
fn parse_feed(xml: &str, source: &str) -> Vec<NewsItem> {
    let rss = collect_blocks(xml, "<item", "</item>");
    let blocks = if rss.is_empty() {
        collect_blocks(xml, "<entry", "</entry>")
    } else {
        rss
    };
    blocks
        .into_iter()
        .filter_map(|block| parse_entry(block, source))
        .collect()
}

/// Every substring between an opening tag (matched by prefix, so `<item` also
/// catches `<item>` and `<item foo="bar">`) and its close.
pub(crate) fn collect_blocks<'a>(xml: &'a str, open: &str, close: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(open) {
        let after = &rest[start..];
        let Some(end) = after.find(close) else { break };
        out.push(&after[..end]);
        rest = &after[end + close.len()..];
    }
    out
}

fn parse_entry(block: &str, source: &str) -> Option<NewsItem> {
    let title = tag_text(block, "title")?;
    let link = link_of(block)?;
    if title.is_empty() || link.is_empty() {
        return None;
    }
    let published = ["pubDate", "published", "updated", "dc:date"]
        .iter()
        .find_map(|tag| tag_text(block, tag))
        .filter(|s| !s.is_empty());
    let summary = ["description", "summary", "content"]
        .iter()
        .find_map(|tag| tag_text(block, tag))
        .map(|raw| plain_text(&raw, 240))
        .filter(|s| !s.is_empty() && *s != title);
    Some(NewsItem {
        title,
        link,
        source: source.to_string(),
        published,
        summary,
    })
}

/// A description as plain text: markup removed (feeds often wrap it in
/// HTML, sometimes escaped), whitespace collapsed, cut at a word near `max`.
pub(crate) fn plain_text(raw: &str, max: usize) -> String {
    let decoded = clean(raw);
    let mut out = String::with_capacity(decoded.len());
    let mut in_tag = false;
    for ch in decoded.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let text = out.replace("&nbsp;", " ").replace("&#8217;", "'").replace("&#8216;", "'").replace("&#8220;", "\"").replace("&#8221;", "\"");
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    let at = cut.rfind(' ').unwrap_or(cut.len());
    format!("{}…", cut[..at].trim_end_matches([',', '.', ';', ':']))
}

/// The text of the first `<tag ...>...</tag>`, CDATA unwrapped and the few
/// entities feeds actually use decoded. `None` if the tag is absent.
pub(crate) fn tag_text(block: &str, tag: &str) -> Option<String> {
    let open_prefix = format!("<{tag}");
    let start = block.find(&open_prefix)?;
    // Skip to the end of the opening tag so attributes are not read as text.
    let gt = block[start..].find('>')? + start + 1;
    let close = format!("</{tag}>");
    let end = block[gt..].find(&close)? + gt;
    Some(clean(&block[gt..end]))
}

/// A link for the entry. RSS puts the URL as `<link>text</link>`; Atom puts it
/// in `<link href="...">`. Tries the text form first, then the href.
fn link_of(block: &str) -> Option<String> {
    if let Some(text) = tag_text(block, "link") {
        if text.starts_with("http") {
            return Some(text);
        }
    }
    // Atom: find href="..." inside a <link ...> tag.
    let start = block.find("<link")?;
    let tag_end = block[start..].find('>')? + start;
    let tag = &block[start..tag_end];
    let href = tag.find("href=")? + 5;
    let quote = tag.as_bytes().get(href).copied()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let value_start = href + 1;
    let value_end = tag[value_start..].find(quote as char)? + value_start;
    let url = clean(&tag[value_start..value_end]);
    url.starts_with("http").then_some(url)
}

/// Strip CDATA wrappers and decode the handful of entities feeds use. Keeps the
/// result trimmed; a headline with leading whitespace reads as a bug.
fn clean(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some(inner) = s.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")) {
        s = inner.trim();
    }
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS_SAMPLE: &str = r#"<rss><channel><title>Feed</title>
      <item><title><![CDATA[First & best]]></title>
        <link>https://example.com/a</link>
        <pubDate>Tue, 20 Oct 2026 06:00:00 +0000</pubDate></item>
      <item><title>Second story</title>
        <link>https://example.com/b</link></item>
    </channel></rss>"#;

    const ATOM_SAMPLE: &str = r#"<feed><title>Atom</title>
      <entry><title>Atom one</title>
        <link href="https://example.com/x" rel="alternate"/>
        <updated>2026-10-20T06:00:00Z</updated></entry>
      <entry><title>Atom two</title>
        <link href="https://example.com/y"/></entry>
    </feed>"#;

    #[test]
    fn descriptions_become_plain_short_text() {
        let raw = "<![CDATA[<p>Sensex <b>rose</b> 500 points&nbsp;today.</p><img src='x'/>]]>";
        assert_eq!(plain_text(raw, 240), "Sensex rose 500 points today.");
        let long = "word ".repeat(100);
        let cut = plain_text(&long, 30);
        assert!(cut.ends_with('…') && cut.chars().count() <= 31, "{cut}");
    }

    #[test]
    fn rss_items_are_parsed_with_cdata_and_entities_resolved() {
        let items = parse_feed(RSS_SAMPLE, "Feed");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "First & best");
        assert_eq!(items[0].link, "https://example.com/a");
        assert!(items[0].published.as_deref().unwrap().contains("2026"));
        // An item with no date is still a valid headline.
        assert_eq!(items[1].title, "Second story");
        assert_eq!(items[1].published, None);
    }

    #[test]
    fn atom_entries_read_the_href_link() {
        let items = parse_feed(ATOM_SAMPLE, "Atom");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Atom one");
        assert_eq!(items[0].link, "https://example.com/x");
        assert_eq!(items[1].link, "https://example.com/y");
    }

    #[test]
    fn the_channel_title_is_not_mistaken_for_an_item() {
        // The feed's own <title> sits outside any <item>; only item titles
        // should come back. Two items in, two items out.
        assert_eq!(parse_feed(RSS_SAMPLE, "Feed").len(), 2);
    }

    #[test]
    fn a_title_attribute_is_not_read_as_the_title_text() {
        // The opening tag may carry attributes; the text is what sits between
        // > and </tag>, not anything inside the tag itself.
        let block = r#"<item><title type="html">Real title</title><link>https://e.com/1</link></item>"#;
        assert_eq!(tag_text(block, "title").as_deref(), Some("Real title"));
    }

    #[test]
    fn round_robin_alternates_sources_rather_than_grouping_them() {
        let items = vec![
            NewsItem { title: "a1".into(), link: "1".into(), source: "A".into(), published: None, summary: None },
            NewsItem { title: "a2".into(), link: "2".into(), source: "A".into(), published: None, summary: None },
            NewsItem { title: "b1".into(), link: "3".into(), source: "B".into(), published: None, summary: None },
        ];
        let out = round_robin_by_source(items);
        assert_eq!(
            out.iter().map(|i| i.source.as_str()).collect::<Vec<_>>(),
            vec!["A", "B", "A"],
        );
    }

    #[test]
    fn an_entry_missing_a_link_or_title_is_skipped_not_panicked() {
        let block = "<item><title>No link here</title></item>";
        assert!(parse_entry(block, "X").is_none());
    }
}
