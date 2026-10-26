//! Wellbeing nudges: breaks, water, and winding down at night.
//!
//! # How NEXUS knows you have been sitting too long
//!
//! macOS records the time since the last keyboard or mouse input
//! (`HIDIdleTime`), readable without any permission prompt. A minute-by-minute
//! tick adds up continuous active time; stepping away for a few minutes counts
//! as a real break and resets it. So the nudge follows what you actually did,
//! not a wall-clock timer that fires while you are already in the kitchen.
//!
//! # Manners
//!
//! - Never during a meeting: nudges are held (time keeps counting) and land
//!   once it ends. Nobody wants "stand up and stretch" over a screen share.
//! - An ignored break nudge comes back every 15 minutes, not every 50: gentle,
//!   but it does not give up.
//! - Wind-down fires once per evening, only if you are actually at the Mac.
//!
//! The state machine is pure and takes time as input, so every rule above is
//! tested without waiting for a clock.

use serde::{Deserialize, Serialize};

/// The user's choices. Stored as JSON in the settings table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub breaks: bool,
    /// Continuous minutes at the keyboard before a break nudge.
    pub break_minutes: i64,
    pub hydration: bool,
    /// Minutes of activity between water reminders.
    pub hydration_minutes: i64,
    pub winddown: bool,
    /// Local "HH:MM".
    pub winddown_at: String,
    /// Minutes without input that count as having stepped away.
    pub away_minutes: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            breaks: true,
            break_minutes: 50,
            hydration: true,
            hydration_minutes: 90,
            winddown: true,
            winddown_at: "23:00".to_string(),
            away_minutes: 5,
        }
    }
}

impl Settings {
    /// Clamp everything to a sane range and normalise the time. A nudge every
    /// 30 seconds, or a "break" that needs 0 minutes away, would make the
    /// feature something to turn off rather than live with.
    pub fn sanitised(mut self) -> Settings {
        self.break_minutes = self.break_minutes.clamp(15, 180);
        self.hydration_minutes = self.hydration_minutes.clamp(30, 240);
        self.away_minutes = self.away_minutes.clamp(2, 30);
        self.winddown_at = parse_hhmm(&self.winddown_at)
            .map(|m| format!("{:02}:{:02}", m / 60, m % 60))
            .unwrap_or_else(|| "23:00".to_string());
        self
    }

    fn winddown_minute(&self) -> i64 {
        parse_hhmm(&self.winddown_at).unwrap_or(23 * 60)
    }

    pub fn any_enabled(&self) -> bool {
        self.breaks || self.hydration || self.winddown
    }
}

/// "HH:MM" as minutes past midnight.
pub fn parse_hhmm(raw: &str) -> Option<i64> {
    let (h, m) = raw.trim().split_once(':')?;
    let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Nudge {
    Break { active_minutes: i64 },
    Water,
    WindDown { at: String },
}

impl Nudge {
    pub fn title(&self) -> &'static str {
        match self {
            Nudge::Break { .. } => "Time for a break",
            Nudge::Water => "Water",
            Nudge::WindDown { .. } => "Wind down",
        }
    }

    pub fn body(&self) -> String {
        match self {
            Nudge::Break { active_minutes } => format!(
                "You've been at it for {active_minutes} minutes. Stand up, roll your shoulders \
                 and neck, and look at something far away for 20 seconds."
            ),
            Nudge::Water => "Have a glass of water.".to_string(),
            Nudge::WindDown { at } => format!(
                "It's past {at}. Start winding down: wrap up, dim the screen, nothing new tonight."
            ),
        }
    }
}

/// An ignored break nudge comes back after this long.
const REMIND_EVERY: i64 = 15 * 60;
/// The longest gap one tick may count. If the Mac slept, the time asleep is not
/// time at the keyboard.
const MAX_STEP: i64 = 180;
/// Wind-down only fires within this many minutes after the chosen time.
const WINDDOWN_WINDOW: i64 = 180;

/// What has been counted so far. In memory only: a restart starting the count
/// again is harmless, and none of this is worth keeping.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tracker {
    pub active_secs: i64,
    since_water: i64,
    /// `active_secs` when the last break nudge fired; 0 if none this stretch.
    break_marker: i64,
    winddown_day: Option<i64>,
    last_tick: Option<i64>,
}

