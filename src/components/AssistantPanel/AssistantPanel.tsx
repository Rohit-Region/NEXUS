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
  onNotification,
  runAction,
} from '../../lib/assistant';
import { actionForCommand, viewFromOutput } from '../../lib/palette-actions';
import { listProjects, voiceSay } from '../../lib/nexus-db';
import type {
  ActionRequest,
  AssistantReply,
  AssistantState,
  Choice,
  SessionSnapshot,
} from '../../types/assistant';
import type { NexusView } from '../../types';
import type { Project } from '../../types/db';
import { ApprovalPrompt } from '../ApprovalPrompt/ApprovalPrompt';
import { ResultList } from '../ResultList/ResultList';
import type { NeedsApproval } from '../../types/assistant';
import './AssistantPanel.css';

interface AssistantPanelProps {
  open: boolean;
  onClose: () => void;
  /**
   * Show the panel. Needed because NEXUS-025 has NEXUS start conversations:
   * an announcement has to be able to open the surface it is announced on,
   * and only the shell owns that state.
   */
  onOpen: () => void;
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
  /**
   * Called when NEXUS has just asked the user something and is waiting on
   * the answer, so the microphone can open itself. Without this, being
   * asked "Which one?" required reaching for the mic button, which is the
   * one moment the user has both hands on nothing and a word ready to say.
   */
  onExpectAnswer: () => void;
}

/**
 * Which of the offered choices a spoken reply names.
 *
 * NEXUS asks "Which one?" and reads the options aloud, so the natural answer
 * is to say one of them back. Without this, saying "Rohit Jio" was treated
 * as a fresh command and escalated to a reasoning provider, which is a
 * baffling answer to a question NEXUS had just asked itself.
 *
 * Matching is on the whole phrase both ways round, because people answer
 * with more or fewer words than the label: "Rohit" for "Rohit Jio", or "the
 * Rohit Jio one". Ordinals work too, since the options are a rendered list.
 */
function chosen(text: string, candidates: Choice[]): Choice | null {
  const flat = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim();
  const said = flat(text);
  if (said === '') return null;

  const exact = candidates.find((c) => flat(c.label) === said);
  if (exact) return exact;

  // Contained either way. Longest label first, so "Divya" does not claim an
  // answer that named "Divya Raj".
  const contained = [...candidates]
    .sort((a, b) => b.label.length - a.label.length)
    .find((c) => {
      const label = flat(c.label);
      return said.includes(label) || label.includes(said);
    });
  if (contained) return contained;

  const ORDINALS = ['first', 'second', 'third', 'fourth', 'fifth'];
  const ordinal = ORDINALS.findIndex((word) => said === word || said === `the ${word}`);
  if (ordinal >= 0 && ordinal < candidates.length) return candidates[ordinal];

  return null;
}

/**
 * What a spoken reply to "Approve?" amounts to.
 *
 * Deliberately a short, closed list. Anything outside it is treated as not
 * an answer, because acting on an ambiguous "sure, but" would send a message
 * the user never confirmed. Matched on the whole phrase after stripping
 * punctuation, so "yes." and "Yes!" count and "yesterday" does not.
 */
