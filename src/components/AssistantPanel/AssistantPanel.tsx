import { useCallback, useEffect, useRef, useState } from 'react';
import { CornerDownLeft, Mic, Sparkles, X } from 'lucide-react';
import { allCommands } from '../../lib/commands';
import {
  assistantAsk,
  assistantSettle,
  assistantSnapshot,
  cancelApproval,
  describeActionError,
  isActionError,
  isNeedsApproval,
  onAssistantState,
  runAction,
} from '../../lib/assistant';
import { actionForCommand, viewFromOutput } from '../../lib/palette-actions';
import { listProjects } from '../../lib/nexus-db';
import type {
  AssistantReply,
  AssistantState,
  Choice,
  SessionSnapshot,
} from '../../types/assistant';
import type { NexusView } from '../../types';
import type { Project } from '../../types/db';
import { ApprovalPrompt } from '../ApprovalPrompt/ApprovalPrompt';
import type { NeedsApproval } from '../../types/assistant';
import './AssistantPanel.css';

interface AssistantPanelProps {
  open: boolean;
  onClose: () => void;
  navigate: (view: NexusView) => void;
  /** Mirrors settings.voiceEnabled; the mic button is inert without it. */
  voiceEnabled: boolean;
  voiceActive: boolean;
  onVoiceToggle: () => void;
  /**
   * A final transcript from NEXUS-010. The panel treats it exactly like typed
   * input, which is what makes voice and text one conversation rather than
   * two.
   */
  spokenText?: string | null;
  onSpokenConsumed: () => void;
}

const STATE_LABEL: Record<AssistantState, string> = {
  idle: 'Ready',
  listening: 'Listening',
  thinking: 'Thinking',
  awaitingConfirmation: 'Waiting for you',
  executing: 'Working',
  completed: 'Done',
  failed: 'Stopped',
  cancelled: 'Cancelled',
};

