import type { NexusView, PanelProps } from '../../types';
import { ProjectList } from '../ProjectList/ProjectList';
import { ProjectDetail } from '../ProjectDetail/ProjectDetail';
import { RegistryScreen } from '../RegistryScreen/RegistryScreen';
import './Dashboard.css';

interface DashboardProps extends PanelProps {
  view: NexusView;
  navigate: (view: NexusView) => void;
  onActiveProjectChange: (name: string | null) => void;
}

export function Dashboard({
  className,
  view,
  navigate,
  onActiveProjectChange,
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
        {view.screen === 'projects' && <ProjectList navigate={navigate} />}

        {view.screen === 'project-detail' && view.projectId !== undefined && (
          <ProjectDetail
            projectId={view.projectId}
            navigate={navigate}
            onActiveProjectChange={onActiveProjectChange}
          />
        )}

        {view.screen === 'registry' && <RegistryScreen />}
      </div>
    </main>
  );
}
