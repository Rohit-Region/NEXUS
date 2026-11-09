# NEXUS Mobile: Spec & Tracker

NEXUS on an Android phone, built from the same codebase as the Mac app. This
is the single plan for it: what the phone does, how it stays in step with the
Mac, and the order it is built in. Check items off as they land.

## Goal

The phone is a full NEXUS, not a remote control for the Mac. It fetches its
own prices, NAVs and news, and keeps working with the Mac switched off and
away from home. When the phone and the Mac are both on the home Wi-Fi, they
sync.

## Principles (non-negotiable)

- **The phone stands alone.** No feature on the phone waits for the Mac to be
  on. A Mac-only feature says "Mac only" rather than spinning.
- **Sync stays at home.** Phone and Mac talk only on the home Wi-Fi. Nothing
  is exposed to the internet: no cloud relay, no open port, no tunnel.
- **Secrets never sync.** API keys and tokens are never written to the sync
  log or to a log file. The one way they move is the USB copy below, sealed
  so that only the phone's Keystore can open them (changed 2026-11-07).
- **One codebase.** Desktop and mobile build from `main`. There is no
  long-lived mobile branch and no second repo; work lands in short branches,
  one per slice.
- **The Mac gets no worse.** A change made for the phone keeps the Mac app's
  behaviour, security posture and dependencies as they are, unless this
  document says otherwise.

## Honest limits (built in, not bugs)

- **Away from home, the two devices drift.** Edits made on either side wait
  and catch up at the next home sync. Neither device shows the other's
  changes in between.
- **Android only.** No iPhone build: that needs a paid Apple developer
  account and a different signing path.
- **Sideloaded, not on the Play Store.** The APK is installed by hand. The
  release signing key must be backed up: without it, an update cannot be
  installed over the existing app, and the app has to be removed, data
  included, to install a new one.
- **The phone's assistant needs a cloud key.** The Mac runs the model through
  the Claude CLI and Ollama, and neither runs on a phone. A Claude or OpenAI
  key is billed per use; a Groq key is free (rate-limited) and is the backup,
  tried last and whenever the one before it fails.
- **Until Phase 2, the phone gets the Mac's data by USB** (`mac-to-phone.sh`).

---

## How it fits together

```
             home Wi-Fi only
 ┌───────────────┐  sync  ┌───────────────┐
 │  NEXUS (Mac)  │◄──────►│ NEXUS (phone) │
 │ Rust + SQLite │        │ Rust + SQLite │
 └───────┬───────┘        └───────┬───────┘
         │ fetches                │ fetches
         ▼                        ▼
   Yahoo, AMFI, PNB MetLife, Tata AIA, Morningstar, news feeds
```

Each device has the whole engine: the same Rust backend, the same React
screens and its own SQLite database. Tauri v2 builds the Android app from the
same project; its Android files live in `src-tauri/gen/android/`.

### What runs where

| Area                                                | Mac                | Phone                                |
| --------------------------------------------------- | ------------------ | ------------------------------------ |
| Portfolio, mutual funds, stocks, gold               | Yes                | Yes                                  |
| ULIP policies, fund switch reports, retirement plan | Yes                | Yes                                  |
| Spending, bills, balances                           | Yes                | Yes, behind the fingerprint lock     |
| Stock picks, IPO desk, news radar, news             | Yes                | Yes                                  |
| Weather, Today, Me profile                          | Yes                | Yes                                  |
| Bank SMS                                            | Google Sheet relay | Read from the phone's own inbox     |
| Alerts                                              | ntfy               | Native push (Phase 3)                |
| Assistant chat                                      | Claude CLI, Ollama | Claude, OpenAI or Groq key           |
| Learn, resume tailoring, cover letters, news radar  | Ollama (local)     | The same keys, when allowed          |
| Career search, Me, Settings, Projects list          | Yes                | Yes                                  |
| Calendar                                            | EventKit           | Mac only for now                     |
| IDE, agents, Clone tasks, shell                     | Yes                | Mac only (the screens show)          |
| Registry of IDEs and agents                         | Yes                | Not on the phone                     |
| WhatsApp, Teams, Outlook desktop, notifications     | Yes                | Mac only                             |
| GitHub and Jira (via `gh` and `acli`)               | Yes                | Mac only                             |
| Voice (speech in and out)                           | Yes                | Mac only                             |

