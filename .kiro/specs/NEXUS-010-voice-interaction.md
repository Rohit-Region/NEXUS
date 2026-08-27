# NEXUS-010: Voice Interaction

> **STATUS: BLOCKED. DO NOT IMPLEMENT.**
>
> This milestone is gated behind an explicit decision that has not been made,
> and behind a technical spike (section 4) whose outcome may remove one of the
> two candidate options entirely.
>
> No code, no dependency, no entitlement, and no schema change may be
> introduced for voice until section 9 is satisfied in full.

## Overview

The microphone button in the `CommandBar` has been a deliberate no-op since NEXUS-001. Its click handler is an empty function, `isListening` is hardcoded `false`, and voice recognition has appeared on the explicit out-of-scope list of every specification from NEXUS-003 onward.

NEXUS-010 is where that would change. It was originally scoped as phase NEXUS-009B; it now has its own document because it is gated by a decision that NEXUS-009 is not, and keeping the gate visible matters more than keeping the phases together.

**This document exists to make the decision well, not to describe an implementation.** It records the options, their honest costs, the constraint that must not be traded away, and the conditions under which work may begin.

### What "JARVIS" does and does not mean here

The shorthand invites an assumption worth killing early. In NEXUS, voice means:

- **Speech to text, then the NEXUS-009 command registry.** A transcript is a query string. It is matched against the same static command list the keyboard palette uses, by the same substring rule.

It does not mean:

- Natural-language understanding, intent classification, or an LLM anywhere in the path
- A conversational agent, a persona, or spoken replies
- Text to speech, which remains out of scope
- Any capability the keyboard palette does not already have

Voice is an **alternative input device for an existing, deterministic feature**. If a command cannot be run from the keyboard palette, it cannot be run by voice either. That constraint is what keeps this milestone small, testable, and honest.

---

## 1. Existing State

### 1.1 Verified at time of writing

Confirmed by inspection across `src/` and `src-tauri/src/`:

- **No speech API is referenced anywhere.** No `SpeechRecognition`, `webkitSpeechRecognition`, `getUserMedia`, `MediaRecorder`, `AudioContext`, or `speechSynthesis`.
- **No microphone entitlement or usage description** in `tauri.conf.json` or `capabilities/default.json`.
- **No audio dependency** in `package.json` or `Cargo.toml`.
- The microphone button carries `aria-label="Voice input (not available)"` and `title="Voice input - coming soon"`.

### 1.2 Dependency on NEXUS-009

**NEXUS-010 cannot begin before NEXUS-009 ships.** It has no command vocabulary of its own: it produces a string and hands it to the registry NEXUS-009 defines. Without that registry there is nothing for a transcript to do.

This ordering is deliberate. Building voice first would force a command grammar to be invented for it, and that grammar would then constrain the keyboard palette rather than the reverse.

### 1.3 The pending placeholder decision

Separately from this milestone, a decision is pending after NEXUS-004 through NEXUS-008 manual validation completes, on the microphone button and the `"Ask Nexus anything..."` placeholder:

1. Leave the microphone placeholder as-is
2. Disable it visibly
3. Replace the placeholder with a Command Palette hint

That decision belongs to the frozen tree, not to this milestone, and NEXUS-009 explicitly does not pre-empt it. It is recorded here only because whichever way it goes affects what the microphone button looks like when NEXUS-010 eventually wires it up.

---

## 2. Scope, if and when unblocked

### 2.1 Provisional functional intent

Recorded to size the work, not as approved requirements.

| ID   | Intent                                                                                                           |
| ---- | ---------------------------------------------------------------------------------------------------------------- |
| V-01 | The microphone button starts and stops listening.                                                                |
| V-02 | A visible, unambiguous indicator shows when the microphone is live.                                              |
| V-03 | Recognised speech appears as text before it is acted on.                                                         |
| V-04 | The transcript is matched against the NEXUS-009 command registry using the existing rule.                        |
| V-05 | A confident single match is presented for confirmation, not executed silently.                                   |
| V-06 | An unrecognised transcript reports what was heard and performs nothing.                                          |
| V-07 | Listening stops automatically after a bounded silence and on any error.                                          |
| V-08 | Denied or revoked microphone permission is reported clearly and leaves the application fully usable by keyboard. |
| V-09 | Voice can be disabled entirely from Settings, and is **off by default**.                                         |
| V-10 | No audio and no transcript is ever written to the database or to disk.                                           |

