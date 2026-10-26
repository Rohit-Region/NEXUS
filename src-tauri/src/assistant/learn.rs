//! The Learn section's logic: spaced repetition, streaks, and reading what the
//! local model writes back.
//!
//! # Spaced repetition
//!
//! Cards are scheduled with a trimmed SM-2 (the algorithm behind Anki's
//! ancestors): a card you recall easily comes back after a growing interval, a
//! card you miss comes back tomorrow. Three grades are enough for a person to
//! answer honestly in a second; finer grades mostly add hesitation.
//!
//! # Reading the model
//!
//! The local model is asked for plain lines ("Q: ... / A: ...", or one topic
//! per line), not JSON. A 3B model writes plain lines far more reliably than it
//! writes valid JSON, and these parsers are deliberately forgiving: numbering,
//! bullets, markdown bold and "Question:" spellings are all accepted, and
//! anything unrecognisable is skipped rather than failing the whole batch.
//!
//! # Days
//!
//! Scheduling works in whole days since the Unix epoch, in UTC. For a user in
//! India the day boundary therefore falls at 05:30 local time, which is fine
//! for "due today" and only matters for a review done between midnight and
//! 05:30, which counts toward the previous day.

/// How hard a recall was, as the user grades it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grade {
    /// Missed it. Comes back tomorrow, and the card's ease drops.
    Again,
    /// Got it with effort or normally.
    Good,
    /// Instant. Interval grows faster.
    Easy,
}

impl Grade {
    pub fn parse(raw: &str) -> Option<Grade> {
        match raw.trim().to_lowercase().as_str() {
            "again" => Some(Grade::Again),
            "good" => Some(Grade::Good),
            "easy" => Some(Grade::Easy),
            _ => None,
        }
    }

    /// SM-2 quality on its 0..=5 scale.
    fn quality(self) -> f64 {
        match self {
            Grade::Again => 1.0,
            Grade::Good => 4.0,
            Grade::Easy => 5.0,
        }
    }
}

/// A card's scheduling state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Schedule {
    pub ease: f64,
    /// Days until the next review.
    pub interval: i64,
    /// Consecutive successful recalls.
    pub reps: i64,
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule {
            ease: 2.5,
            interval: 0,
            reps: 0,
        }
    }
}

/// The next schedule after a review.
pub fn review(prev: Schedule, grade: Grade) -> Schedule {
    let q = grade.quality();
    let (reps, interval) = if q < 3.0 {
        (0, 1)
    } else {
        let reps = prev.reps + 1;
        let interval = match reps {
            1 => 1,
            2 => 6,
            _ => ((prev.interval.max(1) as f64) * prev.ease).round() as i64,
        };
        // Easy gets a little extra on top, so it actually feels different from
        // Good rather than only nudging the ease.
        let interval = if grade == Grade::Easy {
            ((interval as f64) * 1.3).round() as i64
        } else {
            interval
        };
        (reps, interval.max(1))
    };
    let ease = (prev.ease + (0.1 - (5.0 - q) * (0.08 + (5.0 - q) * 0.02))).max(1.3);
    Schedule {
        ease,
        interval,
        reps,
    }
}

/// Today as a whole UTC day number.
pub fn day_of(unix: i64) -> i64 {
    unix.div_euclid(86_400)
}

/// Consecutive days of study ending today, or ending yesterday if nothing has
/// been studied yet today (a streak is not broken until the day is over).
/// `days` is any list of day numbers on which a review happened; duplicates and
/// order do not matter.
pub fn streak(days: &[i64], today: i64) -> i64 {
    let mut set: Vec<i64> = days.to_vec();
    set.sort_unstable();
    set.dedup();
    let has = |d: i64| set.binary_search(&d).is_ok();

    let mut cursor = if has(today) { today } else { today - 1 };
    let mut count = 0;
    while has(cursor) {
        count += 1;
        cursor -= 1;
    }
    count
}

