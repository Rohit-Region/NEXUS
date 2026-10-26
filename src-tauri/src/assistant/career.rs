//! Career: finding roles and scoring them against the user's profile.
//!
//! # Sources, and why only these
//!
//! Remotive (a public JSON API) and We Work Remotely (public RSS). Both publish
//! their listings precisely so they can be read by programs. LinkedIn, Naukri
//! and Indeed forbid automated access in their terms, so NEXUS does not touch
//! them; applications found there are tracked by hand in the same tracker.
//!
//! # Scoring, and why it is not the model
//!
//! The match score is plain keyword overlap between the user's skills and
//! target role (from **Me**) and the job's title, tags and description. That is
//! deliberate: it is instant, it is the same every time, and it can say *why*,
//! which skills matched and which of the job's listed skills the user lacks. A
//! model's score would be slower, would drift between runs, and could not show
//! its working. The score ranks the list; it is not a verdict on fit.
//!
//! # Location
//!
//! Remote listings are often restricted to a region. The eligibility tag reads
//! the listing's location text for signs it is open to someone in India
//! ("worldwide", "India", "APAC") or closed to them ("USA only", "Europe"). It is
//! a heuristic, shown as a hint and never used to hide a listing.

use serde::Serialize;

use super::http::{send, Request};
use super::news_connector::{collect_blocks, tag_text};

/// A job listing, normalised across sources.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub title: String,
    pub company: String,
    pub url: String,
    pub location: String,
    pub posted: Option<String>,
    pub tags: Vec<String>,
    pub source: String,
    /// Plain-text description, used for scoring and snapshotted into the
    /// tracker so a resume can later be tailored against it. Clipped at parse
    /// time; a listing's essentials are near the top.
    #[serde(rename = "description")]
    pub text: String,
}

/// A job with its match worked out.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoredJob {
    #[serde(flatten)]
    pub job: Job,
    /// 0..=100. Ranks the list; not a verdict.
    pub score: u8,
    /// The user's skills found in the listing.
    pub matched: Vec<String>,
    /// Skills the listing names (its tags) that the user does not list.
    pub gaps: Vec<String>,
    /// "open" | "restricted" | "unclear", for someone based in India.
    pub eligibility: &'static str,
}

/// What the scorer needs from the profile.
#[derive(Debug, Clone, Default)]
pub struct Seeker {
    pub skills: Vec<String>,
    pub target_role: String,
}

impl Seeker {
    /// Build from the profile's `skills` (comma-separated) and `role_target`.
    pub fn from_profile(skills: Option<&str>, target_role: Option<&str>) -> Seeker {
        Seeker {
            skills: skills
                .unwrap_or("")
                .split([',', ';', '\n'])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            target_role: target_role.unwrap_or("").trim().to_string(),
        }
    }
}

// -- Fetching -----------------------------------------------------------------

/// Search Remotive, plus the We Work Remotely front-end and full-stack feeds,
/// and return every listing found. A source that fails is skipped (and logged)
/// rather than failing the search.
pub fn fetch_jobs(query: &str) -> Vec<Job> {
    let mut jobs = Vec::new();

    match fetch_remotive(query) {
        Ok(mut found) => jobs.append(&mut found),
        Err(e) => log::warn!("career: remotive unreadable: {e}"),
    }
    for feed in [
        "https://weworkremotely.com/categories/remote-front-end-programming-jobs.rss",
        "https://weworkremotely.com/categories/remote-full-stack-programming-jobs.rss",
    ] {
        match fetch_wwr(feed) {
            Ok(mut found) => jobs.append(&mut found),
            Err(e) => log::warn!("career: wwr unreadable: {e}"),
        }
    }

    // The same listing can appear in both WWR feeds.
    let mut seen = std::collections::HashSet::new();
    jobs.retain(|j| seen.insert(j.url.clone()));
    jobs
}

