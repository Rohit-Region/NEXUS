import { useEffect, useState } from 'react';
import {
  Activity,
  CalendarDays,
  FolderKanban,
  Home,
  KeyRound,
  LayoutGrid,
  Lightbulb,
  ListTodo,
  Radar,
  Settings as SettingsIcon,
  Sparkles,
} from 'lucide-react';
import { listConnectors } from '../../lib/assistant';
import type { ConnectorStatus, ConnectorView } from '../../types/assistant';
import type { NexusView } from '../../types';
import './Sidebar.css';

interface Props {
  view: NexusView;
  navigate: (next: NexusView) => void;
  onOpenAssistant: () => void;
  suggestionCount: number;
}

const REFRESH_MS = 8000;
/** Connectors listed by name. The rest are behind "View all". */
const PINNED = 6;

/** How a connector's own status renders as a dot. */
function dotOf(status: ConnectorStatus): 'ok' | 'warn' | 'off' {
  if (status === 'ready') return 'ok';
  if (status === 'degraded' || status === 'needsAuth') return 'warn';
  return 'off';
}

/**
 * The permanent left rail.
 *
 * Grouped rather than flat, because the items answer different questions:
 * where am I, what am I working on, what does NEXUS think, what is it wired
 * to, and how is it configured. A flat list of eleven items makes the reader
 * do that sorting every time.
 *
 * The connector dots are the part that earns its place. They are the only
 * always-visible answer to "is Jira actually working", and they come from
 * each connector's own `status()`, refreshed on the same cadence as the
 * assistant's watcher.
 */
export function Sidebar({ view, navigate, onOpenAssistant, suggestionCount }: Props) {
  const [connectors, setConnectors] = useState<ConnectorView[]>([]);
  const [showAll, setShowAll] = useState(false);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const rows = await listConnectors();
        if (alive) setConnectors(rows);
      } catch {
        // The rail staying stale is better than the rail disappearing.
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  // NEXUS's own connector is not a "connection": it is the application.
  const linked = connectors.filter((c) => c.id !== 'nexus');
  const shown = showAll ? linked : linked.slice(0, PINNED);

  const item = (
    label: string,
    icon: React.ReactNode,
    active: boolean,
    onClick: () => void,
    badge?: number,
    hint?: string,
  ) => (
    <button
      key={label}
      className={`rail__item${active ? ' rail__item--on' : ''}`}
      type="button"
      onClick={onClick}
      aria-current={active ? 'page' : undefined}
    >
      <span className="rail__icon" aria-hidden="true">
        {icon}
      </span>
      <span className="rail__label">{label}</span>
      {badge !== undefined && badge > 0 && (
        <span className="rail__badge">{badge}</span>
      )}
      {hint && <span className="rail__hint">{hint}</span>}
    </button>
  );

  return (
    <aside className="rail" aria-label="Navigation">
      <div className="rail__brand">
        <span className="rail__mark" aria-hidden="true">
          &gt;_
        </span>
        <span className="rail__wordmark">
          NE<span className="rail__wordmark-accent">X</span>US
        </span>
      </div>

      <nav className="rail__scroll">
        <p className="rail__group">
          <Radar size={11} strokeWidth={2} aria-hidden="true" />
          Command Center
        </p>
        {item('Overview', <Home size={15} strokeWidth={1.8} />, view.screen === 'overview', () =>
          navigate({ screen: 'overview' }),
        )}
        {item('Activity', <Activity size={15} strokeWidth={1.8} />, false, () =>
          navigate({ screen: 'overview' }),
        )}
        {item('Calendar', <CalendarDays size={15} strokeWidth={1.8} />, false, () =>
          navigate({ screen: 'overview' }),
        )}

        <p className="rail__group">Work</p>
        {item(
          'Projects',
          <FolderKanban size={15} strokeWidth={1.8} />,
          view.screen === 'projects' || view.screen === 'project-detail',
          () => navigate({ screen: 'projects' }),
        )}
        {item('Tasks', <ListTodo size={15} strokeWidth={1.8} />, false, () =>
          navigate({ screen: 'projects' }),
        )}

        <p className="rail__group">Intelligence</p>
        {item(
          'Assistant',
          <Sparkles size={15} strokeWidth={1.8} />,
          false,
          onOpenAssistant,
          undefined,
          '⌘K',
        )}
        {item(
          'Suggestions',
          <Lightbulb size={15} strokeWidth={1.8} />,
          false,
          () => navigate({ screen: 'overview' }),
          suggestionCount,
        )}

        <p className="rail__group">Connections</p>
        <ul className="rail__links">
          {shown.map((c) => (
            <li className="rail__link" key={c.id}>
              <span className="rail__link-name">{c.displayName}</span>
              <span
                className={`rail__dot rail__dot--${dotOf(c.status)}`}
                title={`${c.displayName}: ${c.status}`}
              />
            </li>
          ))}
          {linked.length > PINNED && (
            <li>
              <button
                className="rail__more"
                type="button"
                onClick={() => setShowAll((v) => !v)}
              >
                {showAll ? 'Show fewer' : 'View all'}
              </button>
            </li>
          )}
        </ul>

        <p className="rail__group">System</p>
        {item('Registry', <LayoutGrid size={15} strokeWidth={1.8} />, view.screen === 'registry', () =>
          navigate({ screen: 'registry' }),
        )}
        {item('Permissions', <KeyRound size={15} strokeWidth={1.8} />, false, () =>
          navigate({ screen: 'settings' }),
        )}
        {item(
          'Settings',
          <SettingsIcon size={15} strokeWidth={1.8} />,
          view.screen === 'settings',
          () => navigate({ screen: 'settings' }),
        )}
      </nav>
    </aside>
  );
}
