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
} from '../../lib/nexus-db';
import type { VoiceStatus } from '../../types/db';
import './VoiceController.css';

interface VoiceControllerProps {
  /** Mirrors settings.voiceEnabled. False means the mic never starts. */
  enabled: boolean;
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
  active,
  onActiveChange,
  onFinalTranscript,
}: VoiceControllerProps) {
  const [status, setStatus] = useState<VoiceStatus | null>(null);
  const [listening, setListening] = useState(false);
  const [partial, setPartial] = useState('');
  const [error, setError] = useState<string | null>(null);
  // Held in a ref, not state, so a stale render cannot resurrect it. Cleared
  // on every stop. Never written to disk or database (V-10, C-04).
  const finalRef = useRef<string>('');

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
        if (!s.listening) {
          const text = finalRef.current.trim();
          finalRef.current = '';
          setPartial('');
          onActiveChange(false);
          if (text.length > 0 && s.reason !== 'error') {
            onFinalTranscript(text);
          }
        }
      });
      const c = await onVoiceError((message) => {
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

  // Start and stop follow the `active` flag the CommandBar toggles.
  useEffect(() => {
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
  }, [active, enabled, onActiveChange]);

  // Voice turned off in Settings while listening: stop immediately.
  useEffect(() => {
    if (!enabled && active) onActiveChange(false);
  }, [enabled, active, onActiveChange]);

  if (!enabled) return null;

  const unsupported = status !== null && !status.supportsOnDevice;

  return (
    <>
      {listening && (
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