fn get(url: &str) -> Result<String, String> {
    let resp = send(Request::get(url).with_header("User-Agent", "Mozilla/5.0 (NEXUS)"))
        .map_err(|e| e.to_string())?;
    if !resp.ok() {
        return Err(format!("status {}", resp.status));
    }
    Ok(resp.body)
}

fn fetch_remotive(query: &str) -> Result<Vec<Job>, String> {
    let q = query.trim();
    let url = if q.is_empty() {
        "https://remotive.com/api/remote-jobs?category=software-dev".to_string()
    } else {
        format!("https://remotive.com/api/remote-jobs?search={}", encode(q))
    };
    let body = get(&url)?;
    parse_remotive(&body)
}

pub(crate) fn parse_remotive(body: &str) -> Result<Vec<Job>, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("unreadable: {e}"))?;
    let jobs = value
        .get("jobs")
        .and_then(|j| j.as_array())
        .ok_or("no jobs array")?;
    Ok(jobs
        .iter()
        .filter_map(|j| {
            let s = |k: &str| j.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let title = s("title");
            let url = s("url");
            if title.is_empty() || url.is_empty() {
                return None;
            }
            let tags = j
                .get("tags")
                .and_then(|t| t.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str().map(|x| x.trim().to_string()))
                        .filter(|t| !t.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let posted = Some(s("publication_date"))
                .filter(|p| !p.is_empty())
                .map(|p| p.chars().take(10).collect());
            Some(Job {
                title,
                company: s("company_name"),
                url,
                location: s("candidate_required_location"),
                posted,
                tags,
                source: "Remotive".to_string(),
                text: clip(html_to_text(&s("description"))),
            })
        })
        .collect())
}

fn fetch_wwr(feed: &str) -> Result<Vec<Job>, String> {
    Ok(parse_wwr(&get(feed)?))
}

/// WWR items title themselves "Company: Role" and carry a `<region>`.
pub(crate) fn parse_wwr(xml: &str) -> Vec<Job> {
    collect_blocks(xml, "<item", "</item>")
        .into_iter()
        .filter_map(|block| {
            let raw_title = tag_text(block, "title")?;
            let url = tag_text(block, "link").filter(|u| u.starts_with("http"))?;
            let (company, title) = match raw_title.split_once(':') {
                Some((c, t)) => (c.trim().to_string(), t.trim().to_string()),
                None => (String::new(), raw_title.trim().to_string()),
            };
            Some(Job {
                title,
                company,
                url,
                location: tag_text(block, "region").unwrap_or_default(),
                posted: tag_text(block, "pubDate"),
                tags: Vec::new(),
                source: "We Work Remotely".to_string(),
                text: clip(html_to_text(&tag_text(block, "description").unwrap_or_default())),
            })
        })
        .collect()
}

// -- Scoring ------------------------------------------------------------------

/// Score every job against the seeker and sort best-first.
pub fn rank(jobs: Vec<Job>, seeker: &Seeker) -> Vec<ScoredJob> {
    let mut scored: Vec<ScoredJob> = jobs.into_iter().map(|j| score(j, seeker)).collect();
    scored.sort_by(|a, b| b.score.cmp(&a.score));
    scored
}