function spokenVerdict(text: string): 'approve' | 'refuse' | 'unclear' {
  const words = text
    .toLowerCase()
    .replace(/[^a-z\s]/g, ' ')
    .split(/\s+/)
    .filter(Boolean);
  if (words.length === 0 || words.length > 3) return 'unclear';
  // Word sets rather than whole phrases. The exact-phrase list this replaced
  // matched 'send it' but not 'send it please', and nothing at all in
  // Hinglish, so 'haan bhej do' and 'send karo' came back unclear and the
  // approval sat there being re-asked. Every word below is one a person says
  // while answering a question, which is safe because this runs only while
  // an approval prompt is on screen.
  const APPROVE = new Set([
    'yes', 'yeah', 'yep', 'yup', 'ya', 'ok', 'okay', 'sure', 'right',
    'send', 'it', 'do', 'go', 'ahead', 'on', 'confirm', 'approve', 'please',
    'that', 'proceed', 'correct',
    // Hinglish, the way the sentence actually comes out.
    'haan', 'han', 'ha', 'haa', 'theek', 'thik', 'sahi', 'bilkul',
    'bhej', 'bhejo', 'bhejdo', 'karo', 'kar', 'kardo', 'de',
  ]);
  const REFUSE = new Set([
    'no', 'nope', 'nah', 'cancel', 'stop', 'dont', 'not', 'never', 'mind',
    'nevermind', 'forget', 'abort', 'wait', 'leave', 'skip', 'hold',
    'nahi', 'nahin', 'na', 'mat', 'rehne', 'rehnedo', 'chhod', 'chod',
    'chhodo', 'ruko', 'ruk',
  ]);
  // Refusal first: "no, don't send it" contains 'send', and reading that as
  // approval sends something that cannot be recalled. The asymmetry is
  // deliberate, and it matches the same rule on the Rust side.
  if (words.some((w) => REFUSE.has(w))) return 'refuse';
  if (words.every((w) => APPROVE.has(w))) return 'approve';
  return 'unclear';
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
  onOpen,
  navigate,
  voiceEnabled,
  voiceActive,
  onVoiceToggle,
  spokenText,
  onSpokenConsumed,
  onExpectAnswer,
}: AssistantPanelProps) {
  const [snapshot, setSnapshot] = useState<SessionSnapshot | null>(null);
  const [draft, setDraft] = useState('');
  const [reply, setReply] = useState<AssistantReply | null>(null);
  const [pending, setPending] = useState<NeedsApproval | null>(null);
  /**
   * The request the pending approval belongs to, kept whole.
   *
   * The token is bound in Rust to the action id and its exact input, so
   * redeeming it means repeating the same request. Anything reconstructed
   * afterwards is a different request and the gate refuses it.
   */
  const [pendingRequest, setPendingRequest] = useState<ActionRequest | null>(
    null,
  );
  /**
   * The last action's structured payload, so the screen can show everything
   * the connector found rather than only the sentence it reads aloud.
   */
  const [result, setResult] = useState<{ actionId: string; output: unknown } | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  /**
   * Say a reply out loud.
   *
   * Every reply, not only spoken ones: someone who has turned voice on wants
   * to hear answers whether they typed or spoke. The backend stays silent
   * when voice is off, so this needs no condition here.
   */
  const say = useCallback((text: string) => {
    if (!text.trim()) return;
    void voiceSay(text).catch(() => {
      /* A missing voice must never break the answer on screen. */
    });
  }, []);

  /**
   * Report a failure, on screen and aloud.
   *
   * Every error goes through here rather than calling setError directly. A
   * spoken assistant that goes silent precisely when something breaks is
   * worse than one that never spoke: the user is left waiting for a reply
   * that is sitting in red text they may not be looking at.
   */
  const fail = useCallback(
    (message: string) => {
      setError(message);
      say(message);
    },
    [say],
  );

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
  /**
   * Run one prepared request, and handle whatever comes back.
   *
   * Separate from `run` so the approval path can re-run the *exact* request
   * that was refused. Rebuilding it from the reply lost the arguments, and
   * lost the request entirely when it came from a suggestion rather than a
   * single resolved action.
   */
  const execute = useCallback(
    async (request: ActionRequest) => {
      setBusy(true);
      setError(null);
      try {
        const outcome = await runAction(request);
        const view = viewFromOutput(outcome.output);
        if (view) navigate(view);
        // What the action FOUND, not merely what it was asked to do. Before
        // this the panel showed "List open tabs" and discarded the tabs.
        const spoken = outcome.detail ?? outcome.summary;
        setReply({ kind: 'answer', text: spoken, cited: [] });
        setResult({ actionId: outcome.actionId, output: outcome.output });
        setPendingRequest(null);
        say(spoken);
        // Stay listening after answering. A conversation does not end
        // because one turn did, and needing the wake word again between
        // every exchange makes it feel like a series of transactions.
        onExpectAnswer();

        // Composing no longer offers to send.
        //
        // It did, and it sent a message to the wrong person: WhatsApp
        // ignores the phone in `whatsapp://send` while it is already
        // running, so the text went into whichever chat happened to be
        // open. Asking "shall I send?" put a confirmation in front of a
        // recipient NEXUS had not established, which made the prompt
        // actively misleading rather than merely unhelpful.
        //
        // Sending stays reachable by saying "send it" while looking at the
        // chat. It is not offered automatically again until the open chat
        // can be checked against the intended one.
      } catch (err) {
        if (isNeedsApproval(err)) {
          // Kept whole: the token is redeemed by repeating this request,
          // arguments included.
          setPendingRequest(request);
          setPending(err);
          say(`${err.summary}. Approve?`);
          onExpectAnswer();
        } else {
          setPendingRequest(null);
          fail(isActionError(err) ? describeActionError(err) : String(err));
        }
      } finally {
        setBusy(false);
      }
    },
    [navigate, say, fail, onExpectAnswer],
  );

  // The spoken-transcript effect fires on a new transcript alone, so it
  // reads these through refs rather than closing over a render's values.
  const pendingRef = useRef<NeedsApproval | null>(null);
  const pendingRequestRef = useRef<ActionRequest | null>(null);
  useEffect(() => {
    pendingRef.current = pending;
  }, [pending]);
  useEffect(() => {
    pendingRequestRef.current = pendingRequest;
  }, [pendingRequest]);

  /** The options currently on offer, for a spoken answer to match against. */
  const choicesRef = useRef<Choice[] | null>(null);
  useEffect(() => {
    choicesRef.current = reply?.kind === 'choices' ? reply.candidates : null;
  }, [reply]);

  /** Redeem the pending approval by repeating its exact request. */
  const approve = useCallback(() => {
    const held = pendingRef.current;
    const request = pendingRequestRef.current;
    if (!held) return;
    setPending(null);
    if (!request) {
      void cancelApproval(held.token);
      fail('That approval no longer matches anything. Ask again.');
      return;
    }
    void execute({ ...request, approval: held.token });
  }, [execute, fail]);

  /** Let the approval lapse, and say so rather than going quiet. */
  const decline = useCallback(() => {
    const held = pendingRef.current;
    if (!held) return;
    void cancelApproval(held.token);
    setPending(null);
    setPendingRequest(null);
    say('Cancelled.');
  }, [say]);

  const run = useCallback(
    async (commandId: string, input?: unknown) => {
      const mapped = actionForCommand(commandId);
      if (!mapped) {
        fail(`NEXUS does not know how to run "${commandId}".`);
        return;
      }
      // Arguments the resolver extracted from the phrase, for actions that
      // carry a target. A plain registry command supplies its own.
      await execute(
        input === undefined || input === null ? mapped : { ...mapped, input },
      );
    },
    [execute, fail],
  );

  const ask = useCallback(
    async (text: string, spoken: boolean) => {
      const trimmed = text.trim();
      if (trimmed.length === 0 || busy) return;

      setBusy(true);
      setError(null);
      setReply(null);
      setResult(null);
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

        if (answer.kind === 'answer') {
          say(answer.text);
          onExpectAnswer();
        } else if (answer.kind === 'unresolved') {
          if (!answer.understood && spoken) {
            // This used to say nothing at all, on the reasoning that an
            // always-open microphone mostly hears the room and announcing a
            // reason for every fragment would make NEXUS a heckler.
            //
            // That was true of a design where raw speech reached here. It is
            // not true of this one: in always-listening mode the Rust
            // supervisor only forwards utterances that carried the wake
            // word, so everything arriving here was addressed to NEXUS by
            // name. Silently discarding those means every command NEXUS
            // cannot place vanishes without a word, which is indistinguishable
            // from voice being broken -- and with no reasoning provider
            // configured, that is most of them.
            //
            // A fixed line rather than `answer.reason`: the reason is
            // usually the provider's ("ollama is not responding"), which
            // answers a question the user did not ask. What they need to
            // know is that NEXUS heard them and could not place it.
            say("I heard you, but I don't know how to do that yet.");
            onExpectAnswer();
            return;
          }
          say(answer.reason);
          onExpectAnswer();
        } else if (answer.kind === 'choices') {
          say(
            `I can ${answer.candidates
              .map((c) => c.label.toLowerCase())
              .join(', or ')}. Which one?`,
          );
          onExpectAnswer();
        }

        // A single clear action runs immediately. It still passes the gate,
        // so anything that writes or deletes stops for confirmation exactly
        // as it would from the palette.
        if (answer.kind === 'action') {
          await run(answer.commandId, answer.input);
        }
      } catch (err) {
        fail(String(err));
      } finally {
        setBusy(false);
      }
    },
    [busy, run, fail, onExpectAnswer],
  );

  /**
   * NEXUS-025. NEXUS starting the conversation.
   *
   * The listener lives here rather than in AppShell because this is where the
   * IPC boundary already is, and AppShell deliberately imports none. It runs
   * whether or not the panel is open: `open` gates the render, not the hooks,
   * so a message arriving while the panel is shut is still announced.
   *
   * Rust has already decided to speak, chosen the words, and created the
   * offer the answer resolves against. Everything left is saying it and
   * listening for the reply.
   */
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    void (async () => {
      const stop = await onNotification((poll) => {
        if (!poll.announcement) return;
        onOpen();
        say(poll.announcement);
        // NEXUS asked, so it has to listen for the answer the same way it
        // does for its own questions.
        onExpectAnswer();
      });
      if (cancelled) stop();
      else unlisten = stop;
    })();

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [say, onOpen, onExpectAnswer]);

  // A final transcript is treated exactly like typed input, except while
  // NEXUS is waiting on an approval: there, "yes" is an answer to the
  // question it just asked, not a new command.
  useEffect(() => {
    if (!spokenText) return;
    onSpokenConsumed();

    if (pendingRef.current) {
      const verdict = spokenVerdict(spokenText);
      if (verdict === 'approve') {
        approve();
        return;
      }
      if (verdict === 'refuse') {
        decline();
        return;
      }
      // Anything else is not an answer to "Approve?". Running it as a
      // command would leave the approval hanging and act on something the
      // user did not confirm, so NEXUS asks again and keeps listening.
      say('Say yes to go ahead, or no to cancel.');
      onExpectAnswer();
      return;
    }

    // NEXUS asked "Which one?" and read the options out. Saying one back is
    // an answer to that question, not a new request.
    const offered = choicesRef.current;
    if (offered && offered.length > 0) {
      const pick = chosen(spokenText, offered);
      if (pick) {
        void run(pick.commandId, pick.input);
        return;
      }
      // No match falls through deliberately: the user is allowed to change
      // their mind mid-question rather than being held at it.
    }

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

        {result && (
          <ResultList
            actionId={result.actionId}
            output={result.output}
            onRun={(commandId, input) => void run(commandId, input)}
          />
        )}

        {reply?.kind === 'answer' && reply.cited.length > 0 && (
          <p className="assistant-panel__cited">
            Based on {reply.cited.join(', ')}
          </p>
        )}

        {reply?.kind === 'choices' && (
          <div className="assistant-panel__choices">
            <p className="assistant-panel__reply">Which one?</p>
            {/*
              Keyed on the label as well as the id: near-miss contacts are
              the same action several times over, so the id alone is not
              unique and React would collapse them into one button.
            */}
            {reply.candidates.map((choice: Choice) => (
              <button
                className="nexus-btn nexus-btn--secondary"
                type="button"
                key={`${choice.commandId}:${choice.label}`}
                onClick={() => void run(choice.commandId, choice.input)}
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
            // The same two paths the spoken "yes" and "no" take, so a click
            // and a word cannot drift apart.
            onApprove={approve}
            onCancel={decline}
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
