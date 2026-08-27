import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { CornerDownLeft, Search } from 'lucide-react';
import { listProjects, searchWorkspace, voiceSpeak } from '../../lib/nexus-db';
import { allCommands, filterCommands } from '../../lib/commands';
import { normalizeQuery } from '../../lib/list-filters';
import type { NexusView, PaletteCommand } from '../../types';
import {
  describeActionError,
  isActionError,
  runAction,
} from '../../lib/assistant';
import {
  actionForCommand,
  actionForResult,
  viewFromOutput,
} from '../../lib/palette-actions';
import type { Project, SearchResult, VoiceOutcome } from '../../types/db';
import './CommandPalette.css';

interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  navigate: (view: NexusView) => void;
  /**
   * NEXUS-010: a final voice transcript. It seeds the query so the user can
   * see what was heard and pick a candidate. It is NEVER auto-executed
   * (V-05, C-08): confirmation is the same Enter/click as the keyboard path.
   */
  initialQuery?: string | null;
  /**
   * NEXUS-010 D-3: command ids resolved by the deterministic voice matcher,
   * best first. When present the palette shows exactly these instead of
   * running the NEXUS-009 substring filter, whose whole-phrase rule cannot
   * match spoken sentences. Typing clears it and restores NEXUS-009 matching
   * unchanged, so the keyboard contract is untouched.
   */
  voiceCommandIds?: string[] | null;
  /** What the recognizer heard, shown above the input for confirmation. */
  voiceHeard?: string | null;
}

/** Commands and results share one flat selectable list so arrow keys cross
 *  the group boundary without special-casing. */
type Entry =
  | { type: 'command'; key: string; command: PaletteCommand }
  | { type: 'result'; key: string; result: SearchResult };

const KIND_LABEL: Record<SearchResult['kind'], string> = {
  project: 'Project',
  task: 'Task',
  ide: 'IDE',
  agent: 'Agent',
};