**V-05 is not negotiable.** A misrecognised transcript that silently creates or navigates is worse than no voice feature at all. Voice proposes; the user confirms.

**V-09 defaults to off.** A microphone capability that activates without the user opting in is a privacy defect regardless of where the audio goes.

### 2.2 Explicit non-goals

Dictation into text fields. Spoken output. Wake words or always-on listening. Continuous transcription. Multi-turn conversation. Any command the keyboard palette does not offer. Speaker identification. Audio recording, storage, or replay.

---

## 3. The blocking decision

Two options were named. A third exists and is recorded because it may be the best answer; the choice remains yours.

### 3.1 Option 1: WebKit `SpeechRecognition`

The Web Speech API, called from the frontend.

**For.** No new dependency in either manifest. No model file, so no bundle-size change. Smallest possible amount of code.

**Against, and this is decisive.** On Apple platforms the Web Speech API is backed by Apple's speech services, and the web-facing API **exposes no equivalent of `SFSpeechRecognizer.requiresOnDeviceRecognition`**. The application cannot compel on-device recognition, cannot verify whether a given utterance was processed locally, and cannot prevent audio being sent to Apple's servers.

**That is a direct conflict with the local-first constraint** held since NEXUS-002 and restated in every specification since: "Local-first. No remote backend." Adopting Option 1 means either accepting that audio leaves the machine, or asserting a privacy property that cannot be verified.

**It may also not be available at all.** See the spike in section 4.

### 3.2 Option 2: bundled local speech-to-text engine

A speech engine compiled into the application, for example `whisper.cpp` or Vosk, invoked from Rust.

**For.** Audio never leaves the machine, and that is verifiable by inspection rather than asserted. Fully satisfies local-first. Works with no network at all.

**Against.** A new Rust dependency, which the standing rules forbid without explicit approval. A model file measured in tens to hundreds of megabytes, against a current bundle around 38 MB, so either the bundle grows several-fold or a first-run download is introduced, and a first-run download is itself a network dependency. Build complexity and platform-specific compilation. Latency and accuracy vary with model size. Licensing of both engine and model weights needs review.

### 3.3 Option 3, not among the two you named: native `SFSpeechRecognizer` with on-device recognition forced

Apple's speech framework, bridged from Rust, with `requiresOnDeviceRecognition = true`.

**For.** Genuinely on-device and, unlike Option 1, the application sets the flag itself and can refuse to proceed if on-device recognition is unavailable. No model to ship: the OS supplies it. Bundle size effectively unchanged. Accuracy is good and macOS-native, which suits a macOS-first application.

**Against.** Requires Objective-C interop, so a new Rust dependency such as `objc2`, though a far smaller one than a speech engine. On-device recognition requires the language model to be present, which the OS may need to download once and which the app cannot force. Ties the feature to macOS, which is consistent with the project's stated platform but forecloses portability.

### 3.4 Comparison

|                                   | Option 1: WebKit              | Option 2: bundled engine | Option 3: native on-device |
| --------------------------------- | ----------------------------- | ------------------------ | -------------------------- |
| Audio stays local                 | **No, and unverifiable**      | Yes, verifiable          | Yes, flag set by the app   |
| New Rust dependency               | None                          | Large                    | Small, interop only        |
| Bundle size impact                | None                          | Tens to hundreds of MB   | Negligible                 |
| First-run network                 | None                          | Likely, if not bundled   | Possible OS model download |
| Satisfies local-first             | **No**                        | Yes                      | Yes                        |
| Availability in Tauri's WKWebView | **Unverified, see section 4** | Not applicable           | Not applicable             |
| macOS-only                        | No                            | No                       | Yes                        |

### 3.5 Recommendation

**Option 1 should be rejected on the privacy constraint alone**, before the spike is even run. You stated that local-first must not be compromised; Option 1 compromises it in a way the application cannot detect, control, or disclose accurately to the user. The spike may make the rejection moot by showing the API is unavailable, but the constraint is the stronger reason.

Between the remaining two, **Option 3 is the better first move**: it is verifiably on-device, adds interop rather than an engine, and leaves the bundle roughly where it is. Option 2 is the right answer only if cross-platform support or independence from Apple's stack becomes a requirement, neither of which is on the roadmap today.

**No option may be adopted without your explicit approval of the dependency it brings.** That is the standing rule, and this document does not vary it.

---

## 4. Mandatory spike, before any decision

A throwaway investigation. Its output is a written finding, not a branch, and any code written for it is discarded.

