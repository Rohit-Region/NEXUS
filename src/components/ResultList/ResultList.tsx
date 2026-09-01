import { ExternalLink } from 'lucide-react';
import './ResultList.css';

/**
 * Renders what an action actually found.
 *
 * The connector's spoken description is a summary on purpose: reading forty
 * browser tabs aloud is useless. But the screen has room, and discarding the
 * structured payload meant "list my tabs" answered with "7 tabs open: A, B,
 * C, and 4 more" and no way to see the other four.
 *
 * So speech stays short and this shows everything. The shapes below are the
 * payloads connectors actually return; anything unrecognised falls back to
 * the summary alone, which is why an unknown action degrades quietly rather
 * than rendering raw JSON at someone.
 */

interface Row {
  key: string;
  primary: string;
  secondary?: string;
  /** Set when the row can be acted on, e.g. switching to a tab. */
  action?: { commandId: string; input: unknown };
}

interface ResultListProps {
  actionId: string;
  output: unknown;
  onRun: (commandId: string, input?: unknown) => void;
}

/** Host only, so a long URL does not push the title off the row. */
function hostOf(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./, '');
  } catch {
    return url.slice(0, 40);
  }
}

function asArray(output: unknown, key: string): Record<string, unknown>[] {
  if (typeof output !== 'object' || output === null) return [];
  const value = (output as Record<string, unknown>)[key];
  return Array.isArray(value) ? (value as Record<string, unknown>[]) : [];
}

/** A payload field that is an array of plain strings. */
function strings(output: unknown, key: string): string[] {
  if (typeof output !== 'object' || output === null) return [];
  const value = (output as Record<string, unknown>)[key];
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === 'string')
    : [];
}

function str(row: Record<string, unknown>, key: string): string | undefined {
  const value = row[key];
  return typeof value === 'string' && value.trim().length > 0 ? value : undefined;
}

/** Rows for the shapes connectors return. Unknown actions get none. */
function rowsFor(actionId: string, output: unknown): Row[] {
  switch (actionId) {
    case 'browser.list_tabs':
      return asArray(output, 'tabs').map((tab, index) => {
        const title = str(tab, 'title') ?? '(untitled)';
        const url = str(tab, 'url') ?? '';
        return {
          key: `tab-${tab.windowIndex}-${tab.tabIndex}-${index}`,
          primary: title,
          secondary: url ? hostOf(url) : undefined,
          // Switching by title rather than index: the index shifts the
          // moment a tab is opened or closed.
          action: { commandId: 'browser.focus_tab', input: { query: title } },
        };
      });

    case 'system.list_apps':
      // A plain array of names rather than objects, so asArray does not fit.
      return strings(output, 'apps').map((name) => ({
        key: `app-${name}`,
        primary: name,
        action: { commandId: 'system.open_app', input: { name } },
      }));

    case 'ide.discover':
      return asArray(output, 'found').map((entry) => ({
        key: `ide-${str(entry, 'name') ?? Math.random()}`,
        primary: str(entry, 'name') ?? '(unnamed)',
        secondary: entry.registered ? 'registered' : 'not registered yet',
      }));

    case 'ide.list':
      return asArray(output, 'ides').map((entry) => ({
        key: `reg-${entry.id}`,
        primary: str(entry, 'name') ?? '(unnamed)',
        secondary: str(entry, 'executablePath'),
      }));

    case 'outlook.unread_mail':
      return asArray(output, 'messages').map((mail, index) => ({
        key: `mail-${index}`,
        primary: str(mail, 'subject') ?? '(no subject)',
        secondary: str(mail, 'from'),
      }));

    case 'outlook.today_schedule':
      return asArray(output, 'events').map((event, index) => ({
        key: `event-${index}`,
        primary: str(event, 'subject') ?? '(no subject)',
        secondary: str(event, 'start'),
      }));

    case 'github.list_prs':
      return asArray(output, 'pullRequests').map((pr) => ({
        key: `pr-${pr.number}`,
        primary: `#${pr.number} ${str(pr, 'title') ?? ''}`.trim(),
        secondary: str(pr, 'state')?.toLowerCase(),
      }));

    default:
      return [];
  }
}

export function ResultList({ actionId, output, onRun }: ResultListProps) {
  const rows = rowsFor(actionId, output);
  if (rows.length === 0) return null;

  return (
    <ul className="result-list" aria-label="Results">
      {rows.map((row) => {
        const act = row.action;
        return (
          <li className="result-list__row" key={row.key}>
            {act ? (
              <button
                className="result-list__button"
                type="button"
                onClick={() => onRun(act.commandId, act.input)}
                title={row.secondary}
              >
                <span className="result-list__primary">{row.primary}</span>
                {row.secondary && (
                  <span className="result-list__secondary">{row.secondary}</span>
                )}
                <ExternalLink size={11} strokeWidth={2} aria-hidden="true" />
              </button>
            ) : (
              <span className="result-list__static">
                <span className="result-list__primary">{row.primary}</span>
                {row.secondary && (
                  <span className="result-list__secondary">{row.secondary}</span>
                )}
              </span>
            )}
          </li>
        );
      })}
    </ul>
  );
}
