import { useCallback, useState } from 'react';
import { Logo } from '../Logo/Logo';
import { StatusBar } from '../StatusBar/StatusBar';
import { Dashboard } from '../Dashboard/Dashboard';
import { CommandBar } from '../CommandBar/CommandBar';
import type { NexusView } from '../../types';
import './AppShell.css';

/**
 * Thin orchestrator: owns view state only.
 * All project data and database access live in the project components.
 */
export function AppShell() {
  const [view, setView] = useState<NexusView>({ screen: 'projects' });
  const [activeProjectName, setActiveProjectName] = useState<string | null>(null);

  const navigate = useCallback((next: NexusView) => {
    if (next.screen !== 'project-detail') {
      setActiveProjectName(null);
    }
    setView(next);
  }, []);

  const handleActiveProjectChange = useCallback((name: string | null) => {
    setActiveProjectName(name);
  }, []);

  const onProjects =
    view.screen === 'projects' || view.screen === 'project-detail';
  const showBadge = view.screen === 'project-detail' && activeProjectName !== null;

  return (
    <div className="app-shell">
      <header className="app-shell__header">
        <Logo />

        <nav className="app-shell__nav" aria-label="Primary">
          <button
            className="app-shell__nav-btn"
            type="button"
            onClick={() => navigate({ screen: 'projects' })}
            aria-current={onProjects ? 'page' : undefined}
          >
            Projects
          </button>
          <button
            className="app-shell__nav-btn"
            type="button"
            onClick={() => navigate({ screen: 'registry' })}
            aria-current={view.screen === 'registry' ? 'page' : undefined}
          >
            Registry
          </button>
        </nav>

        {showBadge && (
          <span className="app-shell__project-badge" title={activeProjectName ?? undefined}>
            <span className="app-shell__project-badge-label">Project</span>
            {activeProjectName}
          </span>
        )}

        <StatusBar />
      </header>

      <Dashboard
        view={view}
        navigate={navigate}
        onActiveProjectChange={handleActiveProjectChange}
      />

      <CommandBar />
    </div>
  );
}