| #    | Question                                                                                 | Method                                                                    | Why it matters                                                                    |
| ---- | ---------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| S-01 | Does `SpeechRecognition` or `webkitSpeechRecognition` exist in Tauri's WKWebView at all? | Evaluate `typeof webkitSpeechRecognition` in the running app's console    | If absent, Option 1 is impossible and the decision narrows to two                 |
| S-02 | If present, does it function without a Safari-specific entitlement?                      | Attempt one recognition in a scratch build                                | Presence is not the same as permission                                            |
| S-03 | Can on-device recognition be requested or verified through the Web API?                  | API surface review                                                        | Determines whether the local-first conflict is avoidable                          |
| S-04 | What does macOS require for microphone access from a Tauri bundle?                       | `NSMicrophoneUsageDescription`, TCC prompt behaviour, sandbox entitlement | Applies to every option                                                           |
| S-05 | For Option 2, what is the smallest usable model and its on-disk size?                    | Measure                                                                   | Decides bundle growth against a ~38 MB baseline                                   |
| S-06 | For Option 3, which crate provides the interop and what does it pull in?                 | Dependency tree inspection                                                | Decides whether the dependency is acceptable                                      |
| S-07 | For Options 2 and 3, what is the latency from utterance end to transcript?               | Measure on the target machine                                             | A three-second wait makes voice slower than typing, which would end the milestone |

**S-01 is expected to fail.** The Web Speech API has historically been a Safari-only surface rather than a general WKWebView one. That expectation is **not verified** and must not be treated as fact until S-01 is run.

**S-07 is the honest kill criterion.** If voice is slower than the keyboard palette for every command the palette offers, the feature has no user value and should be abandoned rather than shipped.

---

## 5. Constraints that do not move

Whichever option wins.

| ID   | Constraint                                                                                                                                                              |
| ---- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| C-01 | **Local-first is not tradeable.** If audio would leave the machine, the option is rejected. No exception for convenience, accuracy, or implementation cost.             |
| C-02 | **No AI, no model inference beyond speech-to-text, no LLM.** The transcript goes to the NEXUS-009 registry, never to a language model.                                  |
| C-03 | **No spoken output.** Text to speech stays out of scope.                                                                                                                |
| C-04 | **No audio or transcript persistence.** Not to the database, not to a file, not to a log. Audio buffers are released as soon as recognition completes.                  |
| C-05 | **Voice is off by default** and disableable in Settings.                                                                                                                |
| C-06 | **Voice grants no new capability.** Every voice-reachable action is already reachable from the NEXUS-009 palette.                                                       |
| C-07 | **Keyboard and mouse parity.** The application remains fully usable with voice permanently denied, and that must be verified with permission revoked.                   |
| C-08 | **Nothing is executed without confirmation** (V-05).                                                                                                                    |
| C-09 | **No new dependency without explicit approval**, named and justified before it is added.                                                                                |
| C-10 | **Migration 001 stays unchanged** unless a persisted voice preference genuinely requires schema, which it does not: `settings` already exists and holds arbitrary keys. |
| C-11 | `AppShell` must still import nothing from `src/lib/nexus-db.ts`. No React Context.                                                                                      |

---

## 6. Provisional architecture

Sketched to size the work. Not approved, and it changes with the option chosen.

```
CommandBar microphone button
        │  start / stop
        ▼
VoiceController            (NEW: permission, lifecycle, indicator state)
        │  transcript: string
        ▼
NEXUS-009 command registry (UNCHANGED: matchesQuery over labels + keywords)
        │  candidate command or search query
        ▼
CommandPalette             (opens pre-filled, awaits confirmation per V-05)
        │  user presses Enter
        ▼
navigate(...)              (UNCHANGED: existing screen handlers do the work)
```

The transcript enters the existing palette rather than bypassing it. Voice therefore inherits confirmation, keyboard correction, the visible result list, and every guarantee NEXUS-009 already established, without duplicating any of them.

**Option 1** would place recognition in the frontend, in `VoiceController`. **Options 2 and 3** would place it in Rust behind a command such as `nexus_transcribe_audio`, keeping the audio path out of the webview entirely, which is the better shape for both.

A `voice_enabled` key would be added to the NEXUS-008 settings layer. It needs no schema change: `settings` is key/value and the typed layer already tolerates unknown keys and applies defaults, so the addition is one constant, one struct field, and one form control.

---

## 7. Platform requirements