pub fn score(job: Job, seeker: &Seeker) -> ScoredJob {
    let haystack = normalise(&format!("{} {} {}", job.title, job.tags.join(" "), job.text));
    let tokens: std::collections::HashSet<&str> = haystack.split(' ').collect();
    let title_norm = normalise(&job.title);
    let title_tokens: std::collections::HashSet<&str> = title_norm.split(' ').collect();
    // Title and tags only: where a two-letter skill is trusted (see `mentions`).
    let labels = normalise(&format!("{} {}", job.title, job.tags.join(" ")));
    let label_tokens: std::collections::HashSet<&str> = labels.split(' ').collect();

    let matched: Vec<String> = seeker
        .skills
        .iter()
        .filter(|s| mentions(&haystack, &tokens, &label_tokens, s))
        .cloned()
        .collect();

    // Gaps are the job's own stated skills (its tags) that the seeker does not
    // list. Description words are not treated as requirements: prose mentions
    // far more than a role needs.
    let skills_norm: Vec<String> = seeker.skills.iter().map(|s| normalise(s)).collect();
    let all_gaps: Vec<&String> = job
        .tags
        .iter()
        .filter(|t| {
            let tn = normalise(t);
            !skills_norm.iter().any(|s| *s == tn || s.starts_with(&tn) || tn.starts_with(s.as_str()))
        })
        .collect();

    // Coverage: of the job's tags when it has them, else of the seeker's skills.
    // Computed from the full gap list. Capping the gaps before counting them
    // once let a listing tagged with forty skills score as 80% covered on the
    // strength of the eight shown.
    let coverage = if !job.tags.is_empty() {
        (job.tags.len() - all_gaps.len()) as f64 / job.tags.len() as f64
    } else if !seeker.skills.is_empty() {
        (matched.len() as f64 / seeker.skills.len() as f64).min(1.0)
    } else {
        0.0
    };

    // Title relevance: share of the target role's meaningful words in the title.
    let role_words: Vec<String> = normalise(&seeker.target_role)
        .split(' ')
        .filter(|w| w.len() > 2 && !STOP_WORDS.contains(w))
        .map(String::from)
        .collect();
    let title_hit = if role_words.is_empty() {
        0.0
    } else {
        role_words
            .iter()
            .filter(|w| title_tokens.iter().any(|t| t == w || (w.len() >= 4 && t.starts_with(w.as_str()))))
            .count() as f64
            / role_words.len() as f64
    };

    let score = (100.0 * (0.65 * coverage + 0.35 * title_hit)).round().clamp(0.0, 100.0) as u8;
    // Only the first few gaps are shown; the score above already counted all.
    let gaps: Vec<String> = all_gaps.into_iter().take(8).cloned().collect();
    let eligibility = eligibility(&job.location);
    ScoredJob {
        job,
        score,
        matched,
        gaps,
        eligibility,
    }
}

/// Words in a target role that say nothing about the kind of work.
const STOP_WORDS: [&str; 6] = ["the", "and", "for", "with", "remote", "role"];

/// Whether a skill appears in the listing.
///
/// Multi-word skills match as a phrase. Skills of two letters or fewer ("Go",
/// "C", "R") are also ordinary words, so they count only as a whole token in the
/// title or tags, never in prose, where "go to market" would otherwise read as
/// Go experience. Longer skills match anywhere, and also match a token they
/// prefix ("react" in "reactjs").
fn mentions(
    haystack: &str,
    tokens: &std::collections::HashSet<&str>,
    label_tokens: &std::collections::HashSet<&str>,
    skill: &str,
) -> bool {
    let s = normalise(skill);
    if s.is_empty() {
        return false;
    }
    if s.contains(' ') {
        return format!(" {haystack} ").contains(&format!(" {s} "));
    }
    if s.len() <= 2 {
        return label_tokens.contains(s.as_str());
    }
    if tokens.contains(s.as_str()) {
        return true;
    }
    s.len() >= 4 && tokens.iter().any(|t| t.starts_with(s.as_str()))
}

/// Lowercase, keep letters, digits, `+` and `#` (for C++ and C#), and turn
/// everything else into single spaces.
fn normalise(raw: &str) -> String {
    let mapped: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '+' | '#') { c } else { ' ' })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a listing's location reads as open to someone in India.
pub fn eligibility(location: &str) -> &'static str {
    let l = location.to_lowercase();
    if l.trim().is_empty() {
        return "unclear";
    }
    const OPEN: [&str; 6] = ["worldwide", "anywhere", "global", "india", "asia", "apac"];
    const CLOSED: [&str; 10] = [
        "usa", "us only", "united states", "canada", "europe", "uk", "emea", "latam",
        "americas", "germany",
    ];
    if OPEN.iter().any(|w| l.contains(w)) {
        "open"
    } else if CLOSED.iter().any(|w| l.contains(w)) {
        "restricted"
    } else {
        "unclear"
    }
}