export function CommandPalette({
  open,
  onClose,
  navigate,
  initialQuery,
  voiceCommandIds,
  voiceHeard,
}: CommandPaletteProps) {
  const [query, setQuery] = useState('');
  const [projects, setProjects] = useState<Project[]>([]);
  const [results, setResults] = useState<SearchResult[]>([]);
  const [truncated, setTruncated] = useState(false);
  const [selected, setSelected] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Cleared the moment the user types, handing control back to NEXUS-009.
  const [useVoiceCandidates, setUseVoiceCandidates] = useState(false);

  // NEXUS-011: at most one spoken response per voice session. The first
  // terminal outcome claims it, so a no-match followed by Escape does not
  // produce two sentences.
  const announcedRef = useRef(false);
  // Read inside the async search callback, where the render's `commands` would
  // be stale. Only its length matters.
  const commandCountRef = useRef(0);

  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  // Focus is restored to whatever had it when the palette opened (F-03).
  const restoreFocusRef = useRef<HTMLElement | null>(null);

  // ── Open and close lifecycle ─────────────────────────────────────────────

  useEffect(() => {
    if (!open) return;

    restoreFocusRef.current = document.activeElement as HTMLElement | null;
    setQuery(initialQuery ?? '');
    setUseVoiceCandidates(
      Array.isArray(voiceCommandIds) && voiceCommandIds.length > 0,
    );
    setResults([]);
    setTruncated(false);
    setSelected(0);
    setError(null);
    announcedRef.current = false;
    inputRef.current?.focus();

    // Refetched on every open, so a project created since last time is present.
    void (async () => {
      try {
        setProjects(await listProjects());
      } catch (err) {
        setError(String(err));
      }
    })();

    return () => {
      const target = restoreFocusRef.current;
      if (target && document.contains(target)) target.focus();
    };
  }, [open, initialQuery, voiceCommandIds]);

  // ── Spoken responses (NEXUS-011) ─────────────────────────────────────────

  /**
   * Speak the response for an outcome, if this palette session began with
   * speech. Keyboard sessions stay silent: pressing Command-K and Escape
   * should not make the machine talk.
   *
   * Failing to speak never fails the action. Navigation has already happened
   * by the time this is called, and a missing voice is not worth an error
   * banner over.
   */
  const announce = useCallback(
    (outcome: VoiceOutcome) => {
      if (!voiceHeard || announcedRef.current) return;
      announcedRef.current = true;
      void voiceSpeak(outcome).catch(() => {
        /* Silence is the correct degradation for a failed announcement. */
      });
    },
    [voiceHeard],
  );

  // ── Search ───────────────────────────────────────────────────────────────

  useEffect(() => {
    if (!open) return;
    const trimmed = query.trim();
    if (trimmed.length === 0) {
      setResults([]);
      setTruncated(false);
      setLoading(false);
      return;
    }

    let cancelled = false;
    setLoading(true);
    void (async () => {
      try {
        const found = await searchWorkspace(trimmed);
        if (cancelled) return;
        setResults(found.results);
        setTruncated(found.truncated);
        setError(null);
        // Announced here rather than from a render effect: this is the first
        // moment both commands and results are known, so it cannot fire
        // against a half-populated list.
        if (found.results.length === 0 && commandCountRef.current === 0) {
          announce({ kind: 'noMatch' });
        }
      } catch (err) {
        if (!cancelled) {
          setError(String(err));
          announce({ kind: 'failed' });
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [query, open, announce]);

  // ── Entries ──────────────────────────────────────────────────────────────

  const normalized = normalizeQuery(query);

  const commands = useMemo(() => {
    const registry = allCommands(projects);
    if (useVoiceCandidates && voiceCommandIds) {
      // Preserve the matcher's ranking, and silently skip any id that no
      // longer exists (a project deleted between resolve and render).
      return voiceCommandIds
        .map((id) => registry.find((c) => c.id === id))
        .filter((c): c is (typeof registry)[number] => c !== undefined);
    }
    return filterCommands(registry, normalized);
  }, [projects, normalized, useVoiceCandidates, voiceCommandIds]);

  commandCountRef.current = commands.length;

  const entries: Entry[] = useMemo(() => {
    const out: Entry[] = commands.map((command) => ({
      type: 'command',
      key: `c:${command.id}`,
      command,
    }));
    for (const result of results) {
      out.push({ type: 'result', key: `r:${result.kind}:${result.id}`, result });
    }
    return out;
  }, [commands, results]);

  // A changed query must never leave the highlight on a stale row.
  useEffect(() => {
    setSelected(0);
  }, [query]);

  useEffect(() => {
    const node = listRef.current?.querySelector('[aria-selected="true"]');
    node?.scrollIntoView({ block: 'nearest' });
  }, [selected, entries.length]);

  // ── Actions ──────────────────────────────────────────────────────────────

  /**
   * Describe what is about to happen, in terms the response templates accept.
   *
   * Only the command id and, where the command is project-scoped, a project
   * name read from the loaded project list. Nothing derived from speech.
   */
  const outcomeFor = useCallback(
    (entry: Entry): VoiceOutcome => {
      if (entry.type === 'command') {
        const project = projects.find(
          (p) => `create-task-${p.id}` === entry.command.id,
        );
        return {
          kind: 'executed',
          commandId: entry.command.id,
          projectName: project ? project.name : null,
        };
      }
      if (entry.result.kind === 'project') {
        return { kind: 'openedProject', projectName: entry.result.title };
      }
      // Tasks, IDEs and agents navigate to a screen rather than to something
      // with a name worth speaking, so they get the generic acknowledgement.
      return { kind: 'executed', commandId: '', projectName: null };
    },
    [projects],
  );

  /**
   * NEXUS-012: everything the palette runs goes through the action gate.
   *
   * The palette no longer navigates directly. It asks the backend to perform
   * an action; the backend checks the grant, writes the audit row, and
   * returns a directive saying where to go. That indirection is the point:
   * the keyboard path and the voice path now share one enforcement point, so
   * "voice cannot bypass permissions" is a fact about the code rather than a
   * promise about the UI.
   */
  const runEntry = useCallback(
    (entry: Entry) => {
      const request =
        entry.type === 'command'
          ? actionForCommand(entry.command.id)
          : actionForResult(entry.result);

      if (!request) {
        // An id the bridge does not recognise. Surfaced rather than guessed:
        // running a nearby action would be worse than running none.
        setError(`NEXUS does not know how to run "${entry.key}".`);
        return;
      }

      void (async () => {
        try {
          const outcome = await runAction(request);
          const view = viewFromOutput(outcome.output);
          if (!view) {
            setError('NEXUS returned a destination it does not have.');
            return;
          }
          navigate(view);
          // After the navigation, never before: NEXUS reports what it did
          // rather than what it is about to try.
          announce(outcomeFor(entry));
          onClose();
        } catch (err) {
          // Refusals land here, and the palette stays open so the user can
          // read why and do something else.
          setError(isActionError(err) ? describeActionError(err) : String(err));
        }
      })();
    },
    [navigate, onClose, announce, outcomeFor],
  );

  /**
   * Dismissal. A voice session that ends without running anything is still an
   * outcome the user is owed, unless something was already said.
   */
  const dismiss = useCallback(() => {
    announce({ kind: 'cancelled' });
    onClose();
  }, [announce, onClose]);

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      dismiss();
      return;
    }
    if (entries.length === 0) return;

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setSelected((prev) => (prev + 1) % entries.length);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setSelected((prev) => (prev - 1 + entries.length) % entries.length);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const entry = entries[selected];
      if (entry) runEntry(entry);
    } else if (e.key === 'Tab') {
      // Simple focus trap: the palette holds a single focusable input.
      e.preventDefault();
    }
  }

  if (!open) return null;

  const commandCount = commands.length;
  const showResultsGroup = normalized.length > 0;

  return (
    <div
      className="command-palette__scrim"
      onMouseDown={(e) => {
        if (!panelRef.current?.contains(e.target as Node)) dismiss();
      }}
    >
      <div
        className="command-palette"
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        onKeyDown={handleKeyDown}
      >
        {voiceHeard && (
          <div className="command-palette__heard" role="status">
            <span className="command-palette__heard-label">Heard</span>
            <span className="command-palette__heard-text">
              &quot;{voiceHeard}&quot;
            </span>
            <span className="command-palette__heard-hint">
              Nothing runs until you confirm
            </span>
          </div>
        )}

        <div className="command-palette__input-row">
          <Search size={14} strokeWidth={2} aria-hidden="true" />
          <input
            ref={inputRef}
            className="command-palette__input"
            type="text"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setUseVoiceCandidates(false);
            }}
            placeholder="Run a command or search the workspace"
            aria-label="Command or search query"
            role="combobox"
            aria-expanded="true"
            aria-controls="command-palette-list"
            aria-activedescendant={entries[selected]?.key}
            autoComplete="off"
            spellCheck={false}
          />
          <kbd className="command-palette__hint">esc</kbd>
        </div>

        {error && (
          <p className="command-palette__error" role="alert">
            {error}
          </p>
        )}

        <div
          className="command-palette__list"
          id="command-palette-list"
          role="listbox"
          aria-label="Commands and results"
          ref={listRef}
        >
          {commandCount > 0 && (
            <div className="command-palette__group-label">Commands</div>
          )}
          {entries.map((entry, index) =>
            entry.type === 'command' ? (
              <div
                key={entry.key}
                id={entry.key}
                role="option"
                aria-selected={index === selected}
                className={`command-palette__row${
                  index === selected ? ' command-palette__row--selected' : ''
                }`}
                onMouseEnter={() => setSelected(index)}
                onClick={() => runEntry(entry)}
              >
                <span className="command-palette__row-main">
                  <span className="command-palette__row-label">
                    {entry.command.label}
                  </span>
                  {entry.command.description && (
                    <span className="command-palette__row-desc">
                      {entry.command.description}
                    </span>
                  )}
                </span>
                {index === selected && (
                  <CornerDownLeft size={12} strokeWidth={2} aria-hidden="true" />
                )}
              </div>
            ) : (
              <div key={entry.key}>
                {index === commandCount && showResultsGroup && (
                  <div className="command-palette__group-label">Results</div>
                )}
                <div
                  id={entry.key}
                  role="option"
                  aria-selected={index === selected}
                  className={`command-palette__row${
                    index === selected ? ' command-palette__row--selected' : ''
                  }`}
                  onMouseEnter={() => setSelected(index)}
                  onClick={() => runEntry(entry)}
                >
                  <span className="nexus-chip">
                    {KIND_LABEL[entry.result.kind]}
                  </span>
                  <span className="command-palette__row-main">
                    <span className="command-palette__row-label">
                      {entry.result.title}
                    </span>
                    {entry.result.subtitle && (
                      <span className="command-palette__row-desc">
                        {entry.result.subtitle}
                      </span>
                    )}
                  </span>
                  {index === selected && (
                    <CornerDownLeft size={12} strokeWidth={2} aria-hidden="true" />
                  )}
                </div>
              </div>
            ),
          )}

          {loading && (
            <p className="command-palette__status">Searching...</p>
          )}

          {!loading && entries.length === 0 && (
            <p className="command-palette__status">
              Nothing matches &quot;{query.trim()}&quot;.
            </p>
          )}

          {truncated && (
            <p className="command-palette__status">
              More matches exist than can be shown. Narrow the search.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