impl Tracker {
    /// Advance one tick. `idle` is seconds since the last input; `local_minute`
    /// is minutes past local midnight; `local_day` identifies the local date.
    pub fn tick(
        &mut self,
        s: &Settings,
        now: i64,
        idle: i64,
        local_minute: i64,
        local_day: i64,
        in_meeting: bool,
    ) -> Vec<Nudge> {
        let step = self.last_tick.map(|t| (now - t).clamp(0, MAX_STEP)).unwrap_or(0);
        self.last_tick = Some(now);

        if idle >= s.away_minutes * 60 {
            // Away long enough to count as a break. Water is not reset: being
            // away is not the same as having drunk anything.
            self.active_secs = 0;
            self.break_marker = 0;
            return Vec::new();
        }
        self.active_secs += step;
        self.since_water += step;

        let mut out = Vec::new();
        if in_meeting {
            // Held, not dropped: the counts keep going and fire afterwards.
            return out;
        }

        if s.breaks {
            let due = if self.break_marker == 0 {
                self.active_secs >= s.break_minutes * 60
            } else {
                self.active_secs - self.break_marker >= REMIND_EVERY
            };
            if due {
                out.push(Nudge::Break { active_minutes: self.active_secs / 60 });
                self.break_marker = self.active_secs;
            }
        }
        if s.hydration && self.since_water >= s.hydration_minutes * 60 {
            out.push(Nudge::Water);
            self.since_water = 0;
        }
        let at = s.winddown_minute();
        if s.winddown
            && local_minute >= at
            && local_minute < at + WINDDOWN_WINDOW
            && self.winddown_day != Some(local_day)
        {
            out.push(Nudge::WindDown { at: s.winddown_at.clone() });
            self.winddown_day = Some(local_day);
        }
        out
    }

    /// Minutes until the next break nudge, for the status chip.
    pub fn next_break_in(&self, s: &Settings) -> Option<i64> {
        if !s.breaks {
            return None;
        }
        let target = if self.break_marker == 0 {
            s.break_minutes * 60
        } else {
            self.break_marker + REMIND_EVERY
        };
        Some(((target - self.active_secs).max(0) + 59) / 60)
    }
}

/// The one tracker for the process, shared by the background tick and the
/// status command. `None` until the first tick.
pub static TRACKER: std::sync::Mutex<Option<Tracker>> = std::sync::Mutex::new(None);

/// Seconds since the last keyboard or mouse input, from IOKit via `ioreg`.
/// `None` if it cannot be read, in which case no nudge is computed at all
/// rather than guessing whether the user is there.
pub fn idle_seconds() -> Option<i64> {
    let out = super::shell::run(
        "/usr/sbin/ioreg",
        &["-c", "IOHIDSystem", "-d", "4"],
        std::time::Duration::from_secs(5),
    )
    .ok()?;
    parse_idle(&out.stdout)
}