export function AssistantPanel({
  open,
  onClose,
  navigate,
  voiceEnabled,
  voiceActive,
  onVoiceToggle,
  spokenText,
  onSpokenConsumed,
}: AssistantPanelProps) {
  const [snapshot, setSnapshot] = useState<SessionSnapshot | null>(null);
  const [draft, setDraft] = useState('');
  const [reply, setReply] = useState<AssistantReply | null>(null);
  const [pending, setPending] = useState<NeedsApproval | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const inputRef = useRef<HTMLInputElement>(null);
  const logRef = useRef<HTMLDivElement>(null);

  // ── State, pushed rather than polled ─────────────────────────────────────

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;

    void (async () => {
      try {
        const initial = await assistantSnapshot();
        if (!cancelled) setSnapshot(initial);
      } catch (err) {
        if (!cancelled) setError(String(err));
      }
      const stop = await onAssistantState((next) => setSnapshot(next));
      if (cancelled) stop();
      else unlisten = stop;
    })();

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (open) inputRef.current?.focus();
  }, [open]);

  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [snapshot, reply]);

  // ── Running what NEXUS resolved ──────────────────────────────────────────

  /**
   * Run a registry command through the same bridge and the same gate the
   * palette uses. There is deliberately no second path: the conversation is
   * another way to reach the action system, not another action system.
   */
  const run = useCallback(
    async (commandId: string) => {
      const request = actionForCommand(commandId);
      if (!request) {
        setError(`NEXUS does not know how to run "${commandId}".`);
        return;
      }
      setBusy(true);
      setError(null);
      try {
        const outcome = await runAction(request);
        const view = viewFromOutput(outcome.output);
        if (view) navigate(view);
        setReply({ kind: 'answer', text: outcome.summary, cited: [] });
      } catch (err) {
        if (isNeedsApproval(err)) {
          setPending(err);
        } else {
          setError(isActionError(err) ? describeActionError(err) : String(err));
        }
      } finally {
        setBusy(false);
      }
    },
    [navigate],
  );

  const ask = useCallback(
    async (text: string, spoken: boolean) => {
      const trimmed = text.trim();
      if (trimmed.length === 0 || busy) return;

      setBusy(true);
      setError(null);
      setReply(null);
      try {
        const projects: Project[] = await listProjects();
        const specs = allCommands(projects).map((c) => ({
          id: c.id,
          label: c.label,
          keywords: c.keywords,
        }));
        const answer = await assistantAsk(
          trimmed,
          spoken,
          specs,
          projects.map((p) => p.name),
        );
        setReply(answer);
        setDraft('');

        // A single clear action runs immediately. It still passes the gate,
        // so anything that writes or deletes stops for confirmation exactly
        // as it would from the palette.
        if (answer.kind === 'action') {
          await run(answer.commandId);
        }
      } catch (err) {
        setError(String(err));
      } finally {
        setBusy(false);
      }
    },
    [busy, run],
  );

  // A final transcript is treated exactly like typed input.
  useEffect(() => {
    if (!spokenText) return;
    onSpokenConsumed();
    void ask(spokenText, true);
    // `ask` is stable enough for this: re-running on a new transcript is the
    // intent, and re-running on an identity change is not.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [spokenText]);

  if (!open) return null;

  const state = snapshot?.state ?? 'idle';
  const turns = snapshot?.turns ?? [];
  const referents = snapshot?.referents ?? [];

  return (
    <aside className="assistant-panel" aria-label="NEXUS assistant">
      <header className="assistant-panel__head">
        <span className="assistant-panel__title">
          <Sparkles size={13} strokeWidth={2} aria-hidden="true" />
          Assistant
        </span>
        <span
          className={`assistant-panel__state assistant-panel__state--${state}`}
          role="status"
        >
          {STATE_LABEL[state]}
        </span>
        <button
          className="assistant-panel__close"
          type="button"
          onClick={onClose}
          aria-label="Close the assistant"
        >
          <X size={14} strokeWidth={2} aria-hidden="true" />
        </button>
      </header>

      <div className="assistant-panel__log" ref={logRef}>
        {turns.length === 0 && !reply && (
          <p className="assistant-panel__empty">
            Ask NEXUS to open something, or what your blocked tasks are. It
            answers from your workspace and runs commands through the same
            permission gate as everything else.
          </p>
        )}

        {turns.map((turn) => (
          <div className="assistant-panel__turn" key={turn.id}>
            {turn.input.source !== 'ui' && (
              <p className="assistant-panel__said">
                {turn.input.source === 'voice' && (
                  <Mic size={10} strokeWidth={2} aria-hidden="true" />
                )}
                {turn.input.text}
              </p>
            )}
            {turn.summary && (
              <p className="assistant-panel__reply">{turn.summary}</p>
            )}
            {turn.error && (
              <p className="assistant-panel__reply assistant-panel__reply--error">
                {turn.error}
              </p>
            )}
          </div>
        ))}

        {reply?.kind === 'answer' && reply.cited.length > 0 && (
          <p className="assistant-panel__cited">
            Based on {reply.cited.join(', ')}
          </p>
        )}

        {reply?.kind === 'choices' && (
          <div className="assistant-panel__choices">
            <p className="assistant-panel__reply">Which one?</p>
            {reply.candidates.map((choice: Choice) => (
              <button
                className="nexus-btn nexus-btn--secondary"
                type="button"
                key={choice.commandId}
                onClick={() => void run(choice.commandId)}
                disabled={busy}
              >
                {choice.label}
              </button>
            ))}
          </div>
        )}

        {pending && (
          <ApprovalPrompt
            request={pending}
            busy={busy}
            onApprove={() => {
              const token = pending.token;
              setPending(null);
              setBusy(true);
              // The approval belongs to whatever asked for it, so it is
              // redeemed by re-running that exact request.
              void (async () => {
                try {
                  const request = actionForCommand(
                    reply?.kind === 'action' ? reply.commandId : '',
                  );
                  if (request) {
                    await runAction({ ...request, approval: token });
                  }
                } catch (err) {
                  setError(
                    isActionError(err) ? describeActionError(err) : String(err),
                  );
                } finally {
                  setBusy(false);
                }
              })();
            }}
            onCancel={() => {
              void cancelApproval(pending.token);
              setPending(null);
            }}
          />
        )}

        {error && (
          <p className="assistant-panel__reply assistant-panel__reply--error" role="alert">
            {error}
          </p>
        )}
      </div>

      {referents.length > 0 && (
        <div className="assistant-panel__referents" aria-label="In this conversation">
          {referents.slice(-6).map((referent) => (
            <span className="nexus-chip" key={referent.id}>
              {referent.displayName}
            </span>
          ))}
        </div>
      )}

      <form
        className="assistant-panel__input-row"
        onSubmit={(e) => {
          e.preventDefault();
          void ask(draft, false);
        }}
      >
        <input
          ref={inputRef}
          className="assistant-panel__input"
          type="text"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder="Ask NEXUS..."
          aria-label="Ask NEXUS"
          autoComplete="off"
          spellCheck={false}
          disabled={busy}
        />
        <button
          className="assistant-panel__mic"
          type="button"
          onClick={onVoiceToggle}
          disabled={!voiceEnabled}
          aria-pressed={voiceActive}
          title={
            voiceEnabled
              ? 'Speak to NEXUS'
              : 'Turn on voice in Settings to use the microphone'
          }
        >
          <Mic size={13} strokeWidth={2} aria-hidden="true" />
        </button>
        <button
          className="assistant-panel__send"
          type="submit"
          disabled={busy || draft.trim().length === 0}
          aria-label="Send"
        >
          <CornerDownLeft size={13} strokeWidth={2} aria-hidden="true" />
        </button>
      </form>

      {(state === 'completed' || state === 'failed' || state === 'cancelled') && (
        <button
          className="assistant-panel__settle"
          type="button"
          onClick={() => void assistantSettle()}
        >
          Clear status
        </button>
      )}
    </aside>
  );
}
