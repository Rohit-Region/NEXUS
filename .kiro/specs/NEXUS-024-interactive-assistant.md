# NEXUS-024: The Interactive Assistant

## Overview

Everything NEXUS does today, it does because it was asked. This phase makes it
speak first.

The target is the exchange that prompted it:

> **NEXUS:** Hey Rohit, you got a message from Priya on WhatsApp. Shall I read
> it, or skip it?
> **Rohit:** Read it.

That sentence needs four things NEXUS does not have: a way to know a message
arrived, a reason to believe now is the moment, a voice that starts the
conversation rather than answering one, and a question it can hold open while
it waits for the reply. The first is a permission problem, the second already
exists and is being deliberately loosened, the third is a small change, and the
fourth shipped in NEXUS-023 as follow-up offers.

This document plans five milestones. It is written to make the decisions well,
not to describe the implementation.

---

## 1. The constraint that shapes this phase

### 1.1 There is no polite way to read a WhatsApp message

NEXUS-022 stated the position and it has not changed: WhatsApp has no
personal-account API, library automation of the web client risks the number
being banned, and the desktop app publishes no scripting dictionary. Nothing in
this phase revisits that.

What changes is the *source*. macOS itself knows a message arrived, because
WhatsApp told it so in order to draw a banner. That knowledge lives in

```
~/Library/Group Containers/group.com.apple.usernoted/db2/db
```

which is one SQLite file holding every notification every application has
posted. Reading it gives sender and preview for WhatsApp, Teams, Slack,
Calendar and everything else, from a single source, without automating any of
them.

**It requires Full Disk Access, and that is a real cost, accepted knowingly.**
The grant is not scoped to this file. NEXUS will be able to read Messages,
Mail, Safari history and every document on the machine. Nothing in this phase
does so, and the audit trail will show it does not, but the *capability* is
granted and cannot be narrowed. The user made this call with the trade-off
stated.

Two consequences to design around:

- **It may break on an OS update.** Apple has tightened this database more than
  once. The notification source must be one connector that can report
  `Unavailable` without taking the rest of the phase down with it.
- **A denied grant must be legible.** "No messages" and "not allowed to look"
  are different answers and must never render the same.

### 1.2 Speaking on arrival is a decision against the existing design

NEXUS-021 argued for restraint, in its own words: *"An assistant that surfaces
everything it notices is one you learn to ignore, and an ignored assistant is
worse than a silent one: it has trained you not to look."* It built cooldown,
fatigue backoff, a cap of three, and silence as a valid answer.

This phase overrides that for messages. Every qualifying message speaks
immediately. The user asked for this after the argument was put to them, and it
is their machine.

**The machinery is kept, not deleted.** `proactive.rs` keeps its cooldown and
fatigue logic, and message announcements run at an aggression level held in
settings, defaulting to `immediate`. If the thing NEXUS-021 predicted happens,
turning it down is a settings change rather than a rewrite. Building the escape
hatch is cheap; needing it and not having it is not.

---

## 2. Milestones

| ID        | Name                   | Depends on       |
| --------- | ---------------------- | ---------------- |
| NEXUS-024 | Notification ingestion | Full Disk Access |
| NEXUS-025 | The spoken question    | 024              |
| NEXUS-026 | Failure recovery aloud | nothing new      |
| NEXUS-027 | Calendar awareness     | nothing new      |
| NEXUS-028 | Follow-up memory       | 025              |

NEXUS-026 and NEXUS-027 depend on no new permission and can be built first if
the Full Disk Access grant stalls.

---

## 3. NEXUS-024: Notification ingestion

A connector that watches what macOS was told, and nothing else.

### 3.1 Requirements

| ID   | Requirement                                                                                                      |
| ---- | ---------------------------------------------------------------------------------------------------------------- |
| F-01 | A `notifications` connector, `Read` only, `LocalOnly`, with no action that writes, dismisses or clears anything. |
| F-02 | It reports `Unavailable` with the reason when Full Disk Access is not granted, and never reports "no messages".  |
| F-03 | `notifications.recent` returns sender, application, preview and arrival time for notifications since a cursor.   |
| F-04 | Only applications the user has opted into are read. The default set is empty.                                    |
| F-05 | A watcher polls on an interval and raises new arrivals as events. Nothing is persisted beyond the cursor.        |
| F-06 | Previews are never written to the database, never logged, and never sent to a reasoning provider.                |

### 3.2 Design notes

**Poll, do not tail.** The file is written by another process and has no change
notification worth trusting. A poll on a few seconds is enough for a message
notifier and cannot corrupt anything. Open read-only, and treat every failure
as "unavailable" rather than fatal.

