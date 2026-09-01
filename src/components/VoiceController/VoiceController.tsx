import { useCallback, useEffect, useRef, useState } from 'react';
import { Mic, MicOff } from 'lucide-react';
import {
  onVoiceError,
  onVoiceState,
  onVoiceTranscript,
  voiceRequestAuthorization,
  voiceStart,
  voiceStatus,
  voiceStop,
  voiceStopSpeaking,
  voiceSay,
  voiceSyncAlwaysListening,
  voiceWake,
} from '../../lib/nexus-db';
import type { VoiceStatus } from '../../types/db';
import './VoiceController.css';

/**
 * How long NEXUS waits for a command after answering to its name.
 *
 * Long enough to think of what to say, short enough that a forgotten wake
 * word does not turn the next unrelated sentence into a command.
 */
const WAKE_WINDOW_MS = 10_000;

/**
 * How long NEXUS waits after asking the user something.
 *
 * Longer than the wake window because the question is spoken first, and the
 * microphone is shut for the length of it: "I can list open tabs, or read
 * the current page. Which one?" spends several seconds of the window before
 * the user can say anything at all.
 */
const ANSWER_WINDOW_MS = 20_000;

interface VoiceControllerProps {
  /** Mirrors settings.voiceEnabled. False means the mic never starts. */
  enabled: boolean;
  /**
   * Mirrors settings.alwaysListening. When true the microphone is kept open
   * by a supervisor in Rust and only utterances carrying the wake word are
   * acted on; `active` no longer drives the session.
   */
  alwaysListening: boolean;
  /**
   * Incremented each time NEXUS asks the user something. A counter rather
   * than a flag: two questions in a row must each open the microphone, and
   * a boolean would need resetting between them.
   */
  expectAnswer: number;
  /** Set by the CommandBar microphone button. */
  active: boolean;
  onActiveChange: (active: boolean) => void;
  /**
   * Called with a FINAL transcript.
   *
   * Since NEXUS-014 the controller no longer resolves the transcript itself:
   * it hands the words to the assistant, which walks the same deterministic
   * ladder for spoken and typed input. The controller still never executes
   * anything, and the permission gate still enforces confirmation.
   */
  onFinalTranscript: (text: string) => void;
}

