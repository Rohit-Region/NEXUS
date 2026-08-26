import type { TaskStatus } from './db';

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

// NEXUS-003 navigation. View state only; no routing library.
export type NexusScreen = 'projects' | 'project-detail' | 'registry';

export interface NexusView {
  screen: NexusScreen;
  projectId?: number; // set when screen === 'project-detail'
}

// Shared shape for the reusable ProjectForm
export interface ProjectFormValues {
  name: string;
  description: string;
  repositoryPath: string;
  repositoryUrl: string;
  defaultIdeId: number | null;
  defaultAgentId: number | null;
}

export type ProjectFormMode = 'create' | 'edit';

// NEXUS-004: shared shape for the reusable TaskForm
export interface TaskFormValues {
  title: string;
  description: string;
  status: TaskStatus;
}

export type TaskFormMode = 'create' | 'edit';

// NEXUS-005: shared shape for the reusable RegistryForm
export interface RegistryFormValues {
  name: string;
  entryType: string;
  executablePath: string;
  enabled: boolean;
}

export type RegistryFormMode = 'create' | 'edit';