---

## Decisions

- **HTTP on Android is `ureq`; the Mac keeps `curl`.** NEXUS-017 chose the
  system `curl` over a TLS library to keep about 56 crates out of the build.
  Android has no `curl`, so the Android build alone compiles in `ureq` with
  `rustls`: 11 crates it did not already have, and none in the Mac app
  (measured). Both builds sit behind the existing `http::send`, so no
  connector changes. Approved 2026-11-05.
- **Android trusts the Mozilla root certificates built into the app**
  (`webpki-roots`), not the phone's certificate store. Using the phone's store
  (`rustls-platform-verifier`) needs a JNI start-up call and a Kotlin
  component, and the built-in roots need neither. The cost: the roots update
  when the app is updated, and a certificate the user installed on the phone
  is not trusted.
- **Mac-only code is fenced off with `cfg`, not moved into a separate crate.**
  The `nexus-core` crate split is deferred; it is done only if the
  platform-gated code becomes hard to follow.
- **Sync is home Wi-Fi only** (see Phase 2), chosen over Tailscale or a cloud
  relay so nothing is reachable from outside the home network.
- **The phone's lock is its own biometric** (fingerprint or face, with the
  phone's PIN as the fallback), through Tauri's `tauri-plugin-biometric`.
  Spending stays locked on the phone exactly as Touch ID locks it on the Mac.
  Approved 2026-11-05.
- **The release signing key is kept in two places, never in the repo.** The
  keystore file goes in the user's private Google Drive; its passwords go in
  a password manager (Google Password Manager is enough), not beside the file.
  Either half alone is useless to whoever finds it. The repo is public, so
  `*.jks`, `*.keystore` and `key.properties` are git-ignored.

- **Secrets on the phone live in its own store, encrypted by the Android
  Keystore** (`SecretStore.kt`, reached from Rust over JNI by
  `android_secrets.rs`). The Keychain's three functions in `http.rs` have
  Android versions, so every secret-holding feature works unchanged: the
  Claude key, Jira, GitHub. No new dependency. Chosen over
  `tauri-plugin-stronghold` 2026-11-07.
- **The phone writes with Claude where the Mac uses Ollama** (`writer.rs`):
  Learn, resume tailoring, cover letters, interview practice and the news
  radar. Only with both Reasoning switches on, since each prompt carries the
  user's profile, resume or holdings, and each call is audited. The Mac keeps
  its local model and its "nothing leaves this Mac" promise. Chosen
  2026-11-07.
- **Every screen except the Registry is on the phone**, behind five tabs:
  Home, Money, Stocks, Ask, and More for the rest. Settings that only
  configure the Mac (voice, Mac access, wellbeing nudges, ntfy calls and
  approvals) are hidden there. Chosen 2026-11-07.

## Open questions

- [x] **Where secrets live on the phone.** The Android Keystore (see
      Decisions).

---

## Phase 1: Money on the phone (NEXT)

The Money and Stocks screens working on Android, fetching everything
themselves. Four PRs, in order.

### PR 1: HTTP client for Android (`feat/mobile-http-client`)

- [x] `ureq` (with `rustls` and built-in roots) as an Android-only dependency,
      and a dev-dependency so the Mac's tests can run it.
- [x] `http::send` routes to `ureq` on Android, to `curl` on the Mac, with the
      same `Response` and `HttpError`, the same timeouts, the same body limit,
      `https://` only, and redirects reported (as absolute addresses), never
      followed.
- [x] Tests run both transports against one local server and require the same
      status, redirect, body type and credentials; the Mac app gains no crate.
- [x] A body over the size limit is cut on a character boundary; it used to
      panic when the limit fell inside a multi-byte character.

### PR 2: Fence off Mac-only code

- [x] The Mac-only dependencies (`objc2-*` and `block2`: EventKit, speech,
      audio, LocalAuthentication) move under
      `[target.'cfg(target_os = "macos")']`.
- [x] Every program NEXUS runs goes through `shell.rs`, and off the Mac it
      starts none and says "runs only on the Mac". That one guard covers
      `osascript`, `plutil`, `open`, `gh`, `claude`, the Keychain's
      `security` and the restock check's `curl`; `caffeinate` is refused
      the same way. A test fails if a spawn is added without the guard.
