//! Today: the user's day in one line, assembled from their own data.
//!
//! # Why the line is assembled, not generated
//!
//! Every fact here is already known exactly: how many meetings, which bill is
//! due, how many cards wait. A model asked to phrase them would add warmth and,
//! on the evidence of this project, the occasional detail that is not there. A
//! morning line that invents a meeting is worse than no line, so the sentence
//! is built from the facts directly. It is also instant, which a line read
//! every time the app opens should be.
//!
//! # Honesty about what is not known
//!
//! If the calendar cannot be read (no permission, no account), the line says
//! nothing about meetings rather than "no meetings today". An empty day and an
//! unreadable calendar are different claims, and only one of them is true.

use serde::Serialize;

use super::bills::TaxDate;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarSummary {
    /// Timed meetings today. All-day entries (holidays, birthdays) are not meetings.
    pub count: usize,
    pub next_title: Option<String>,
    /// Local "HH:MM".
    pub next_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillDue {
    pub name: String,
    pub days_until: i64,
    pub amount: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Today {
    pub name: Option<String>,
    /// "morning" | "afternoon" | "evening".
    pub part_of_day: &'static str,
    /// `None` when the calendar could not be read: unknown, not empty.
    pub calendar: Option<CalendarSummary>,
    pub reminders_due: i64,
    /// Bills, EMIs and premiums due within a week, soonest first.
    pub bills_due: Vec<BillDue>,
    /// The next statutory tax date, when it is within a month.
    pub tax_soon: Option<TaxDate>,
    pub follow_ups_due: i64,
    pub interviewing: i64,
    pub cards_due: i64,
    pub streak: i64,
    /// The composed one-liner.
    pub line: String,
}

pub fn part_of_day(hour: i64) -> &'static str {
    match hour {
        0..=11 => "morning",
        12..=16 => "afternoon",
        _ => "evening",
    }
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn when(days: i64) -> String {
    match days {
        d if d < 0 => format!("{} overdue", plural(-d, "day", "days")),
        0 => "due today".to_string(),
        1 => "due tomorrow".to_string(),
        d => format!("due in {d} days"),
    }
}

/// The one-line summary. Everything it says comes from `t`; anything `t` does
/// not know (an unreadable calendar) it does not mention.
pub fn compose_line(t: &Today) -> String {
    let greeting = match &t.name {
        Some(n) if !n.trim().is_empty() => format!("Good {}, {}.", t.part_of_day, n.trim()),
        _ => format!("Good {}.", t.part_of_day),
    };

    let mut parts: Vec<String> = Vec::new();

    if let Some(cal) = &t.calendar {
        if cal.count == 0 {
            parts.push("No meetings today".to_string());
        } else {
            let mut s = format!("{} today", plural(cal.count as i64, "meeting", "meetings"));
            if let (Some(title), Some(at)) = (&cal.next_title, &cal.next_at) {
                s.push_str(&format!(", next {title} at {at}"));
            }
            parts.push(s);
        }
    }
    if t.reminders_due > 0 {
        parts.push(format!("{} due", plural(t.reminders_due, "reminder", "reminders")));
    }
    if let Some(first) = t.bills_due.first() {
        let mut s = format!("{} {}", first.name, when(first.days_until));
        let more = t.bills_due.len() - 1;
        if more > 0 {
            s.push_str(&format!(" (+{more} more this week)"));
        }
        parts.push(s);
    }
    if let Some(tax) = &t.tax_soon {
        parts.push(format!("{} in {}", tax.name, plural(tax.days_until, "day", "days")));
    }
    if t.follow_ups_due > 0 {
        parts.push(format!("{} due", plural(t.follow_ups_due, "job follow-up", "job follow-ups")));
    }
    if t.cards_due > 0 {
        parts.push(format!("{} to review", plural(t.cards_due, "card", "cards")));
    }

    if parts.is_empty() {
        format!("{greeting} Nothing pressing today.")
    } else {
        format!("{greeting} {}.", parts.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Today {
        Today {
            name: Some("Rohit".into()),
            part_of_day: "morning",
            calendar: None,
            reminders_due: 0,
            bills_due: Vec::new(),
            tax_soon: None,
            follow_ups_due: 0,
            interviewing: 0,
            cards_due: 0,
            streak: 0,
            line: String::new(),
        }
    }

    #[test]
    fn a_quiet_day_says_so() {
        assert_eq!(compose_line(&empty()), "Good morning, Rohit. Nothing pressing today.");
    }

    #[test]
    fn an_unreadable_calendar_is_not_reported_as_an_empty_one() {
        // calendar: None means unknown. The line must not claim "no meetings".
        let line = compose_line(&empty());
        assert!(!line.to_lowercase().contains("meeting"), "{line}");
        let mut t = empty();
        t.calendar = Some(CalendarSummary { count: 0, next_title: None, next_at: None });
        assert!(compose_line(&t).contains("No meetings today"));
    }

    #[test]
    fn a_full_day_reads_in_order_with_plurals_right() {
        let mut t = empty();
        t.calendar = Some(CalendarSummary {
            count: 2,
            next_title: Some("Standup".into()),
            next_at: Some("10:00".into()),
        });
        t.reminders_due = 1;
        t.bills_due = vec![
            BillDue { name: "Rent".into(), days_until: 1, amount: Some(25000.0) },
            BillDue { name: "Netflix".into(), days_until: 4, amount: None },
        ];
        t.follow_ups_due = 2;
        t.cards_due = 1;
        assert_eq!(
            compose_line(&t),
            "Good morning, Rohit. 2 meetings today, next Standup at 10:00; 1 reminder due; \
             Rent due tomorrow (+1 more this week); 2 job follow-ups due; 1 card to review."
        );
    }

    #[test]
    fn an_overdue_bill_says_how_late() {
        let mut t = empty();
        t.bills_due = vec![BillDue { name: "Card".into(), days_until: -2, amount: None }];
        assert!(compose_line(&t).contains("Card 2 days overdue"));
    }

    #[test]
    fn no_name_still_greets() {
        let mut t = empty();
        t.name = None;
        t.part_of_day = "evening";
        assert_eq!(compose_line(&t), "Good evening. Nothing pressing today.");
    }

    #[test]
    fn the_part_of_day_follows_the_clock() {
        assert_eq!(part_of_day(7), "morning");
        assert_eq!(part_of_day(13), "afternoon");
        assert_eq!(part_of_day(20), "evening");
    }
}
