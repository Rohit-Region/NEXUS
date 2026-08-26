// System status values — will expand in future milestones
export type SystemStatus = 'ONLINE' | 'OFFLINE' | 'DEGRADED';

// Base props shared by panel components
export interface PanelProps {
  className?: string;
}

// CommandBar state
export interface CommandState {
  value: string;
  isListening: boolean; // always false in NEXUS-001
}