**The cursor is the only state.** Storing message previews would create the
transcript-on-disk that every voice milestone has spent effort not creating.
NEXUS reads a preview, speaks it if asked to, and forgets it.

**F-04 is the privacy boundary that makes this defensible.** Full Disk Access
grants everything; the opt-in list is what NEXUS chooses to use. It is a
promise enforced in code and visible in the audit trail, not a claim in a
document.

---

## 4. NEXUS-025: The spoken question

Turning an arrival into an exchange.

### 4.1 Requirements

| ID   | Requirement                                                                                              |
| ---- | -------------------------------------------------------------------------------------------------------- |
| F-01 | A new arrival is announced aloud by name: "you have a message from `<person>` on `<app>`."               |
| F-02 | The announcement ends in a question with named answers: read it, skip it, or reply.                      |
| F-03 | The question is a follow-up offer, so it expires, and an unheard answer is re-asked rather than dropped. |
| F-04 | "Read it" speaks the preview. It does not open the application.                                          |
| F-05 | "Reply" hands off to the existing compose path, with its confirmation unchanged.                         |
| F-06 | Announcements respect Do Not Disturb and stay silent while the microphone is capturing a command.        |
| F-07 | Aggression is a setting: `immediate` (default), `batched`, or `silent`.                                  |

### 4.2 Design notes

**This is the follow-up mechanism, pointed the other way.** NEXUS-023 built an
offer that a bare "yes" answers, carrying its own input and expiring on a TTL.
An announcement is the same object, created by an event instead of by a
completed action. The affirmation vocabulary, the re-ask on a mishearing and
the expiry all come for free.

The one genuinely new part is that NEXUS opens the microphone because *it*
asked, not because the user pressed anything. `expectAnswer` already does this
for the assistant's own questions.

**F-06 is not politeness, it is correctness.** Speaking over a command in
flight means the recogniser hears NEXUS, and NEXUS answering itself is the kind
of loop that makes always-listening unusable.

---

## 5. NEXUS-026: Failure recovery aloud

Today a failed action writes a sentence into a panel the user may not be
looking at. Three separate defects this week were invisible for exactly that
reason.

### 5.1 Requirements

| ID   | Requirement                                                                                            |
| ---- | ------------------------------------------------------------------------------------------------------ |
| F-01 | A failure during a spoken exchange is spoken, not only displayed.                                      |
| F-02 | A failure with a known remedy offers it as an action: "Accessibility is not granted. Shall I open it?" |
| F-03 | The remedy is an ordinary action through the gate, confirmed like any other.                           |
| F-04 | An unrecoverable failure says so plainly rather than offering a remedy that will not work.             |
| F-05 | Remedies are declared by the connector that raised the error, never inferred by the core.              |

### 5.2 Design notes

Connectors already write good failure text. The Accessibility sentence in
`whatsapp_connector` and `ide_connector` names the exact settings pane. What is
missing is that the sentence is a dead end. `ActionError` gains an optional
remedy, and the connector that knows the cause supplies it. This mirrors
`follow_up` exactly and needs no new mechanism.

The audit change from NEXUS-023 matters here: dispatch failures now record the
reason rather than the class, so a remedy has something to key off.

---

## 6. NEXUS-027: Calendar awareness

### 6.1 Requirements

| ID   | Requirement                                                                                   |
| ---- | --------------------------------------------------------------------------------------------- |
| F-01 | A spoken warning before a meeting starts, at a configurable lead time.                        |
| F-02 | NEXUS stays silent for the duration of a meeting, and says nothing about why.                 |
| F-03 | A briefing on a schedule: today's meetings, unread Teams, blocked tasks, PRs awaiting review. |
| F-04 | The briefing is spoken on request at any time, not only on its schedule.                      |
| F-05 | With Outlook unreachable, the briefing reports what it could not reach and delivers the rest. |

### 6.2 Design notes

`outlook.today_schedule` and the existing briefing already do the reading. This
milestone is a scheduler and a voice, not new integration.

**F-02 is the highest-value line in this milestone**, given the decision in
§1.2. Immediate announcements plus a meeting is the worst case for an assistant
that speaks first, and the calendar is what makes it avoidable without asking
the user to configure anything.

---

## 7. NEXUS-028: Follow-up memory

The most assistant-like milestone, and the only one needing a new store.

### 7.1 Requirements

| ID   | Requirement                                                                          |
| ---- | ------------------------------------------------------------------------------------ |
| F-01 | A commitment the user states aloud is recorded with its subject and a due time.      |
| F-02 | NEXUS raises it once when due, as a question, subject to the same fatigue rules.     |
| F-03 | A commitment can be completed, deferred or dropped by voice.                         |
| F-04 | Only things the user said explicitly are recorded. Nothing is inferred from context. |
| F-05 | Commitments are visible and deletable in the UI. Nothing about them is hidden.       |

