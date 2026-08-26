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
}

export interface UpdateProjectInput {
  id: number;
  name: string;
  description?: string;
  repositoryPath?: string;
  repositoryUrl?: string;
}