/// Longest description kept. Fifty listings cross into the UI at once, and a
/// role's essentials (what it is, what it needs) sit in its first few thousand
/// characters.
const MAX_DESCRIPTION: usize = 6_000;

fn clip(text: String) -> String {
    if text.len() <= MAX_DESCRIPTION {
        return text;
    }
    let mut end = MAX_DESCRIPTION;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// A file name built from text the user did not choose for a file system
/// (a company called "A/B Labs", a role with a colon). Path separators and the
/// characters Finder or other systems reject become hyphens; length is capped.
pub fn safe_filename(raw: &str) -> String {
    let mapped: String = raw
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let collapsed = mapped.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(|c: char| c == '.' || c == '-' || c == ' ');
    let mut out: String = trimmed.chars().take(120).collect();
    if out.is_empty() {
        out = "document".to_string();
    }
    out
}

/// A path in `dir` for `stem.txt` that does not exist yet: `stem (2).txt`,
/// `stem (3).txt` and so on, so saving never overwrites an earlier file.
pub fn unused_path(dir: &std::path::Path, stem: &str) -> std::path::PathBuf {
    let mut path = dir.join(format!("{stem}.txt"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).txt"));
        n += 1;
    }
    path
}

/// Strip HTML to readable text: tags removed, the common entities decoded,
/// whitespace collapsed.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(title: &str, tags: &[&str], text: &str, location: &str) -> Job {
        Job {
            title: title.into(),
            company: "Acme".into(),
            url: format!("https://x/{title}"),
            location: location.into(),
            posted: None,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            source: "test".into(),
            text: text.into(),
        }
    }

    fn me() -> Seeker {
        Seeker::from_profile(Some("React, TypeScript, CSS, Go"), Some("Senior Frontend Engineer"))
    }

    #[test]
    fn profile_skills_split_on_commas() {
        assert_eq!(me().skills, vec!["React", "TypeScript", "CSS", "Go"]);
    }

    #[test]
    fn a_matching_frontend_role_outranks_an_unrelated_one() {
        let ranked = rank(
            vec![
                job("Data Engineer", &["python", "spark", "airflow"], "pipelines", "Worldwide"),
                job("Senior Frontend Engineer", &["react", "typescript", "css"], "build UI", "Worldwide"),
            ],
            &me(),
        );
        assert_eq!(ranked[0].job.title, "Senior Frontend Engineer");
        assert!(ranked[0].score > ranked[1].score);
        assert!(ranked[0].score >= 90, "{}", ranked[0].score);
    }

    #[test]
    fn gaps_are_the_jobs_tags_the_seeker_lacks() {
        let s = score(job("Frontend Dev", &["react", "graphql", "nextjs"], "", ""), &me());
        assert_eq!(s.gaps, vec!["graphql", "nextjs"]);
        assert!(s.matched.contains(&"React".to_string()));
    }

    #[test]
    fn a_listing_tagged_with_many_skills_is_not_inflated_by_the_display_cap() {
        // Forty tags, of which the seeker has one. Shown gaps are capped at
        // eight, but coverage must be 1/40, not (40 - 8)/40.
        let mut tags = vec!["react".to_string()];
        tags.extend((0..39).map(|i| format!("skill{i}")));
        let refs: Vec<&str> = tags.iter().map(String::as_str).collect();
        let s = score(job("Backend Engineer", &refs, "", ""), &me());
        assert_eq!(s.gaps.len(), 8);
        assert!(s.score < 30, "a 1-in-40 match scored {}", s.score);
    }

    #[test]
    fn a_short_skill_does_not_match_inside_another_word() {
        // "Go" must not match "good" or "google".
        let s = score(job("Engineer", &[], "a good google team", ""), &me());
        assert!(!s.matched.contains(&"Go".to_string()), "{:?}", s.matched);
        // Nor the English word in prose ("go to market").
        let s = score(job("Engineer", &[], "help us go to market", ""), &me());
        assert!(!s.matched.contains(&"Go".to_string()), "{:?}", s.matched);
        // But a Go role, or a Go tag, counts.
        let s = score(job("Go Engineer", &[], "", ""), &me());
        assert!(s.matched.contains(&"Go".to_string()));
        let s = score(job("Engineer", &["go", "kubernetes"], "", ""), &me());
        assert!(s.matched.contains(&"Go".to_string()));
    }

    #[test]
    fn a_long_skill_matches_a_suffixed_token() {
        let s = score(job("Engineer", &[], "experience with reactjs", ""), &me());
        assert!(s.matched.contains(&"React".to_string()));
    }

    #[test]
    fn eligibility_reads_location_for_india() {
        assert_eq!(eligibility("Worldwide"), "open");
        assert_eq!(eligibility("USA, India"), "open");
        assert_eq!(eligibility("USA only"), "restricted");
        assert_eq!(eligibility("Europe, UK timezones"), "restricted");
        assert_eq!(eligibility(""), "unclear");
        assert_eq!(eligibility("Remote"), "unclear");
    }

    #[test]
    fn file_names_cannot_escape_or_break_the_path() {
        assert_eq!(safe_filename("Resume - A/B Labs - Dev: Lead"), "Resume - A-B Labs - Dev- Lead");
        assert_eq!(safe_filename("../../etc/passwd"), "etc-passwd");
        assert_eq!(safe_filename("   "), "document");
        assert!(safe_filename(&"x".repeat(500)).len() <= 120);
    }

    #[test]
    fn saving_twice_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("nexus-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = unused_path(&dir, "Resume - Acme - Dev");
        assert!(first.ends_with("Resume - Acme - Dev.txt"));
        std::fs::write(&first, "v1").unwrap();
        let second = unused_path(&dir, "Resume - Acme - Dev");
        assert!(second.ends_with("Resume - Acme - Dev (2).txt"));
        std::fs::write(&second, "v2").unwrap();
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "v1", "first file was overwritten");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn html_descriptions_become_plain_text() {
        assert_eq!(html_to_text("<p>Build <b>UI</b> &amp; tools</p>"), "Build UI & tools");
    }

    #[test]
    fn remotive_json_is_read() {
        let body = r#"{"jobs":[{"title":"Frontend Engineer","company_name":"Acme","url":"https://remotive.com/1",
            "candidate_required_location":"Worldwide","publication_date":"2026-10-17T10:00:00",
            "tags":["react","css"],"description":"<p>Hi</p>"},{"title":"","url":""}]}"#;
        let jobs = parse_remotive(body).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].posted.as_deref(), Some("2026-10-17"));
        assert_eq!(jobs[0].tags, vec!["react", "css"]);
    }

    /// Live, ignored by default: hits the real sources.
    /// `cargo test --lib career::tests::live_jobs -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_jobs() {
        match fetch_remotive("frontend") {
            Ok(j) => println!("remotive ok: {}", j.len()),
            Err(e) => println!("remotive err: {e}"),
        }
        let jobs = fetch_jobs("frontend");
        println!("fetched {}", jobs.len());
        for s in rank(jobs, &me()).iter().take(8) {
            println!(
                "{:>3}  [{}] {} @ {} ({}) matched={:?} gaps={:?}",
                s.score, s.eligibility, s.job.title, s.job.company, s.job.source, s.matched, s.gaps
            );
        }
    }

    #[test]
    fn wwr_titles_split_into_company_and_role() {
        let xml = r#"<rss><channel><item><title><![CDATA[Acme Inc: Senior React Developer]]></title>
            <region>Anywhere in the World</region><link>https://weworkremotely.com/x</link>
            <description><![CDATA[<p>Build things</p>]]></description></item></channel></rss>"#;
        let jobs = parse_wwr(xml);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].company, "Acme Inc");
        assert_eq!(jobs[0].title, "Senior React Developer");
        assert_eq!(eligibility(&jobs[0].location), "open");
    }
}
