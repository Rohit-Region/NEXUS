import type { Settings, TaskStatus } from './db';

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
export type NexusScreen =
  | 'overview'
  | 'projects'
  | 'project-detail'
  | 'registry'
  | 'settings';

/**
 * NEXUS-009: a one-shot instruction consumed by the destination screen on
 * mount. Consumed as a useState initializer, which makes it one-shot by
 * construction: it seeds mount state and is never read again.
 */
export type NexusIntent = 'create-project' | 'create-task';

export interface NexusView {
  screen: NexusScreen;
  projectId?: number; // set when screen === 'project-detail'
  intent?: NexusIntent;
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

// NEXUS-007: list controls. Sort modes are per entity because the sortable
// fields differ; one shared union would permit invalid combinations.
export type ProjectSortMode =
  | 'created-desc'
  | 'created-asc'
  | 'updated-desc'
  | 'name-asc'
  | 'name-desc';

export type TaskSortMode =
  | 'created-desc'
  | 'created-asc'
  | 'updated-desc'
  | 'title-asc'
  | 'status';

export type RegistrySortMode =
  | 'created-desc'
  | 'created-asc'
  | 'name-asc'
  | 'type-asc';

/** Registry enabled filter. 'all' is the default and means no filtering. */
export type EnabledFilter = 'all' | 'enabled' | 'disabled';

/** One entry in a sort control. */
export interface SortOption<T extends string> {
  value: T;
  label: string;
}

/**
 * NEXUS-008 startup fallback. Must stay identical to Settings::defaults() in
 * src-tauri/src/db/settings.rs, or the application behaves differently when
 * settings fail to load than when they load empty.
 */
export const DEFAULT_SETTINGS: Settings = {
  launchScreen: 'overview',
  projectSort: 'created-desc',
  taskSort: 'created-desc',
  registrySort: 'created-desc',
  taskStatusFilter: [],
  newProjectDefaultIdeId: null,
  newProjectDefaultAgentId: null,
  voiceEnabled: false,
  // Must match Settings::defaults() in src-tauri/src/db/settings.rs, which
  // takes it from voice::speech::DEFAULT_VOICE. Rishi rather than Tara:
  // AVSpeechSynthesizer does not expose Tara, whatever `say -v ?` reports.
  voiceName: 'Rishi',
};

/**
 * NEXUS-009: one entry in the command registry.
 *
 * The registry owns command definitions; actions delegate to the existing
 * navigation mechanism and never call a database command themselves.
 */
export interface PaletteCommand {
  id: string;
  label: string;
  /** Shown under the label where it adds context. */
  description?: string;
  /** Extra terms matched alongside the label. */
  keywords: string[];
  group: 'Navigate' | 'Create';
  run: (navigate: (view: NexusView) => void) => void;
}
