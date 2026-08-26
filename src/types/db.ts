// TypeScript types that mirror the Rust command payload structs.
// Field names match the serde(rename_all = "camelCase") output from Rust.

export interface DbStatus {
  initialized: boolean;
  migrationLevel: number;
  dbPath: string;
}

export interface DbCounts {
  projects: number;
  tasks: number;
  aiAgents: number;
  ides: number;
  settings: number;
}

export interface Project {
  id: number;
  name: string;
  description: string | null;
  repositoryPath: string | null;
  repositoryUrl: string | null;
  defaultIdeId: number | null;
  defaultAgentId: number | null;
  createdAt: string;
  updatedAt: string;
}

export interface CreateProjectInput {
  name: string;
  description?: string;
  repositoryPath?: string;
  repositoryUrl?: string;
  defaultIdeId?: number | null;
  defaultAgentId?: number | null;
}

export interface UpdateProjectInput {
  id: number;
  name: string;
  description?: string;
  repositoryPath?: string;
  repositoryUrl?: string;
  defaultIdeId?: number | null;
  defaultAgentId?: number | null;
}

// NEXUS-004: tasks. TaskStatus mirrors TASK_STATUSES in src-tauri/src/db/tasks.rs.
export type TaskStatus = 'open' | 'in_progress' | 'blocked' | 'done';

export interface Task {
  id: number;
  externalId: string | null;
  title: string;
  description: string | null;
  status: TaskStatus;
  projectId: number;
  assignedAgent: number | null;
  createdAt: string;
  updatedAt: string;
}

export interface CreateTaskInput {
  projectId: number;
  title: string;
  description?: string;
  status?: TaskStatus;
}

export interface UpdateTaskInput {
  id: number;
  title: string;
  description?: string;
  status: TaskStatus;
}

export interface UpdateTaskStatusInput {
  id: number;
  status: TaskStatus;
}

// NEXUS-005: registry. One shape serves both `ides` and `ai_agents`;
// `entryType` maps to ide_type / agent_type on the Rust side.
export interface RegistryEntry {
  id: number;
  name: string;
  entryType: string;
  executablePath: string | null;
  enabled: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface CreateRegistryEntryInput {
  name: string;
  entryType: string;
  executablePath?: string;
  enabled?: boolean;
}

export interface UpdateRegistryEntryInput {
  id: number;
  name: string;
  entryType: string;
  executablePath?: string;
  enabled: boolean;
}

export interface AssignTaskAgentInput {
  id: number;
  /** null clears the assignment. */
  agentId: number | null;
}