- [x] The calendar reports "Mac only"; voice has an off-Mac stand-in
      (`voice/off_mac.rs`) that shares the intent, response and wake-word
      logic and answers "Mac only" for the microphone and speech.
- [x] Spending stays locked off the Mac until the fingerprint unlock lands
      (PR 4); the window has a label for it ready.
- [x] `cargo check --lib --target aarch64-linux-android` succeeds with no
      warnings.

### PR 3: Android shell

- [x] `tauri android init`, committed under `src-tauri/gen/android`. The
      phone's app id is `com.nexus.mobile`, set in
      `tauri.android.conf.json`; the Mac keeps `com.nexus.desktop`, since its
      data folder and macOS permissions are tied to it. Named NEXUS, with the
      NEXUS launcher icon.
- [x] Internet permission only.
- [x] Release signing reads `src-tauri/gen/android/keystore.properties` when
      it exists; without it the release APK is unsigned. The keystore and
      that file are git-ignored.
- [x] A debug APK installs and opens on the emulator, and creates its own
      database.

Building needs JDK 17 (the Android Gradle Plugin's version). Android
Studio's bundled JDK 25 is too new for Gradle 8.14.

### PR 4: NEXUS on the phone

- [x] A phone shell (`PhoneShell`) around the same `Dashboard` as the Mac:
      Home, Money, Stocks and Ask as tabs at the bottom, the rest under More.
      Chosen by the WebView's user agent (`lib/platform.ts`), so the Mac's
      shell is unchanged.
- [x] The page stays clear of the status bar, the gesture bar, a camera
      cutout and the keyboard (`MainActivity.kt`); the bars are NEXUS black
      with light icons.
- [x] Phone layout rules live at the end of each screen's own stylesheet,
      scoped under `.phone-shell`, so none can reach the Mac: holdings on two
      lines, section tabs that scroll sideways, forms that wrap, 40px and
      larger touch targets, and the stock table's names pinned while it
      scrolls sideways.
- [x] Portfolio, the PNB and Tata fund reports, goals, retirement, bills and
      stocks work on the phone with its own database, fetching over its own
      HTTPS client. A chart no longer swallows the swipe that scrolls past it.
- [x] Spending opens only after the phone's fingerprint, face or PIN, through
      `tauri-plugin-biometric` 2.3 (the newest for this Tauri; Android only,
      one crate). Locked again after 5 minutes away and on every restart.
- [x] The phone's secret store (see Decisions): a stored key survives a
      restart.
- [x] Spending reads ICICI and HDFC texts straight from the phone's inbox
      (`BankInbox.kt`, `android_sms.rs`), once the user allows it, in place
      of the Sheet and its key. Same sender rule and OTP exclusion as the
      script, and the script's id for each text, so one that also came in
      through the Sheet is booked once. The first read takes the whole
      inbox.
- [x] Groq as a third, free provider, tried after Claude and OpenAI. A
      provider that fails (no credit, rate limit, outage) hands over to the
      next, on the Mac and the phone; each attempt is audited. The default
      model is `openai/gpt-oss-120b`, with short reasoning so the reply fits
      the token limit.
- [x] The assistant on the phone, with a Claude or OpenAI key entered in
      Settings > Reasoning (the Mac can now enter one there too, instead of
      in Terminal). A reply it cannot give is shown, not only spoken.
- [x] Learn, Career, News, Me, Projects, Clone and Settings on the phone;
      the parts that need the Mac say so or are hidden.
- [x] Home is the same command centre as the Mac's, in one column with the
      core first, shown even before the phone has a project. The schedule,
      pull requests, standup, service versions and stock watch stay on the
      Mac. A reminder coming due opens the assistant with it written out,
      while the app is open (alerts with the app closed are Phase 3). Kai
      Connect stays on the Mac.
- [x] `scripts/mac-to-phone.sh` copies the Mac's data over USB until sync
      exists: a consistent snapshot of the whole database (holdings,
      spending and its categories, plans, settings, permissions, profile),
      which the phone puts in place of its own at start (`db/from_mac.rs`).
      The Mac is the master copy until Phase 2: what was entered only on the
      phone is replaced, so changes are made on the Mac and the script run
      again. The phone then re-reads its inbox; texts the Mac already has
      from the Sheet carry the same id and are booked once.
- [x] The same script sends the keys (Jira, GitHub, Claude, OpenAI),
      sealed: the phone publishes the public half of an RSA key held
      in its Android Keystore, the Mac pipes each secret from the Keychain
      straight into encryption with it, and the phone opens them into its
      secure storage (`MacTransfer.kt`). No secret is in the clear on either
      disk, on the cable, or in a command line. Microsoft sign-ins are left
      out: they are sessions, and only Mac features use them. Jira and
      GitHub tokens go to the phone, rather than only their results, so
      Assigned to Me is live away from home (decided 2026-11-07).
- [x] Jira's API token can be entered in Settings (it was a Terminal command
      before). On the phone, a refused token is reported as such: the
      fallback through a Jira tab needs the Mac's Chrome.
