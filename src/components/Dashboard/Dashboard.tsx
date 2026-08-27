import type { NexusView, PanelProps } from '../../types';
import type { Settings } from '../../types/db';
import { OverviewScreen } from '../OverviewScreen/OverviewScreen';
import { ProjectList } from '../ProjectList/ProjectList';
import { ProjectDetail } from '../ProjectDetail/ProjectDetail';
import { RegistryScreen } from '../RegistryScreen/RegistryScreen';
import { SettingsScreen } from '../SettingsScreen/SettingsScreen';
import './Dashboard.css';

interface DashboardProps extends PanelProps {
  view: NexusView;
  navigate: (view: NexusView) => void;
  onActiveProjectChange: (name: string | null) => void;
  settings: Settings;
  onSettingsChange: (next: Settings) => void;
  /** Navigation counter; see AppShell. Only used to force an intent remount. */
  navSeq: number;
}

export function Dashboard({
  className,
  view,
  navigate,
  onActiveProjectChange,
  settings,
  onSettingsChange,
  navSeq,
}: DashboardProps) {
  return (
    <main className={`dashboard${className ? ` ${className}` : ''}`} role="main">
      {/* Background watermark */}
      <div className="dashboard__watermark" aria-hidden="true">
        <span className="dashboard__watermark-text">
          NE<span className="accent">X</span>US
        </span>
      </div>

      {/* Scrollable content column */}
      <div className="dashboard__scroll">
        {view.screen === 'overview' && <OverviewScreen navigate={navigate} />}

        {view.screen === 'projects' && (
          <ProjectList
            key={view.intent ? `projects-${navSeq}` : 'projects'}
            navigate={navigate}
            settings={settings}
            intent={view.intent}
          />
        )}

        {view.screen === 'project-detail' && view.projectId !== undefined && (
          <ProjectDetail
            key={
              view.intent
                ? `${view.projectId}-${navSeq}`
                : `${view.projectId}`
            }
            projectId={view.projectId}
            intent={view.intent}
            navigate={navigate}
            onActiveProjectChange={onActiveProjectChange}
            settings={settings}
          />
        )}

        {view.screen === 'registry' && <RegistryScreen settings={settings} />}

        {view.screen === 'settings' && (
          <SettingsScreen
            settings={settings}
            onSettingsChange={onSettingsChange}
          />
        )}
      </div>
    </main>
  );
}