export function VoiceController({
  enabled,
  alwaysListening,
  expectAnswer,
  active,
  onActiveChange,
  onFinalTranscript,
}: VoiceControllerProps) {
  const [status, setStatus] = useState<VoiceStatus | null>(null);
  const [listening, setListening] = useState(false);
  const [partial, setPartial] = useState('');
  /**
   * The last thing NEXUS heard clearly and deliberately did nothing with.
   *
   * Shown briefly so an ignored sentence is distinguishable from one that
   * was never picked up. Without it the only difference between "you did
   * not say Nexus" and "the microphone is broken" is that one of them
   * flashes some text first.
   */
  const [ignored, setIgnored] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Held in a ref, not state, so a stale render cannot resurrect it. Cleared
  // on every stop. Never written to disk or database (V-10, C-04).
  const finalRef = useRef<string>('');
  // Waiting for a command after answering to its name. A timestamp rather
  // than a flag, so a window that was never used simply lapses.
  const armedUntilRef = useRef<number>(0);
  const [armed, setArmed] = useState(false);

  const armFor = useCallback((ms: number) => {
    armedUntilRef.current = Date.now() + ms;
    setArmed(true);
  }, []);

  const arm = useCallback(() => armFor(WAKE_WINDOW_MS), [armFor]);

  const disarm = useCallback(() => {
    armedUntilRef.current = 0;
    setArmed(false);
  }, []);

  /**
   * Decide what an overheard utterance was.
   *
   * Everything not addressed to NEXUS is dropped here without being
   * resolved, shown, or run. That discarding is the whole reason a
   * permanently open microphone is tolerable.
   */
  const handleHeard = useCallback(
    async (text: string) => {
      const armed = Date.now() < armedUntilRef.current;

      try {
        // Detection runs first, armed or not, and that is a correction.
        //
        // The armed branch used to short-circuit and forward the transcript
        // verbatim. Being armed means a wake word is *not required*, which is
        // not the same as a wake word being *ignored*: someone who says "Hey
        // Nexus, sign in to Microsoft" inside the window had "Hey Nexus"
        // passed through to the resolver, which then matched nothing and
        // answered nothing. The same sentence worked perfectly a moment
        // earlier or later, which is the worst kind of intermittent.
        const outcome = await voiceWake(text);

        if (outcome.woke && outcome.command) {
          // A wake word with a command in the same breath. Stripped, whether
          // or not the window happened to be open.
          disarm();
          onFinalTranscript(outcome.command);
          return;
        }

        if (outcome.woke) {
          // The wake word alone. Acknowledge and listen, even while armed:
          // saying NEXUS's name again is a fresh start, not an answer.
          // Armed before speaking, not after: the reply takes a moment and
          // the user may start talking over the end of it.
          arm();
          if (outcome.reply) await voiceSay(outcome.reply);
          return;
        }

        // No wake word. Inside the window that is expected, because NEXUS
        // just asked something; outside it, this is the room talking and
        // dropping it is the whole reason an open microphone is tolerable.
        if (armed) {
          disarm();
          onFinalTranscript(text);
          return;
        }

        // Dropped, and say so. The indicator was showing the words as they
        // were recognised and then doing nothing with them, which reads as
        // NEXUS taking the command and failing rather than declining to
        // take it at all. Heard-and-ignored is a different state from
        // not-heard and has to look like one.
        setIgnored(text);
      } catch (err) {
        setError(String(err));
      }
    },
    [arm, disarm, onFinalTranscript],
  );

  // The state listener is installed once, so it reads these through refs
  // rather than being torn down and rebuilt on every change.
  const alwaysRef = useRef(alwaysListening);
  const heardRef = useRef(handleHeard);
  useEffect(() => {
    alwaysRef.current = alwaysListening;
  }, [alwaysListening]);
  useEffect(() => {
    heardRef.current = handleHeard;
  }, [handleHeard]);

  // The hint is a nudge, not a log. It clears on its own, and immediately if
  // the next thing said does wake NEXUS.
  useEffect(() => {
    if (ignored === null) return;
    const timer = window.setTimeout(() => setIgnored(null), 6000);
    return () => window.clearTimeout(timer);
  }, [ignored]);

  useEffect(() => {
    if (armed) setIgnored(null);
  }, [armed]);

  // Let a window that was never used lapse in the UI too.
  useEffect(() => {
    if (!armed) return;
    const remaining = Math.max(0, armedUntilRef.current - Date.now());
    const timer = window.setTimeout(disarm, remaining);
    return () => window.clearTimeout(timer);
  }, [armed, disarm]);

  // NEXUS just asked a question. Open the microphone for the answer rather
  // than making the user reach for the button mid-conversation.
  useEffect(() => {
    if (expectAnswer === 0 || !enabled) return;
    if (alwaysListening) {
      armFor(ANSWER_WINDOW_MS);
    } else {
      // Not always-listening: there is no open session to arm, so start one.
      // It closes itself on silence exactly as a button press would.
      onActiveChange(true);
    }
    // Only a new question should reopen the microphone. Re-running because
    // a callback changed identity would reopen it at random.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [expectAnswer]);

  // Subscribe once. Transcripts live only for the length of a session.
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    let cancelled = false;

    void (async () => {
      const a = await onVoiceTranscript((t) => {
        setPartial(t.text);
        // Keep the latest text, final or not. Stopping manually cancels the
        // task before a final result arrives, and discarding partials there
        // would throw away what the user actually said.
        if (t.text.trim().length > 0) {
          finalRef.current = t.text;
        }
      });
      const b = await onVoiceState((s) => {
        setListening(s.listening);
        if (s.listening) return;

        const text = finalRef.current.trim();
        finalRef.current = '';
        setPartial('');

        if (!alwaysRef.current) {
          onActiveChange(false);
          if (text.length > 0 && s.reason !== 'error') {
            onFinalTranscript(text);
          }
          return;
        }

        // In always-listening the session ending is routine: the supervisor
        // opens the next one. The button is not tied to it, so it is left
        // alone here.
        if (text.length === 0 || s.reason === 'error') return;
        void heardRef.current(text);
      });
      const c = await onVoiceError((message) => {
        // A silent session is not a fault when the microphone never closes.
        //
        // The supervisor opens a session, the room says nothing, Apple
        // reports "no speech detected", and the next session opens. That is
        // the normal resting state of always-listening, not an error, and
        // surfacing it put a banner on screen blaming Dictation for being
        // off while Dictation was plainly on. An error message that is
        // usually wrong teaches the user to ignore the ones that are right.
        //
        // Outside always-listening it stays an error, because there the user
        // pressed a button and got nothing, which is worth saying.
        if (alwaysRef.current && /no speech/i.test(message)) return;

        setError(message);
        finalRef.current = '';
        setPartial('');
      });
      if (cancelled) {
        a(); b(); c();
      } else {
        unlisteners.push(a, b, c);
      }
    })();

    return () => {
      cancelled = true;
      unlisteners.forEach((u) => u());
    };
  }, [onActiveChange, onFinalTranscript]);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await voiceStatus());
    } catch (err) {
      setError(String(err));
    }
  }, []);

  useEffect(() => {
    if (enabled) void refreshStatus();
  }, [enabled, refreshStatus]);

  // Keep the Rust supervisor in step with the saved preference.
  useEffect(() => {
    void (async () => {
      try {
        await voiceSyncAlwaysListening();
      } catch (err) {
        setError(String(err));
      }
    })();
  }, [enabled, alwaysListening]);

  // Start and stop follow the `active` flag the CommandBar toggles.
  //
  // Skipped entirely in always-listening: the supervisor owns the session
  // there, and a second thing opening and closing the microphone would race
  // it. The button arms the command window instead.
  useEffect(() => {
    if (alwaysListening) {
      if (active) {
        arm();
        onActiveChange(false);
      }
      return;
    }
    let cancelled = false;
    void (async () => {
      if (active && enabled) {
        setError(null);
        try {
          // NEXUS-011: silence any response still being spoken before the
          // microphone opens. Rust refuses to speak while listening; this is
          // the other half of that guarantee, so NEXUS cannot capture the
          // tail of its own last sentence as the next command.
          await voiceStopSpeaking();
          const s = await voiceStatus();
          if (s.authorization === 'notDetermined') {
            await voiceRequestAuthorization();
          }
          if (cancelled) return;
          await voiceStart();
        } catch (err) {
          if (!cancelled) {
            setError(String(err));
            onActiveChange(false);
          }
        }
      } else if (!active) {
        try {
          await voiceStop();
        } catch {
          // Stopping an idle session is not an error worth surfacing.
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [active, enabled, alwaysListening, arm, onActiveChange]);

  // Voice turned off in Settings while listening: stop immediately.
  useEffect(() => {
    if (!enabled && active) onActiveChange(false);
  }, [enabled, active, onActiveChange]);

  if (!enabled) return null;

  const unsupported = status !== null && !status.supportsOnDevice;

  return (
    <>
      {/*
        Always-listening renders its own indicator, shown whether or not a
        session happens to be open. The supervisor recycles the session every
        45 seconds and after every utterance; tying the indicator to
        `listening` would blink it each time and read as a fault.
      */}
      {alwaysListening && (
        <div
          className={`voice-indicator${armed ? ' voice-indicator--armed' : ''}`}
          role="status"
          aria-live="polite"
        >
          <span className="voice-indicator__dot" aria-hidden="true" />
          <span className="voice-indicator__label">
            {armed ? 'Go ahead' : 'Say "Nexus"'}
          </span>
          <span className="voice-indicator__partial">
            {ignored !== null && !armed ? (
              <>
                Heard &ldquo;{ignored}&rdquo;. Start with &ldquo;Nexus&rdquo; and
                I will act on it.
              </>
            ) : (
              partial || (armed ? 'Listening for your command...' : '')
            )}
          </span>
          {armed && (
            <button
              className="nexus-btn nexus-btn--secondary"
              type="button"
              onClick={disarm}
            >
              Cancel
            </button>
          )}
        </div>
      )}

      {!alwaysListening && listening && (
        <div className="voice-indicator" role="status" aria-live="polite">
          <span className="voice-indicator__dot" aria-hidden="true" />
          <span className="voice-indicator__label">Listening</span>
          <span className="voice-indicator__partial">
            {partial || 'Say a command...'}
          </span>
          <button
            className="nexus-btn nexus-btn--secondary"
            type="button"
            onClick={() => onActiveChange(false)}
          >
            Stop
          </button>
        </div>
      )}

      {(error || unsupported) && !listening && (
        <div className="voice-indicator voice-indicator--error" role="alert">
          <MicOff size={12} strokeWidth={2} aria-hidden="true" />
          <span className="voice-indicator__partial">
            {unsupported
              ? 'On-device speech recognition is unavailable for this language. NEXUS will not use remote recognition, so voice stays off. The keyboard palette is unaffected.'
              : error}
            {/*
              SFSpeechRecognizer is provisioned through Dictation, and
              isAvailable() still reports true when Dictation is off, so this
              only surfaces as a task error. Apple's message names the cause
              but not the remedy, so the remedy is added here.
            */}
            {!unsupported && (
              <span className="voice-indicator__hint">
                macOS provides speech recognition through Dictation. If it is
                off, turn it on in System Settings &rsaquo; Keyboard &rsaquo;
                Dictation. NEXUS still recognises on-device only.
              </span>
            )}
          </span>
          <button
            className="nexus-btn nexus-btn--secondary"
            type="button"
            onClick={() => setError(null)}
          >
            Dismiss
          </button>
        </div>
      )}
    </>
  );
}

/** Exported for the CommandBar button label. */
export const VoiceIcon = Mic;