- [x] Prices refresh when the app opens, and again when it comes back after
      5 minutes or more away (background refresh is Phase 3).
- [ ] Installed and checked on the real phone.

## Phase 2: Home Wi-Fi sync

One PR, branched after PR 4 (chosen 2026-11-10). The USB copy
(`mac-to-phone.sh`) keeps one job: setting up a phone the first time. After
that, only changes travel.

Decisions (2026-11-10):

- **Encryption uses each system's own crypto, no new crates:** Android's
  `javax.crypto` over JNI (as `SecretStore.kt` already does) and the Mac's
  CommonCrypto through a few hand-written bindings. Both sides must produce
  byte-identical results, so a shared set of test vectors checks them.
- **Pairing works both ways:** the pairing key travels sealed in the USB copy,
  or the Mac shows a QR code the phone scans. Scanning needs the camera
  permission and a scanner; which one (a plugin or Android's own) is an open
  dependency question to settle before it is built.
- **The audit logs merge both ways,** so the Mac's Reasoning page shows what
  the phone sent to a cloud model, and the reverse.

Work:

- [ ] **No clashing row numbers:** the phone numbers its new rows from
      1,000,000,000 up; the Mac keeps the low range. Links between rows
      (an expense and its shares) keep working.
- [ ] **Change log:** triggers record every insert, update and delete in a
      synced table, with the device and a clock reading.
- [ ] **Conflicts:** for user data (holdings, goals, policies, budgets, payee
      rules, expense categories, profile, Learn, Career), the newest edit
      wins, field by field. Deletes are kept as markers so they travel too.
- [ ] **Logs are merged, not overwritten:** the ULIP NAV log, bank texts, the
      pick journal and the audit logs keep every entry from both sides.
- [ ] **One payment, once:** an expense booked from the same bank text on
      both devices has two row numbers but one `sms_id`; sync matches on it.
- [ ] **Not synced:** caches (they refetch), settings that belong to one
      device (voice, the inbox cursor, model choices, ntfy), the Registry,
      IDEs and agents, and secrets (never).
- [ ] **Same version on both:** sync refuses when the database versions
      differ and says which device to update.
- [ ] **The Mac listens** while NEXUS is open and answers only the paired
      phone; the phone starts every sync, since Android cannot reliably wait
      for connections in the background.
- [ ] **Finding each other:** the phone asks the home network where NEXUS
      is and the Mac replies, so a changed address breaks nothing. "Home" is
      wherever the paired Mac answers: no Wi-Fi name check, which would need
      Android's location permission.
- [ ] **Encrypted and signed** with the pairing key, both directions.
- [ ] **Pairing** through the USB copy, and by QR code.
- [ ] **When:** on opening the app, on coming back to it, every few minutes
      while it is open, and from a "Sync now" button.
- [ ] **Outlook and Teams** reach the phone as results: the Mac, which is
      signed in, sends the calendar and messages, so the phone never signs
      in to Microsoft. Fresh as of the last sync (decided 2026-11-07).
- [ ] A "Last synced" line on both devices, and a plain reason when a sync
      fails ("Mac not found", "update NEXUS on the phone").

## Phase 3: Phone powers

- [x] **Bank SMS** read on the phone directly, replacing the Google Sheet
      relay (PR 4).
- [ ] **Push notifications** replacing ntfy.
- [ ] **Background refresh** of prices and NAVs while the app is closed.
- [ ] **Fingerprint** for anything else that is locked on the Mac.

## Phase 4: Everything else

- [x] **Assistant chat** on the phone with a Claude API key (PR 4).
- [x] News, career and the remaining screens laid out for a phone (PR 4).
- [x] "Mac only" screens that explain what they need and where to find it
      (PR 4).