/// A question/answer pair the model proposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardDraft {
    pub front: String,
    pub back: String,
}

/// Strip the decoration a model puts on a line: list markers, numbering,
/// markdown bold, surrounding whitespace.
fn clean_line(raw: &str) -> String {
    let mut s = raw.trim().replace("**", "").replace("__", "");
    // Leading bullets and "1." / "1)" numbering.
    loop {
        let t = s.trim_start();
        let stripped = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("• "))
            .map(|r| r.to_string())
            .or_else(|| {
                let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
                if digits > 0 {
                    let rest = &t[digits..];
                    rest.strip_prefix(". ")
                        .or_else(|| rest.strip_prefix(") "))
                        .or_else(|| rest.strip_prefix('.'))
                        .map(|r| r.to_string())
                } else {
                    None
                }
            });
        match stripped {
            Some(next) if next != s => s = next,
            _ => break,
        }
    }
    s.trim().to_string()
}

/// Split "Label: rest" when the label is one of `labels`, case-insensitive.
fn after_label<'a>(line: &'a str, labels: &[&str]) -> Option<&'a str> {
    let lower = line.to_lowercase();
    for label in labels {
        let prefix = format!("{label}:");
        if lower.starts_with(&prefix) {
            return Some(line[prefix.len()..].trim());
        }
    }
    None
}