Applies to every option.

- **`NSMicrophoneUsageDescription`** in the bundle's `Info.plist`, with a truthful, specific string. It is shown to the user in the TCC prompt and is the only explanation they get.
- **macOS TCC prompt** on first use. The application must handle denial gracefully at every subsequent launch, not only the first.
- **Permission can be revoked** in System Settings at any time while the app is running. Recovery must not require a restart.
- **Sandbox entitlement** `com.apple.security.device.audio-input` if the bundle is ever sandboxed. It is not today.
- **Bundle identifier `com.nexus.desktop` does not change.** Adding an entitlement must not alter it, or every user's database silently moves.

That last point deserves emphasis: the database path is derived from the bundle identifier. Any packaging change made for microphone access must be verified not to move it.

---

## 8. Provisional task outline

Deliberately coarse. Detailed tasks are written when the milestone is unblocked, not before.

| Phase | Work                                                                                                                    |
| ----- | ----------------------------------------------------------------------------------------------------------------------- |
| P-0   | Run the section 4 spike. Write the findings. **Stop.**                                                                  |
| P-1   | Decide the option. Approve or reject any dependency it brings. **Stop.**                                                |
| P-2   | Packaging: usage description, entitlement if needed, verify the database path is unmoved.                               |
| P-3   | Recognition layer, in Rust or the frontend per the decision.                                                            |
| P-4   | `VoiceController`: permission, lifecycle, silence timeout, error paths.                                                 |
| P-5   | Microphone button wiring and the live indicator.                                                                        |
| P-6   | Transcript into the palette, awaiting confirmation.                                                                     |
| P-7   | `voice_enabled` in Settings, defaulting off.                                                                            |
| P-8   | Tests: Rust for any transcription boundary; manual for everything else.                                                 |
| P-9   | Manual validation including permission denied, permission revoked mid-session, silence, misrecognition, and no-network. |

---

## 9. Gate: definition of ready

NEXUS-010 may not start until **all** of the following are true.

- [ ] NEXUS-004 through NEXUS-008 manual validation is complete and any defects are fixed.
- [ ] NEXUS-009 is implemented, validated, and merged, so a command registry exists.
- [ ] The placeholder and microphone-button decision from 1.3 is made.
- [ ] The section 4 spike is complete and its findings are written down.
- [ ] S-01 has an answer. If `SpeechRecognition` is unavailable in the WKWebView, Option 1 is formally struck.
- [ ] S-07 has a measured latency, and it is fast enough to beat typing the same command.
- [ ] An option is chosen **in writing**, with the local-first implication stated explicitly.
- [ ] Every dependency the chosen option requires is named and **explicitly approved**.
- [ ] The bundle-size impact is known and accepted.
- [ ] It is confirmed that no first-run download is introduced, or that one is explicitly accepted despite the network implication.
- [ ] This document is rewritten as a full specification at NEXUS-003 through NEXUS-009 depth, with real requirements, IPC contracts, tasks, acceptance criteria, and a manual checklist.

Until every box is ticked, the microphone button stays exactly as NEXUS-001 left it.

---

## 10. Explicitly Out of Scope

Permanently, or until separately approved:

- **Text to speech and any spoken output.** Standing out-of-scope item since NEXUS-004.
- **Wake words, hotwords, or always-on listening.** Listening starts only on explicit user action.
- **Continuous or background transcription.**
- **Natural-language understanding, intent classification, or an LLM in the voice path.** The transcript meets a static registry and nothing else.
- **Conversational or multi-turn interaction.**
- **Dictation into text fields.**
- **Voice commands with no keyboard equivalent** (C-06).
- **Speaker identification, voice profiles, or biometrics.**
- **Storing, logging, or transmitting audio or transcripts** (C-04).
- **Any cloud speech service**: Apple, Google, Whisper API, or otherwise. Cloud recognition is rejected by C-01, not merely deferred.
- **Voice on any platform other than macOS**, unless Option 2 is chosen and portability is separately requested.

Also out of scope, per the standing NEXUS constraints:

- Jira, Claude, PlayerZero, Cursor, Grok, and ChatGPT integrations
- AI orchestration or execution of any kind
- IDE launching, terminal execution, browser automation
- News, weather, morning briefings, notifications
- Authentication, cloud sync, CI/CD, auto-update
- Custom title bar or window chrome
- Any routing library, state manager, UI library, form library, or ORM
- Any new Rust or frontend dependency without explicit prior approval
