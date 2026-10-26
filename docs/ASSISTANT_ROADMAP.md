# NEXUS Personal Assistant: Roadmap & Tracker

The single checklist for turning NEXUS into a personal assistant. Nothing ships
that isn't listed here; nothing listed here is forgotten. Check items off as they
land.

## Principles (non-negotiable)

- **Local-first brain.** Ollama (`llama3.2` now; swap to a bigger model with one
  `ollama pull`, no code change).
- **Honest outputs.** Model-written text is marked "composed by NEXUS." No
  fabricated facts, no advice dressed as certainty.
- **Gated actions.** Anything that writes, sends, or applies is confirmed. The
  only unattended action is what the user explicitly set (e.g. a restock alert).
- **Nothing inferred about the user.** NEXUS only knows what the user typed into
  **Me**. No guessing goals from calendar or messages.
- **India-aware.** Prices in INR, NSE/BSE stocks, India gold and tax dates.

## Honest limits (built in, not bugs)

- **Money:** NEXUS tracks and informs. It does **not** give investment advice,
  predict markets, or promise returns.
- **Jobs:** NEXUS finds, tailors, preps, and pre-fills. It does **not**
  auto-submit applications (ToS bans, reputation, errors). You review & submit.
- **Clone:** supervised only, writes in a sandbox, you approve a diff. Not
  autonomous. Needs a stronger model than `llama3.2` for reliable coding.
- **No bank/brokerage auto-sync** (security, needs a paid aggregator). Manual or
  read-only entry.

---

## Foundation

- [x] **The brain.** Ollama installed, `llama3.2` pulled, auto-used (local
  provider tried first).
- [x] **Sectioned navigation.** Rail "Life" group; each section its own screen.
- [x] **Me / personal profile.** `profile` table (migration 008),
  `profile.rs`, `nexus_get_profile` / `nexus_set_profile_field`, `MeScreen`.
  Backbone every pillar reads via `profile::as_context_lines`.

---

## Phase 1: Informative news (done, expand later)

- [x] `news_connector.rs`: RSS/Atom fetch + parse (BBC, HN, The Verge, Ars).
- [x] `nexus_fetch_news`, `nexus_news_briefing` (model-written lead).
- [x] **Briefing** hero on Overview (replaced NEXUS Says + Live Activity).
- [ ] **News section**: full feed view (currently a placeholder).
- [ ] Choose/weight feeds and interests.
- [ ] Per-topic model summaries.

## Phase 2: Money (NEXT)

- [x] **Market connector**: live prices in INR (stocks by NSE symbol, gold ETF)
      via Yahoo Finance. `market_connector.rs`. Confirmed reachable through the
      app's own HTTP path (not blocked by the proxy, unlike Amazon was).
- [x] **Portfolio**: add holdings (symbol + qty), live value, day change, total.
      `MoneyScreen`, migration 009 `holdings`, `nexus_portfolio` /
      `nexus_add_holding` / `nexus_remove_holding`.
- [x] **Mutual funds (AMFI NAV)**: search any Indian fund by name, add by units,
      priced daily via AMFI (`mf_connector.rs`, mfapi.in mirror). Covers Zerodha
      / ICICI / Tata holdings by manual entry. `nexus_search_funds`, migration
      010 `kind` column.
- [x] **Manual holdings**: ULIP / cash / anything with no feed, entered as a
      rupee value (kind 'manual').