### 7.2 Design notes

**F-04 is the whole milestone.** A system that infers commitments from
conversation will be wrong often, and being reminded of something you never
agreed to is worse than not being reminded at all. The trigger is an explicit
phrase, "remind me to" or "I need to", and everything else is ignored.

Deliberately not scheduling, not a task manager, and not synced anywhere. Tasks
already exist in the workspace; this is the shorter-lived thing you say out
loud and forget by lunchtime.

---

## 8. What this phase does not do

Stated so it is not mistaken for oversight:

- **It does not send WhatsApp messages automatically.** Reply still goes
  through compose and confirmation. The notification database is a read.
- **It does not read WhatsApp conversation history.** Only what was posted as a
  notification while NEXUS was watching.
- **It does not dismiss, clear or act on notifications.** Read only.
- **It does not add OS-level notifications.** NEXUS speaks and shows things in
  its own window, as NEXUS-021 decided.
- **It does not send message contents to a reasoning provider.** A preview is
  spoken locally or not at all.

---

## 9. Risks

| Risk                                            | Response                                                                        |
| ----------------------------------------------- | ------------------------------------------------------------------------------- |
| The notification schema changes in an OS update | One connector, reporting `Unavailable`; the rest of the phase keeps working     |
| Full Disk Access is granted and later regretted | Opt-in application list; audit trail; the grant is revocable in System Settings |
| Immediate announcements become noise            | Aggression setting ships with `batched` and `silent` already built              |
| NEXUS talks over the user, or over itself       | F-06 in §4.1; silence while capturing is a correctness rule, not a courtesy     |
| Announcements interrupt meetings                | NEXUS-027 F-02, which is why the calendar milestone is not optional             |

---

## 10. Order of work

1. **NEXUS-026.** No new permission, and it makes every later milestone
   debuggable. Failures currently vanish, which is how three defects survived
   this week.
2. **NEXUS-024.** After Full Disk Access is granted. Prove the read works
   before anything is built on it.
3. **NEXUS-025.** The payoff. Nothing new architecturally once 024 lands.
4. **NEXUS-027.** Needed *before* immediate announcements are lived with for
   long, per §6.2.
5. **NEXUS-028.** Last. Wants the spoken-question loop settled first.

---

## 11. Build status

All five milestones are implemented. 739 tests pass. What is **verified** and
what is merely **built** differ, and the difference is the grant:

| Milestone | State | Verified by |
| --------- | ----- | ----------- |
| 026 Failure recovery | Built | Unit tests; the remedy rides the follow-up mechanism |
| 024 Notification ingestion | Built, read unverified | `plutil` decoding tested against real plist fixtures; the store read needs Full Disk Access |
| 025 Spoken question | Built and verified | A commitment seeded as due was raised by the running watcher: offer created, event emitted, `raised_at` stamped |
| 027 Calendar awareness | Built, partly verified | Meeting logic unit tested; the live read runs and fails correctly ("Not signed in to Microsoft"), and the back-off holds at 2 attempts per 16 ticks |
| 028 Follow-up memory | Built and verified | Store, phrase parsing and raising all unit tested |

Two things could not be done without the user present, and neither is a
defect:

- **Full Disk Access** and the connector grants are System Settings actions.
  Writing grant rows directly would bypass the gate this application is built
  around.
- **Whether dictation lands in Claude's prompt box** needs eyes on a screen.

### 11.1 The watcher is in Rust, and that was a correction

The first attempt polled from a `useEffect` in `AppShell`. It never ran, for
reasons never established: the component rendered, its children mounted and
their effects fired, but that one did not.

Chasing it further would have been the wrong use of the time, because the
frontend was the wrong home for it anyway. NEXUS speaking first cannot depend
on a component being mounted, a panel being open, or a view being current. The
watcher is now a thread started in `lib.rs` that emits an event; the window is
a surface NEXUS talks *through*, not the thing that decides whether it talks.
It also restores the rule `AppShell` documents about itself, that it imports
no IPC, which the first attempt broke.

The database lock is taken and released inside each tick rather than held
across the sleep, or every command in the application would block for the
whole interval.

## 12. Granting Full Disk Access

System Settings → Privacy & Security → Full Disk Access → add **NEXUS**.

In development the grant must go to the process that actually opens the file,
which during `tauri dev` is the dev binary rather than the bundled app. Verify
with a read of one row before building anything on it: a denied read and an
empty table look identical from the outside, and that confusion will cost a day.
