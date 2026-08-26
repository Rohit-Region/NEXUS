import type { PanelProps } from '../../types';
import { DbPanel } from '../DbPanel/DbPanel';
import './Dashboard.css';

type DashboardProps = PanelProps;

export function Dashboard({ className }: DashboardProps) {
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
        {/* Welcome content */}
        <div className="dashboard__content">
          <h1 className="dashboard__title">
            NE<span className="accent">X</span>US Command Center
          </h1>
          <div className="dashboard__divider" aria-hidden="true" />
          <p className="dashboard__subtitle">
            Developer Command Center — v0.1.0
          </p>
        </div>

        {/* NEXUS-002: persistence verification panel */}
        <DbPanel />
      </div>
    </main>
  );
}
