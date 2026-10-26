//! The daily standup draft, and prep for the next meeting.
//!
//! # Assembled, not generated
//!
//! A standup is read aloud to the team. A line about a ticket the user never
//! touched would be embarrassing in a way no amount of fluent phrasing makes
//! up for, so every line here is built from what Jira and GitHub returned:
//! tickets touched since the last working day, the user's own pull requests,
//! and today's calendar. It is a draft: "updated" in Jira means touched by
//! anyone, so the user reads it, edits it, and sends it.
//!
//! # Tickets and pull requests are joined by key
//!
//! Pull requests here are titled "[CONN-709] - ...". When a merged PR names a
//! ticket that already has a line, the PR is folded into that line rather than
//! repeated, so the standup says one thing once.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JiraItem {
    pub key: String,
    pub summary: String,
    pub status: String,
    /// new | indeterminate | done, when known.
    pub category: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Checks {
    pub failing: usize,
    pub pending: usize,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrItem {
    pub repo: String,
    pub number: i64,
    pub title: String,
    /// OPEN | MERGED | CLOSED.
    pub state: String,
    pub is_draft: bool,
    pub merged_at: Option<String>,
    /// APPROVED | CHANGES_REQUESTED | REVIEW_REQUIRED | "".
    pub review_decision: Option<String>,
    pub checks: Option<Checks>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Meeting {
    pub title: String,
    /// Local "HH:MM".
    pub at: String,
    /// Still ahead today.
    pub upcoming: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StandupInput {
    /// Start of the "yesterday" window, YYYY-MM-DD (Friday, on a Monday).
    pub since: String,
    /// Tickets touched since `since` (any status).
    pub recent: Vec<JiraItem>,
    /// Tickets still open and assigned.
    pub open: Vec<JiraItem>,
    pub prs: Vec<PrItem>,
    pub meetings: Vec<Meeting>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingPrep {
    pub title: String,
    pub at: String,
    /// What NEXUS knows about tickets named in the meeting title.
    pub related: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Standup {
    pub yesterday: Vec<String>,
    pub today: Vec<String>,
    pub blockers: Vec<String>,
    pub meetings: Vec<String>,
    pub next_meeting: Option<MeetingPrep>,
    /// The whole thing as plain text, ready to paste into chat.
    pub text: String,
}

/// "avetta/find-client-ui" to "find-client-ui".
fn short(repo: &str) -> &str {
    repo.rsplit('/').next().unwrap_or(repo)
}

fn pr_ref(pr: &PrItem) -> String {
    format!("{}#{}", short(&pr.repo), pr.number)
}

/// The PR title without its leading "[KEY] - " tag, which the line already
/// carries in its own words.
fn bare_title(title: &str) -> &str {
    let t = title.trim();
    match t.strip_prefix('[').and_then(|r| r.split_once(']')) {
        Some((_, rest)) => rest.trim_start_matches([' ', '-', ':']).trim(),
        None => t,
    }
}

/// Jira keys named in a piece of text: "CONN-709", in brackets or not.
pub fn keys_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        let Some((project, number)) = raw.split_once('-') else { continue };
        let ok = project.len() >= 2
            && project.len() <= 10
            && project.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && project.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            && !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit());
        if ok && !out.iter().any(|k| k == raw) {
            out.push(raw.to_string());
        }
    }
    out
}

/// Statuses that mean "not started yet", across the workflows seen here.
fn not_started(status: &str) -> bool {
    let s = status.to_lowercase();
    matches!(s.as_str(), "open" | "to do" | "todo" | "backlog" | "new")
        || s.contains("ready for development")
        || s.contains("selected for development")
}

fn blocked(status: &str) -> bool {
    status.to_lowercase().contains("block")
}

fn decision(pr: &PrItem) -> &str {
    pr.review_decision.as_deref().unwrap_or("")
}

pub fn compose(input: &StandupInput) -> Standup {
    let mut yesterday: Vec<String> = Vec::new();
    let mut today: Vec<String> = Vec::new();
    let mut blockers: Vec<String> = Vec::new();

    // Merged PRs in the window, by the ticket keys they name.
    let merged: Vec<&PrItem> = input
        .prs
        .iter()
        .filter(|p| {
            p.state.eq_ignore_ascii_case("merged")
                && p.merged_at.as_deref().is_some_and(|m| m.get(..10).unwrap_or(m) >= input.since.as_str())
        })
        .collect();
    // (repo, number): PR numbers repeat across repositories.
    let mut folded: Vec<(String, i64)> = Vec::new();

    // Yesterday: tickets touched since the window opened, except ones still
    // not started. A "Ready for Development" ticket updated yesterday was
    // groomed or assigned, not worked on, and saying "worked on" about it in
    // a standup would be false. (Seen in real data: CONN-717.)
    for issue in input
        .recent
        .iter()
        .filter(|i| i.category.as_deref() != Some("new") && !not_started(&i.status))
    {
        let prs_for: Vec<&&PrItem> = merged
            .iter()
            .filter(|p| keys_in(&p.title).contains(&issue.key))
            .collect();
        let tail = if prs_for.is_empty() {
            String::new()
        } else {
            folded.extend(prs_for.iter().map(|p| (p.repo.clone(), p.number)));
            format!(
                " ({} merged)",
                prs_for.iter().map(|p| pr_ref(p)).collect::<Vec<_>>().join(", ")
            )
        };
        let line = if issue.category.as_deref() == Some("done") {
            format!("Finished {}: {}{tail}", issue.key, issue.summary)
        } else {
            format!("Worked on {}: {} (now {}){tail}", issue.key, issue.summary, issue.status)
        };
        yesterday.push(line);
    }
    // Merged PRs that no ticket line already covers.
    for pr in &merged {
        if !folded.contains(&(pr.repo.clone(), pr.number)) {
            yesterday.push(format!("Merged {}: {}", pr_ref(pr), bare_title(&pr.title)));
        }
    }

    // Today: tickets in flight; the next ticket if nothing is in flight.
    let in_flight: Vec<&JiraItem> = input
        .open
        .iter()
        .filter(|i| !not_started(&i.status) && !blocked(&i.status))
        .collect();
    for issue in &in_flight {
        today.push(format!("Continue {}: {} ({})", issue.key, issue.summary, issue.status));
    }
    if in_flight.is_empty() {
        if let Some(next) = input.open.iter().find(|i| not_started(&i.status)) {
            today.push(format!("Pick up {}: {}", next.key, next.summary));
        }
    }

    // Today and blockers from open PRs.
    for pr in input.prs.iter().filter(|p| p.state.eq_ignore_ascii_case("open")) {
        let failing = pr.checks.as_ref().map(|c| c.failing).unwrap_or(0);
        if failing > 0 {
            blockers.push(format!(
                "{} has {failing} failing check{}",
                pr_ref(pr),
                if failing == 1 { "" } else { "s" }
            ));
        }
        if decision(pr) == "CHANGES_REQUESTED" {
            blockers.push(format!("{}: changes requested", pr_ref(pr)));
            today.push(format!("Address review on {}: {}", pr_ref(pr), bare_title(&pr.title)));
        } else if pr.is_draft {
            today.push(format!("Finish draft {}: {}", pr_ref(pr), bare_title(&pr.title)));
        } else if decision(pr) == "APPROVED" {
            if failing == 0 {
                today.push(format!("Merge {} (approved): {}", pr_ref(pr), bare_title(&pr.title)));
            }
        } else {
            today.push(format!("Get review on {}: {}", pr_ref(pr), bare_title(&pr.title)));
        }
    }

    for issue in input.open.iter().filter(|i| blocked(&i.status)) {
        blockers.push(format!("{} is {}: {}", issue.key, issue.status, issue.summary));
    }

    let meetings: Vec<String> = input
        .meetings
        .iter()
        .map(|m| format!("{} {}", m.at, m.title))
        .collect();

    let next_meeting = input.meetings.iter().find(|m| m.upcoming).map(|m| {
        let mut related: Vec<String> = Vec::new();
        for key in keys_in(&m.title) {
            let issue = input.open.iter().chain(input.recent.iter()).find(|i| i.key == key);
            match issue {
                Some(i) => related.push(format!("{}: {} ({})", i.key, i.summary, i.status)),
                None => related.push(format!("{key}: not in your assigned or recent tickets")),
            }
            for pr in input.prs.iter().filter(|p| keys_in(&p.title).contains(&key)) {
                let state = match (pr.state.to_lowercase().as_str(), decision(pr)) {
                    ("merged", _) => "merged".to_string(),
                    (_, "APPROVED") => "approved".to_string(),
                    (_, "CHANGES_REQUESTED") => "changes requested".to_string(),
                    _ if pr.is_draft => "draft".to_string(),
                    _ => "open".to_string(),
                };
                related.push(format!("  {} {state}", pr_ref(pr)));
            }
        }
        MeetingPrep {
            title: m.title.clone(),
            at: m.at.clone(),
            related,
        }
    });

    let section = |name: &str, lines: &[String], empty: &str| -> String {
        let body = if lines.is_empty() {
            format!("- {empty}")
        } else {
            lines.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n")
        };
        format!("{name}\n{body}")
    };
    let text = [
        section("Yesterday", &yesterday, "Nothing recorded in Jira or GitHub"),
        section("Today", &today, "Nothing in flight; planning the next ticket"),
        section("Blockers", &blockers, "None"),
    ]
    .join("\n\n");

    Standup {
        yesterday,
        today,
        blockers,
        meetings,
        next_meeting,
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jira(key: &str, summary: &str, status: &str, category: Option<&str>) -> JiraItem {
        JiraItem {
            key: key.into(),
            summary: summary.into(),
            status: status.into(),
            category: category.map(String::from),
        }
    }

    fn pr(number: i64, title: &str, state: &str, decision: &str, failing: usize) -> PrItem {
        PrItem {
            repo: "avetta/find-client-ui".into(),
            number,
            title: title.into(),
            state: state.into(),
            is_draft: false,
            merged_at: (state == "MERGED").then(|| "2026-10-22T09:00:00Z".to_string()),
            review_decision: Some(decision.into()),
            checks: Some(Checks { failing, pending: 0 }),
        }
    }

    /// Shaped like the real data seen while building this (find-client-ui,
    /// week of 2026-10-19).
    fn week() -> StandupInput {
        StandupInput {
            since: "2026-10-22".into(),
            recent: vec![jira("CONN-709", "Client search query layer", "Done", Some("done"))],
            open: vec![
                jira("CONN-706", "Name search field", "In Development", Some("indeterminate")),
                jira("CONN-720", "Results sorting", "Ready for Development", Some("new")),
                jira("CONN-731", "Map tiles", "Blocked", Some("indeterminate")),
            ],
            prs: vec![
                pr(5, "[CONN-709] - Client search query layer", "MERGED", "APPROVED", 0),
                pr(8, "[CONN-706] - Name search field with recent searches", "OPEN", "APPROVED", 0),
                pr(10, "[CONN-718] - Zero-results empty state", "OPEN", "", 0),
                pr(9, "[CONN-723] - Radius control", "OPEN", "CHANGES_REQUESTED", 2),
            ],
            meetings: vec![
                Meeting { title: "Standup".into(), at: "10:00".into(), upcoming: false },
                Meeting { title: "CONN-706 design sync".into(), at: "15:00".into(), upcoming: true },
            ],
        }
    }

    #[test]
    fn a_merged_pr_folds_into_its_ticket_rather_than_repeating() {
        let s = compose(&week());
        assert_eq!(
            s.yesterday,
            vec!["Finished CONN-709: Client search query layer (find-client-ui#5 merged)"]
        );
    }

    #[test]
    fn today_lists_work_in_flight_and_pr_next_steps() {
        let s = compose(&week());
        assert!(s.today.contains(&"Continue CONN-706: Name search field (In Development)".to_string()));
        assert!(s.today.iter().any(|l| l.starts_with("Merge find-client-ui#8 (approved)")));
        assert!(s.today.iter().any(|l| l.starts_with("Get review on find-client-ui#10")));
        assert!(s.today.iter().any(|l| l.starts_with("Address review on find-client-ui#9")));
        // Not started and not blocked tickets are not "continued".
        assert!(!s.today.iter().any(|l| l.contains("CONN-720") || l.contains("CONN-731")));
    }

    #[test]
    fn blockers_come_from_failing_checks_requested_changes_and_blocked_tickets() {
        let s = compose(&week());
        assert!(s.blockers.contains(&"find-client-ui#9 has 2 failing checks".to_string()));
        assert!(s.blockers.contains(&"find-client-ui#9: changes requested".to_string()));
        assert!(s.blockers.contains(&"CONN-731 is Blocked: Map tiles".to_string()));
    }

    #[test]
    fn nothing_in_flight_suggests_the_next_ticket() {
        let mut input = week();
        input.open.retain(|i| i.key == "CONN-720");
        input.prs.clear();
        let s = compose(&input);
        assert_eq!(s.today, vec!["Pick up CONN-720: Results sorting"]);
    }

    #[test]
    fn the_next_meeting_gets_its_tickets_and_prs() {
        let prep = compose(&week()).next_meeting.expect("a meeting is ahead");
        assert_eq!(prep.title, "CONN-706 design sync");
        assert_eq!(prep.related[0], "CONN-706: Name search field (In Development)");
        assert_eq!(prep.related[1], "  find-client-ui#8 approved");
    }

    #[test]
    fn an_empty_day_reads_honestly() {
        let s = compose(&StandupInput { since: "2026-10-22".into(), ..Default::default() });
        assert_eq!(
            s.text,
            "Yesterday\n- Nothing recorded in Jira or GitHub\n\nToday\n- Nothing in flight; \
             planning the next ticket\n\nBlockers\n- None"
        );
    }

    #[test]
    fn a_merge_before_the_window_is_not_yesterday() {
        let mut input = week();
        input.recent.clear();
        input.prs[0].merged_at = Some("2026-10-15T09:00:00Z".into());
        assert!(compose(&input).yesterday.is_empty());
    }

    #[test]
    fn the_same_pr_number_in_another_repo_is_not_folded_away() {
        let mut input = week();
        let mut other = pr(5, "Bump deps", "MERGED", "APPROVED", 0);
        other.repo = "avetta/ui".into();
        input.prs.push(other);
        let s = compose(&input);
        assert!(s.yesterday.contains(&"Merged ui#5: Bump deps".to_string()), "{:?}", s.yesterday);
    }

    #[test]
    fn a_ticket_only_groomed_yesterday_is_not_claimed_as_worked_on() {
        let mut input = week();
        input.recent.push(jira("CONN-717", "Search this area", "Ready for Development", Some("new")));
        let s = compose(&input);
        assert!(!s.yesterday.iter().any(|l| l.contains("CONN-717")), "{:?}", s.yesterday);
    }

    #[test]
    fn keys_are_found_in_brackets_and_prose_but_not_in_noise() {
        assert_eq!(keys_in("[CONN-709] - Client search"), vec!["CONN-709"]);
        assert_eq!(keys_in("CONN-706 design sync and AB-12"), vec!["CONN-706", "AB-12"]);
        assert!(keys_in("covid-19 and x-1 and CONN-").is_empty());
    }
}
