import type {
  ProjectSortMode,
  RegistrySortMode,
  TaskSortMode,
} from './index';

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

// NEXUS-006: workspace aggregates. Every value is computed in SQL; nothing
// here is derived client-side from a partial dataset.
export interface WorkspaceSummary {
  projects: number;
  tasks: number;
  tasksOpen: number;
  tasksInProgress: number;
  tasksBlocked: number;
  tasksDone: number;
  tasksUnassigned: number;
  idesTotal: number;
  idesEnabled: number;
  agentsTotal: number;
  agentsEnabled: number;
}

export interface ProjectTaskCounts {
  projectId: number;
  total: number;
  open: number;
  inProgress: number;
  blocked: number;
  done: number;
}

export interface AgentTaskCounts {
  agentId: number;
  taskCount: number;
}

/** Nests Task rather than flattening it, reusing the NEXUS-004 contract. */
export interface TaskWithProject {
  task: Task;
  projectName: string;
}

// NEXUS-008: preferences. A typed struct crosses the boundary; the frontend
// never sees a key, a raw value, or a parse decision.
export interface Settings {
  launchScreen: 'overview' | 'projects';
  projectSort: ProjectSortMode;
  taskSort: TaskSortMode;
  registrySort: RegistrySortMode;
  taskStatusFilter: TaskStatus[];
  newProjectDefaultIdeId: number | null;
  newProjectDefaultAgentId: number | null;
  /** NEXUS-010. Off by default; the microphone never starts while false. */
  voiceEnabled: boolean;
  /**
   * NEXUS-011. Voice name or system identifier for spoken responses. An empty
   * string means the system default voice. Availability is machine-specific,
   * so this is a preference resolved with a fallback chain at speak time, not
   * a guarantee.
   */
  voiceName: string;
  /**
   * Keep the microphone open for the wake word, rather than opening it only
   * when asked. Off by default: it changes when the microphone is live, so
   * it is a choice made once and remembered.
   */
  alwaysListening: boolean;
  /** What NEXUS says back when called by name. Rotated in order. */
  wakeReplies: string[];
}

// NEXUS-009: unified cross-entity search, deferred from NEXUS-007.
export type SearchResultKind = 'project' | 'task' | 'ide' | 'agent';

export interface SearchResult {
  kind: SearchResultKind;
  id: number;
  title: string;
  subtitle: string | null;
  projectId: number | null;
}

export interface SearchResults {
  results: SearchResult[];
  /** True when the result cap was reached; the UI must say so. */
  truncated: boolean;
}

// NEXUS-010: on-device voice. The recognizer produces a transcript and
// nothing else; matching, confirmation and execution stay in NEXUS-009.
export type VoiceAuthorization =
  | 'notDetermined'
  | 'denied'
  | 'restricted'
  | 'authorized'
  | 'unknown';

export interface VoiceStatus {
  recognizerAvailable: boolean;
  /** False means NEXUS refuses to listen rather than using remote recognition. */
  supportsOnDevice: boolean;
  authorization: VoiceAuthorization;
  listening: boolean;
  locale: string;
}

export interface VoiceTranscript {
  text: string;
  isFinal: boolean;
  /** Milliseconds from session start to this result (S-07). */
  elapsedMs: number;
  /** Always true: the request is configured on-device only. */
  onDevice: boolean;
}

export interface VoiceState {
  listening: boolean;
  /** 'user' | 'silence' | 'timeout' | 'final' | 'error' */
  reason: string;
}

/** Passed to the voice matcher. Mirrors PaletteCommand's matchable fields. */
export interface VoiceCommandSpec {
  id: string;
  label: string;
  keywords: string[];
}

/** Candidates only. Nothing here is executed without confirmation. */
export interface VoiceIntent {
  commandIds: string[];
  searchQuery: string;
  normalized: string;
  projectName: string | null;
  ambiguous: boolean;
}

// NEXUS-011: spoken responses.

/**
 * What happened, as reported to the response templates.
 *
 * A discriminated union with no transcript member, by design: the wording is
 * chosen in Rust from the executed command id, and recognised speech has no
 * route to the synthesizer. `projectName` is read from the database row that
 * was opened, never from what was heard.
 */
export type VoiceOutcome =
  | { kind: 'executed'; commandId: string; projectName: string | null }
  | { kind: 'openedProject'; projectName: string }
  | { kind: 'noMatch' }
  | { kind: 'failed' }
  | { kind: 'cancelled' };

/** One voice offered in Settings. The list differs per machine. */
export interface VoiceOption {
  id: string;
  name: string;
  language: string;
  quality: 'default' | 'enhanced' | 'premium';
  /**
   * As the system reports it, never inferred from the name. Apple leaves the
   * Eloquence voices unspecified, and guessing would put a made-up label in
   * front of the user.
   */
  gender: 'male' | 'female' | 'unspecified';
  /** True for en-IN, the locale NEXUS recognises in. */
  preferredLocale: boolean;
}

/**
 * Someone NEXUS can message by name.
 *
 * Typed by the user and never synced: NEXUS does not read the macOS address
 * book, so the only people it knows are the ones deliberately entered.
 */
export interface Contact {
  id: number;
  name: string;
  /** International format, digits only, as stored. */
  phone: string;
  createdAt: string;
  updatedAt: string;
}

export interface ContactInput {
  /** Absent when creating. */
  id: number | null;
  name: string;
  phone: string;
}

/** What a transcript heard in always-listening mode amounts to. */
export interface WakeOutcome {
  /**
   * False for everything not addressed to NEXUS, which is most of what a
   * permanently open microphone hears. The caller must then do nothing.
   */
  woke: boolean;
  /** A command spoken in the same breath as the wake word. */
  command: string | null;
  /** The acknowledgement to speak, when the wake word came alone. */
  reply: string | null;
}

export interface VoiceSpeech {
  /** False means deliberately suppressed, which is not an error. */
  spoken: boolean;
  /** Template output. Never a transcript, and never persisted. */
  text: string;
  /** The voice actually used after fallback; null means the system default. */
  voice: string | null;
}
