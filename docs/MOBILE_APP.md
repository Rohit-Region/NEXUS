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
- **Secrets never sync.** API keys and tokens are entered on each device and
  stay on it. They are never written to the sync log or to a log file.
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
- **The phone's assistant costs money.** The Mac runs the model through the
  Claude CLI and Ollama, and neither runs on a phone, so the phone's assistant
  uses a Claude API key, which is billed per use.
- **Until Phase 2, the phone starts empty.** Holdings and policies are entered
  on the phone, or copied once for testing, until sync exists.

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
| Bank SMS                                            | Google Sheet relay | Read on the phone directly (Phase 3) |
| Alerts                                              | ntfy               | Native push (Phase 3)                |
| Assistant chat                                      | Claude CLI, Ollama | Claude API key (Phase 4)             |
| Calendar                                            | EventKit           | Mac only for now                     |
| IDE, agents, Clone, shell                           | Yes                | Mac only                             |
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

## Open questions

- [ ] **Where secrets live on the phone.** The Mac keeps tokens in the
      Keychain through `/usr/bin/security`, which Android does not have. The
      likely answer is the Android Keystore; needed before Phase 4, since
      Phase 1 to 3 use no credentials.

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

- [ ] `tauri android init`; the app id, name and icon.
- [ ] Internet permission only.
- [ ] A debug APK installs and opens on the emulator.

### PR 4: Money screens for a phone

- [ ] A phone layout: bottom tabs, touch-sized controls, a single column.
- [ ] Portfolio, policies (the PNB and Tata reports) and stocks work on the
      phone with its own database.
- [ ] Spending opens only after the fingerprint check.
- [ ] Prices and NAVs refresh when the app opens (background refresh is Phase 3).
- [ ] Installed and checked on the real phone.

## Phase 2: Home Wi-Fi sync

- [ ] **Pairing:** the Mac shows a QR code; the phone scans it once. The code
      carries the Mac's address and a shared key, so only the paired phone is
      answered.
- [ ] **Finding each other:** the phone looks for the Mac only on the home
      network.
- [ ] **Change log:** each device records its edits; a sync exchanges what
      the other has not seen.
- [ ] **Conflicts:** for user data (holdings, policies, plans, settings), the
      newest edit wins, field by field.
- [ ] **Logs are merged, not overwritten:** the ULIP NAV log, bank SMS and the
      pick journal keep every entry from both sides.
- [ ] **Not synced:** caches (they refetch) and secrets (never).
- [ ] A "Last synced" line on both devices.

## Phase 3: Phone powers

- [ ] **Bank SMS** read on the phone directly, replacing the Google Sheet
      relay.
- [ ] **Push notifications** replacing ntfy.
- [ ] **Background refresh** of prices and NAVs while the app is closed.
- [ ] **Fingerprint** for anything else that is locked on the Mac.

## Phase 4: Everything else

- [ ] **Assistant chat** on the phone with a Claude API key.
- [ ] News, career and the remaining screens laid out for a phone.
- [ ] "Mac only" screens that explain what they need and where to find it.