- [ ] **Price watch + alerts**: threshold alert (reuses stock-watch + ntfy).
- [x] **Savings tracker**: Spending tab "Saved in <month>" card: money in
      (recorded income, else the user's usual income, marked as an estimate)
      plus refunds less spending; investments and card bills are moved, not
      subtracted. Six-month saved/overspent chart, click a month to open it.
- [x] **Goals**: name, target, date, return; progress bar + required monthly
      saving, computed against today. `GoalsPanel`, migration 011 `goals`,
      `nexus_list_goals` / `nexus_add_goal` / `nexus_update_goal_amount` /
      `nexus_remove_goal`. Math in `finance.rs` (11 tests).
- [x] **Retirement projection**: age, corpus, SIP, return, inflation, today's
      expense, projects corpus at retirement vs a 25x (4% rule) target, shows
      shortfall + extra monthly needed. `RetirementPanel`,
      `nexus_retirement_projection`, inputs persisted in the profile.
- [x] **Money extras (India)**: bill / EMI / insurance / subscription renewals
      with computed next-due dates, plus built-in India tax dates (advance tax
      Q1-Q4, ITR). `bills.rs` (7 tests), migration 012 `bills`, `BillsPanel`
      (Money > Bills & Tax tab), `nexus_list_bills` / `nexus_add_bill` /
      `nexus_remove_bill` / `nexus_tax_dates`.
- [ ] **Money briefing line**: model summary over the user's own numbers.
- [x] **Spending tab** (Money, first tab). Bank SMS forwarded by the phone
  ("Incoming SMS to URL forwarder", sender `(?i)(ICICI|HDFC)`, OTPs excluded)
  to a Google Apps Script in the user's own account (`docs/bank-sms/Code.gs`),
  stored in a Google Sheet; NEXUS pulls new rows every 5 min
  (`bank_sms.rs`; key in the Keychain, only Google's content host followed on
  redirect). `spend.rs` reads each SMS: shapes seen in real messages are
  booked automatically (ICICI UPI debit, ICICI debit with balance), anything
  else waits in "to check" with a pre-filled suggestion, non-payments (dues,
  requests, statements) are set aside. Migration 018: `expenses` (paise),
  `payee_rules`, `spend_budgets`, `sms_inbox`.
  - Month view: calendar heatmap of daily spending with the selected day's
    payments, today / month / daily average / biggest day, vs the same days
    last month, by category with budgets, top places, by account, by weekday,
    searchable list with inline recategorise (remembered per payee).
  - Only `spend` counts: income, refunds and transfers (card bill, investments)
    are listed but never added, so card purchases are not counted twice.
  - Manual add / edit / delete for cash and corrections.
  - Bank balance per account: last "Avl Bal" stated in an SMS (or set by
    hand), carried forward through payments booked since (migration 020).
  - Savings use income from the salary SMS (a credit of at least half the
    usual income); ICICI credits are booked as income automatically.
  - Touch ID lock (`spend_lock.rs`, LocalAuthentication, password fallback):
    enforced in the backend on every data command; re-locks after 5 min
    without Spending use or 5 min of Mac idle, and on restart.
- [ ] Spending: HDFC card, ICICI credit and card-bill formats (add each from a
  real message as it arrives in "to check").
- [ ] Spending: monthly statement import to reconcile missed messages.

## Phase 3: Learn (done, v1)

- [x] Skill plan: model-drafted ordered topics, personalised from **Me**,
      redraftable (cards survive a redraft), topics tick off.
- [x] Tutor loop: per-topic "Explain" (about 150 words + example) and "Make
      cards" (5 Q/A flashcards). Missed cards come back tomorrow.
- [x] Spaced repetition: trimmed SM-2 (Again / Good / Easy), full-screen review.
- [x] Streak + per-skill progress + due counts.
- Implementation: `learn.rs` (scheduling, streak, forgiving parsers, 10 tests
  incl. real llama3.2 output), migration 013, `LearnScreen`, 11 commands.
- **Model path note:** study material uses `ollama_provider::generate_text`
  (a writer prompt, plain text), not the action planner. Under the planner's
  prompt, "write a plan / create flashcards" came back as invented action ids.
- **Transport note:** `http::send` caps calls at ~20s (curl `max-time`) even
  though the provider declares 90s. Fine for current generations (0.4 to 2s);
  revisit before long generations (e.g. resume tailoring).
- [ ] Later: quiz-from-mistakes view, mastery score per topic.

## Phase 4: Career & job-switch agent (in progress)

- [x] Job discovery from sources that permit it: Remotive (API) + We Work
      Remotely (RSS). LinkedIn / Naukri / Indeed forbid automated access and
      are not touched; those roles are tracked by hand. `career.rs`.
- [x] Match scoring vs profile: deterministic keyword overlap with its working
      shown (matched skills + the listing's skills you lack). India eligibility
      hint from the location text. 11 tests.
- [x] Application tracker: Saved / Applied / Interviewing / Offer / Closed,
      next step + date, notes, follow-up-due badge. Migration 014,
      `CareerScreen` (Discover + Pipeline).
- [x] Resume tailoring: **reorder-only**. Within each job the bullets most
      relevant to the role move up; matching skills go first. No line is added,
      dropped or reworded (enforced by test). Why: llama3.2 asked to rewrite a
      resume gave the user's employer the ad's title ("Senior") and silently
      dropped a whole job. Plus a short model-written summary, fact-checked.
      `tailor.rs`, Career > Resume tab, migration 015 (`documents`,
      application description / tailored resume / cover letter).
- [x] Cover letter: model-drafted, fact-checked by `tailor::unsupported`:
      numbers not in the resume, job-ad names not in the resume ("Senior",
      "GraphQL"), and required skills claimed but absent (caught the live
      model claiming "accessibility, including WCAG" with no such experience).
      Flags are checks, not deletions. Known limit: an odd spelling of the role
      title can raise an extra "Senior" flag; puffery ("seasoned") is not
      caught. 11 tests + a live harness.
- [x] Interview prep (per application, "Interview prep" tab):
      - **Role brief, never generated**: the job's requirements split into
        "your resume shows" / "not on your resume", each gap with a one-click
        "Learn" that creates a Learn skill with a plan. Built from text only.
      - **No company brief.** Tested on a real hiring company, llama3.2
        invented a founding year, headcount, a $100M Series C and a $1.5B
        valuation despite being told to say when unsure. Replaced with search
        links (company, recent news, the listing).
      - Fixed list of questions to ask the interviewer.
      - **Mock interview**: 6 questions from the job + resume (3 technical,
        2 behavioural, 1 on a resume item); answer, then coach feedback
        (Worked / Missing / Structure / Try opening with). The suggested
        opening line is fact-checked against resume + answer.
      - Migration 016 `interview_questions`; 5 commands; requirement
        extraction reworked to keep the job's spelling ("Next.js") and
        three-letter skills (CSS, AWS, SQL, Vue).
- [x] Assisted apply ("Apply" tab): a checklist that gets everything ready
      and never touches the form. Open listing; resume and cover letter as
      copy-text or a file saved to Downloads and shown in Finder (never
      overwrites: numbered names); one-click copy of name / email / phone /
      LinkedIn / portfolio; "I've submitted it" moves the role to Applied with
      a 7-day follow-up. Contact details live in Me under "For applications"
      and are excluded from every model prompt (`profile::CONTACT_PREFIX`,
      tested). `career::safe_filename` + `unused_path`, tested.

**Phase 4 (Career) complete.**

## Infrastructure fixes found while building Career

- [x] **Pipe deadlock in `shell::wait_bounded`**: output over ~64 KB hung every
      subprocess until its deadline (curl, `gh`, osascript). Was the real cause of
      the "timed out" mutual-fund NAV and an empty Remotive feed. Pipes now
      drained concurrently; stdin written on its own thread. 2 regression tests.
- [x] **Model timeout never applied**: Ollama declared 90s but `http::send`
      hard-coded 20s. Added `send_with_timeout`; guard test.
- [x] **Pending offer lost when a model is installed**: a misheard reply to
      NEXUS's own question went to the model, which invented an action. While an
      offer is open, replies no longer escalate to a model.
- [x] Two converse tests assumed no local model; now hermetic.
- [x] **Ollama context window** set explicitly (8K) for writing: the default
      silently drops the start of an overflowing prompt, i.e. the resume.
- [x] **Test race on a process-wide list** (`notification_connector`,
      pre-existing): six tests shared `ANNOUNCED` and failed at random in a full
      run. `forget_announced()` now holds a test lock and is `#[must_use]`.

## Phase 5: Today briefing + care (ties it together)

- [x] Briefing hero (news lead).
- [x] **Today readout** leads the hero: one line assembled (not generated)
      from meetings + next meeting, reminders due, bills within a week, a tax
      date within a month, job follow-ups, cards to review; clickable chips go
      to Money / Career / Learn. Local day via SQLite `localtime` (verified
      against the system clock). An unreadable calendar says nothing about
      meetings rather than "no meetings". `today.rs` (tested), `nexus_today`.
      The model's news line moved down beside the news.
- [x] **Health & wellbeing**: break + posture nudge after N minutes of real
      keyboard/mouse activity (macOS `HIDIdleTime` via `ioreg`, no permission
      prompt), reset by stepping away; ignored nudges repeat every 15 min;
      water every N active minutes; wind-down once per evening; nothing during
      a calendar meeting (held, then delivered); sleep is not counted as work.
      Background thread like the notification watcher; macOS notifications.
      Settings in Me > Wellbeing (settings table, not the profile, so never in
      a prompt); live "Active 42m · break in 8m" chip on Today.
      `wellbeing.rs` (10 tests + live idle check). Not yet observed end to end
      (a real nudge needs 50 real minutes).
- [x] **Work prep**: Standup card on Overview. Yesterday / Today / Blockers,
      assembled (not generated) from Jira, GitHub and today's calendar; an
      editable, copyable draft. Two new gated Read actions:
      `jira.recent_work` (tickets touched since the last working day, **any**
      status: `my_issues` hides Done, i.e. the finished work a standup needs;
      JQL validated against live Jira) and `github.my_prs` (your PRs across
      tracked repos, each repo asked as its owner's mapped account; live-tested).
      Merged PRs fold into their ticket's line by key; not-started tickets that
      were merely groomed are not claimed as "worked on" (seen in real data).
      Sources that fail are named, never silently thin. Next-meeting prep
      links tickets and PRs named in the meeting title. `standup.rs` (10 tests).

**Phase 5 complete.**

## Phase 6: Supervised task clone (done)

- [x] Engine chosen by measurement, same task + 14 hidden tests:
      llama3.2 6-7/14, **Claude Code 14/14** (both runs). qwen2.5-coder:14b not
      measured (download throttled to ~11h on this network).
- [x] **Sandbox, each limit found by running Claude against a throwaway repo:**
      detached git worktree outside the repo; `--tools Read Edit Write Glob Grep`
      (with Bash merely disallowed, Claude reached for `Monitor` to run tests);
      `--strict-mcp-config` + empty server list (without it the session had the
      avt `auth_token` / `query_db` tools). Verified: exactly 5 tools exist.
      No shell, so no commits; NEXUS never commits either. Guard test fails if
      any flag is removed.
- [x] NEXUS runs the project's tests itself (npm/pnpm/yarn/cargo/unittest,
      resolved without PATH, node_modules shared by symlink and excluded from
      the diff, CI=1); the result shown is what NEXUS observed, null when no
      tests, never assumed. Diff read before tests so artefacts can't enter it.
- [x] Per-project consent: `projects.clone_allowed`, **off for all 9 projects**
      (checked in the live DB). Apply = `git apply --check` then apply to the
      working copy as uncommitted changes; conflicts touch nothing. Discard
      removes the worktree. Runs interrupted by a restart are marked so.
- [x] `clone.rs` (7 tests + a live end-to-end pipeline test: edit 27s, 7/7
      tests, applied uncommitted, worktree removed), `shell::run_in_dir`,
      migration 017, 6 commands, Clone screen (Work rail).
- [ ] Not yet: a fix-failing-tests loop (re-run Claude with the test output).
      Deliberately left manual for v1: you see the failure and decide.

**All six phases complete.**

---

## Add-ons folded into the phases

- [x] Personal memory/profile (foundation).
- [ ] Health & wellbeing (Phase 5).
- [ ] Money extras: bills, EMIs, insurance, India tax dates (Phase 2).
- [ ] Work prep: standup + meeting prep (Phase 5).

## Later (not scheduled)

- [ ] Second brain / read-later with model summaries.
- [ ] Relationship nudges (birthdays, stay-in-touch).
- [ ] Time tracking / focus timer.

---

## Build order

1. ~~Foundation (brain, nav, Me)~~ (done)
2. ~~News~~ (done)
3. ~~Money~~ (done; briefing line folds into Today)
4. ~~Learn~~ (done, v1)
5. ~~Career~~ (done)
6. ~~Today + care + work prep~~ (done)
7. ~~Supervised clone~~ (done)

_Last updated: 2026-10-21._