/// Pull question/answer pairs out of the model's text.
///
/// A pair needs both halves; a question with no answer is dropped. An answer
/// that runs over several lines is joined. At most `max` cards are kept.
pub fn parse_cards(text: &str, max: usize) -> Vec<CardDraft> {
    let mut out = Vec::new();
    let mut front: Option<String> = None;
    let mut back: Option<String> = None;

    let flush = |front: &mut Option<String>, back: &mut Option<String>, out: &mut Vec<CardDraft>| {
        if let (Some(f), Some(b)) = (front.take(), back.take()) {
            if !f.is_empty() && !b.is_empty() {
                out.push(CardDraft { front: f, back: b });
            }
        }
        *front = None;
        *back = None;
    };

    for raw in text.lines() {
        let line = clean_line(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(q) = after_label(&line, &["q", "question"]) {
            flush(&mut front, &mut back, &mut out);
            front = Some(q.to_string());
        } else if let Some(a) = after_label(&line, &["a", "answer"]) {
            if front.is_some() {
                back = Some(a.to_string());
            }
        } else if let Some(b) = back.as_mut() {
            // Continuation of a multi-line answer.
            b.push(' ');
            b.push_str(&line);
        }
        if out.len() >= max {
            break;
        }
    }
    flush(&mut front, &mut back, &mut out);
    out.truncate(max);
    out
}

/// Pull a topic list out of the model's text: one topic per line, decoration
/// stripped, duplicates and preamble ("Here are the topics:") dropped.
pub fn parse_topics(text: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = clean_line(raw);
        if line.is_empty() || line.len() > 120 {
            continue;
        }
        // Preamble and sign-off lines end with a colon or read as sentences
        // about the list rather than items in it.
        let lower = line.to_lowercase();
        if line.ends_with(':') || lower.starts_with("here ") || lower.starts_with("sure") {
            continue;
        }
        let topic = line.trim_end_matches('.').trim().to_string();
        if !out.iter().any(|t| t.eq_ignore_ascii_case(&topic)) {
            out.push(topic);
        }
        if out.len() >= max {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missed_card_comes_back_tomorrow_and_gets_harder() {
        let s = review(Schedule { ease: 2.5, interval: 10, reps: 4 }, Grade::Again);
        assert_eq!(s.interval, 1);
        assert_eq!(s.reps, 0);
        assert!(s.ease < 2.5);
    }

    #[test]
    fn good_recalls_grow_the_interval_1_6_then_by_ease() {
        let s1 = review(Schedule::default(), Grade::Good);
        assert_eq!(s1.interval, 1);
        let s2 = review(s1, Grade::Good);
        assert_eq!(s2.interval, 6);
        let s3 = review(s2, Grade::Good);
        assert!(s3.interval > 6, "{}", s3.interval);
    }

    #[test]
    fn easy_outpaces_good() {
        let base = Schedule { ease: 2.5, interval: 6, reps: 2 };
        assert!(review(base, Grade::Easy).interval > review(base, Grade::Good).interval);
    }

    #[test]
    fn ease_never_drops_below_the_floor() {
        let mut s = Schedule::default();
        for _ in 0..30 {
            s = review(s, Grade::Again);
        }
        assert!(s.ease >= 1.3);
    }

    #[test]
    fn a_streak_counts_back_from_today_or_yesterday() {
        assert_eq!(streak(&[100, 99, 98], 100), 3);
        // Nothing today yet: the streak still stands from yesterday.
        assert_eq!(streak(&[99, 98], 100), 2);
        // A gap breaks it.
        assert_eq!(streak(&[100, 98], 100), 1);
        assert_eq!(streak(&[], 100), 0);
        // Duplicates and order are irrelevant.
        assert_eq!(streak(&[98, 100, 99, 100], 100), 3);
    }

    #[test]
    fn cards_are_read_through_model_decoration() {
        let text = "Here are some cards:\n\
                    1. **Q:** What does `let` do in Rust?\n\
                    **A:** Binds a value to a name.\n\
                    \n\
                    2. Question: What is ownership?\n\
                    Answer: Each value has one owner,\n\
                    and it is dropped when the owner goes out of scope.\n\
                    Q: Orphan question with no answer\n";
        let cards = parse_cards(text, 10);
        assert_eq!(cards.len(), 2, "{cards:?}");
        assert_eq!(cards[0].front, "What does `let` do in Rust?");
        assert_eq!(cards[0].back, "Binds a value to a name.");
        assert!(cards[1].back.contains("goes out of scope"));
    }

    #[test]
    fn the_card_cap_is_respected() {
        let text = (0..20)
            .map(|i| format!("Q: q{i}\nA: a{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(parse_cards(&text, 5).len(), 5);
    }

    #[test]
    fn topics_drop_preamble_numbering_and_duplicates() {
        let text = "Here is a plan for Rust:\n\
                    1. Ownership and borrowing\n\
                    2. Lifetimes.\n\
                    - Traits\n\
                    * traits\n\
                    Sure, let me know if you need more.\n";
        let topics = parse_topics(text, 10);
        assert_eq!(topics, vec!["Ownership and borrowing", "Lifetimes", "Traits"]);
    }

    /// Verbatim llama3.2 output under the writer prompt, captured while building
    /// this. If a prompt or parser change breaks reading what the model really
    /// writes, this fails before a user sees an empty deck.
    #[test]
    fn real_model_output_parses() {
        let plan = "Big-O notation\nData modeling\nDatabase design\nScalability\nCaching\n\
                    Load balancing\nMicroservices architecture\nDistributed systems\nCloud computing";
        assert_eq!(parse_topics(plan, 12).len(), 9);

        let cards = "Q: What is the primary purpose of caching in system design?\n\
                     A: To reduce the time it takes to access frequently requested data or resources.\n\n\
                     Q: What type of caching is used to store frequently accessed data in memory?\n\
                     A: In-memory caching.\n\n\
                     Q: What happens to cached data when it expires or becomes outdated?\n\
                     A: The data is typically replaced with fresh data from the original source.";
        let parsed = parse_cards(cards, 10);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[1].back, "In-memory caching.");
    }

    #[test]
    fn grades_parse_case_insensitively() {
        assert_eq!(Grade::parse("Good"), Some(Grade::Good));
        assert_eq!(Grade::parse(" EASY "), Some(Grade::Easy));
        assert_eq!(Grade::parse("meh"), None);
    }
}