fn parse_idle(ioreg: &str) -> Option<i64> {
    let line = ioreg.lines().find(|l| l.contains("\"HIDIdleTime\""))?;
    let ns: i64 = line.rsplit('=').next()?.trim().parse().ok()?;
    Some(ns / 1_000_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 20_000;

    /// Run `minutes` one-minute ticks of activity starting at `start`.
    fn active(t: &mut Tracker, s: &Settings, start: i64, minutes: i64, meeting: bool) -> (i64, Vec<Nudge>) {
        let mut all = Vec::new();
        let mut now = start;
        for _ in 0..minutes {
            now += 60;
            all.extend(t.tick(s, now, 10, 10 * 60, DAY, meeting));
        }
        (now, all)
    }

    fn only_breaks() -> Settings {
        Settings { hydration: false, winddown: false, ..Settings::default() }
    }

    #[test]
    fn a_break_nudge_after_fifty_active_minutes_not_before() {
        let s = only_breaks();
        let mut t = Tracker::default();
        t.tick(&s, 0, 10, 600, DAY, false); // first tick establishes the clock
        let (_, early) = active(&mut t, &s, 0, 49, false);
        assert!(early.is_empty(), "{early:?}");
        let (_, at50) = active(&mut t, &s, 49 * 60, 1, false);
        assert_eq!(at50, vec![Nudge::Break { active_minutes: 50 }]);
    }

    #[test]
    fn stepping_away_counts_as_a_break_and_resets() {
        let s = only_breaks();
        let mut t = Tracker::default();
        t.tick(&s, 0, 10, 600, DAY, false);
        let (now, _) = active(&mut t, &s, 0, 45, false);
        // Six minutes away.
        assert!(t.tick(&s, now + 360, 6 * 60, 600, DAY, false).is_empty());
        assert_eq!(t.active_secs, 0);
        // Back for 45 more: no nudge yet, the stretch restarted.
        let (_, after) = active(&mut t, &s, now + 360, 45, false);
        assert!(after.is_empty(), "{after:?}");
    }

    #[test]
    fn an_ignored_break_comes_back_every_fifteen_minutes() {
        let s = only_breaks();
        let mut t = Tracker::default();
        t.tick(&s, 0, 10, 600, DAY, false);
        let (_, first_hour) = active(&mut t, &s, 0, 65, false);
        let breaks: Vec<_> = first_hour.iter().filter(|n| matches!(n, Nudge::Break { .. })).collect();
        assert_eq!(breaks.len(), 2, "at 50 and 65 minutes: {first_hour:?}");
    }

    #[test]
    fn nudges_wait_out_a_meeting_then_land() {
        let s = only_breaks();
        let mut t = Tracker::default();
        t.tick(&s, 0, 10, 600, DAY, false);
        let (now, during) = active(&mut t, &s, 0, 70, true);
        assert!(during.is_empty(), "nothing during the meeting: {during:?}");
        let (_, after) = active(&mut t, &s, now, 1, false);
        assert_eq!(after.len(), 1);
    }

    #[test]
    fn sleep_is_not_counted_as_work() {
        let s = only_breaks();
        let mut t = Tracker::default();
        t.tick(&s, 0, 10, 600, DAY, false);
        // The Mac slept for three hours, then input resumed at once (idle small).
        t.tick(&s, 3 * 3600, 5, 600, DAY, false);
        assert!(t.active_secs <= MAX_STEP, "{}", t.active_secs);
    }

    #[test]
    fn water_every_ninety_active_minutes() {
        let s = Settings { breaks: false, winddown: false, ..Settings::default() };
        let mut t = Tracker::default();
        t.tick(&s, 0, 10, 600, DAY, false);
        let (_, n) = active(&mut t, &s, 0, 181, false);
        assert_eq!(n.iter().filter(|x| **x == Nudge::Water).count(), 2);
    }

    #[test]
    fn wind_down_fires_once_per_evening_and_only_in_its_window() {
        let s = Settings { breaks: false, hydration: false, ..Settings::default() };
        let mut t = Tracker::default();
        assert!(t.tick(&s, 0, 10, 22 * 60 + 59, DAY, false).is_empty());
        assert_eq!(t.tick(&s, 60, 10, 23 * 60, DAY, false).len(), 1);
        assert!(t.tick(&s, 120, 10, 23 * 60 + 5, DAY, false).is_empty(), "once per evening");
        // The next evening fires again.
        assert_eq!(t.tick(&s, 180, 10, 23 * 60 + 1, DAY + 1, false).len(), 1);
    }

    #[test]
    fn settings_are_kept_sane() {
        let s = Settings {
            break_minutes: 1,
            hydration_minutes: 9999,
            away_minutes: 0,
            winddown_at: "25:99".into(),
            ..Settings::default()
        }
        .sanitised();
        assert_eq!((s.break_minutes, s.hydration_minutes, s.away_minutes), (15, 240, 2));
        assert_eq!(s.winddown_at, "23:00");
        assert_eq!(Settings { winddown_at: "7:5".into(), ..Settings::default() }.sanitised().winddown_at, "07:05");
    }

    #[test]
    fn ioreg_idle_time_is_parsed_from_nanoseconds() {
        let sample = "    | |   \"HIDIdleTime\" = 7566643875\n    | |   \"Other\" = 1";
        assert_eq!(parse_idle(sample), Some(7));
        assert_eq!(parse_idle("nothing here"), None);
    }

    /// Live, ignored by default: reads the real idle time through NEXUS's
    /// own subprocess path. `cargo test --lib wellbeing::tests::live_idle -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_idle() {
        let started = std::time::Instant::now();
        println!("idle = {:?} s (read in {:?})", idle_seconds(), started.elapsed());
    }

    #[test]
    fn next_break_counts_down() {
        let s = only_breaks();
        let mut t = Tracker::default();
        t.tick(&s, 0, 10, 600, DAY, false);
        active(&mut t, &s, 0, 20, false);
        assert_eq!(t.next_break_in(&s), Some(30));
    }
}
