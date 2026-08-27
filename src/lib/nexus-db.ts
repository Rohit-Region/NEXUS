/**
 * Typed wrappers around Tauri invoke() for all NEXUS DB commands.
 * Components import from this module — never from @tauri-apps/api directly.
 * Raw SQL never passes through this boundary.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  AgentTaskCounts,
  AssignTaskAgentInput,
  CreateProjectInput,
  CreateRegistryEntryInput,
  CreateTaskInput,
  DbCounts,
  DbStatus,
  Project,
  ProjectTaskCounts,
  RegistryEntry,
  SearchResults,
  Settings,
  VoiceState,
  VoiceCommandSpec,
  VoiceIntent,
  VoiceOption,
  VoiceOutcome,
  VoiceSpeech,
  VoiceStatus,
  VoiceTranscript,
  Task,
  TaskWithProject,
  UpdateProjectInput,
  UpdateRegistryEntryInput,
  UpdateTaskInput,
  UpdateTaskStatusInput,
  WorkspaceSummary,
} from '../types/db';

export function getDbStatus(): Promise<DbStatus> {
  return invoke<DbStatus>('nexus_get_db_status');
}

export function getDbCounts(): Promise<DbCounts> {
  return invoke<DbCounts>('nexus_get_db_counts');
}

export function createProject(input: CreateProjectInput): Promise<Project> {
  return invoke<Project>('nexus_create_project', { input });
}

export function listProjects(): Promise<Project[]> {
  return invoke<Project[]>('nexus_list_projects');
}

export function updateProject(input: UpdateProjectInput): Promise<Project> {
  return invoke<Project>('nexus_update_project', { input });
}

export function createTask(input: CreateTaskInput): Promise<Task> {
  return invoke<Task>('nexus_create_task', { input });
}

export function listTasks(projectId: number): Promise<Task[]> {
  return invoke<Task[]>('nexus_list_tasks', { projectId });
}

export function updateTask(input: UpdateTaskInput): Promise<Task> {
  return invoke<Task>('nexus_update_task', { input });
}

export function updateTaskStatus(input: UpdateTaskStatusInput): Promise<Task> {
  return invoke<Task>('nexus_update_task_status', { input });
}

export function assignTaskAgent(input: AssignTaskAgentInput): Promise<Task> {
  return invoke<Task>('nexus_assign_task_agent', { input });
}

// ── Registry: IDEs ──────────────────────────────────────────────────────────

export function createIde(input: CreateRegistryEntryInput): Promise<RegistryEntry> {
  return invoke<RegistryEntry>('nexus_create_ide', { input });
}

export function listIdes(enabledOnly: boolean): Promise<RegistryEntry[]> {
  return invoke<RegistryEntry[]>('nexus_list_ides', { enabledOnly });
}

export function updateIde(input: UpdateRegistryEntryInput): Promise<RegistryEntry> {
  return invoke<RegistryEntry>('nexus_update_ide', { input });
}

export function deleteIde(id: number): Promise<void> {
  return invoke<void>('nexus_delete_ide', { id });
}

// ── Registry: AI agents ─────────────────────────────────────────────────────

export function createAgent(input: CreateRegistryEntryInput): Promise<RegistryEntry> {
  return invoke<RegistryEntry>('nexus_create_agent', { input });
}

export function listAgents(enabledOnly: boolean): Promise<RegistryEntry[]> {
  return invoke<RegistryEntry[]>('nexus_list_agents', { enabledOnly });
}

export function updateAgent(input: UpdateRegistryEntryInput): Promise<RegistryEntry> {
  return invoke<RegistryEntry>('nexus_update_agent', { input });
}

export function deleteAgent(id: number): Promise<void> {
  return invoke<void>('nexus_delete_agent', { id });
}

// ── Aggregates ──────────────────────────────────────────────────────────────

export function getWorkspaceSummary(): Promise<WorkspaceSummary> {
  return invoke<WorkspaceSummary>('nexus_get_workspace_summary');
}

export function countTasksByProject(): Promise<ProjectTaskCounts[]> {
  return invoke<ProjectTaskCounts[]>('nexus_count_tasks_by_project');
}

export function countTasksByAgent(): Promise<AgentTaskCounts[]> {
  return invoke<AgentTaskCounts[]>('nexus_count_tasks_by_agent');
}

export function listRecentTasks(limit: number): Promise<TaskWithProject[]> {
  return invoke<TaskWithProject[]>('nexus_list_recent_tasks', { limit });
}

// ── Settings ────────────────────────────────────────────────────────────────

export function getSettings(): Promise<Settings> {
  return invoke<Settings>('nexus_get_settings');
}

export function updateSettings(input: Settings): Promise<Settings> {
  return invoke<Settings>('nexus_update_settings', { input });
}

export function resetSettings(): Promise<Settings> {
  return invoke<Settings>('nexus_reset_settings');
}

// ── Search ──────────────────────────────────────────────────────────────────

export function searchWorkspace(query: string): Promise<SearchResults> {
  return invoke<SearchResults>('nexus_search_workspace', { query });
}

// Deletion is deliberately absent. Since NEXUS-012 it is an action, not a
// database call: it goes through the permission gate in src/lib/assistant.ts
// so it is confirmed and audited like anything else NEXUS does on your
// behalf. See nexus.delete_project and nexus.delete_task.

// ── Voice (NEXUS-010) ───────────────────────────────────────────────────────
//
// These only control the recognizer. Transcripts arrive as Tauri events, are
// matched by the NEXUS-009 registry, and require confirmation before anything
// runs. Nothing here executes a command.

export function voiceStatus(): Promise<VoiceStatus> {
  return invoke<VoiceStatus>('nexus_voice_status');
}

export function voiceRequestAuthorization(): Promise<void> {
  return invoke<void>('nexus_voice_request_authorization');
}

export function voiceStart(): Promise<void> {
  return invoke<void>('nexus_voice_start');
}

export function voiceStop(): Promise<void> {
  return invoke<void>('nexus_voice_stop');
}

// ── Spoken responses (NEXUS-011) ────────────────────────────────────────────
//
// The caller reports what happened; Rust picks the words from a deterministic
// template keyed on the executed command id. No transcript crosses this
// boundary, so nothing NEXUS heard can end up being spoken back.

/**
 * Speak the response for an outcome.
 *
 * Returns without speaking when voice is disabled in Settings or the
 * microphone is open. Both are ordinary results, reported as `spoken: false`
 * rather than as errors.
 */
export function voiceSpeak(outcome: VoiceOutcome): Promise<VoiceSpeech> {
  return invoke<VoiceSpeech>('nexus_voice_speak', { outcome });
}

/** Silence the synthesizer. Safe when nothing is being spoken. */
export function voiceStopSpeaking(): Promise<void> {
  return invoke<void>('nexus_voice_stop_speaking');
}

/** English voices installed on this machine, for the Settings picker. */
export function voiceListVoices(): Promise<VoiceOption[]> {
  return invoke<VoiceOption[]>('nexus_voice_list_voices');
}

/** Transcripts arrive as events. They are never persisted anywhere. */
export function onVoiceTranscript(
  cb: (t: VoiceTranscript) => void,
): Promise<UnlistenFn> {
  return listen<VoiceTranscript>('nexus://voice/transcript', (e) => cb(e.payload));
}

export function onVoiceState(cb: (s: VoiceState) => void): Promise<UnlistenFn> {
  return listen<VoiceState>('nexus://voice/state', (e) => cb(e.payload));
}

export function onVoiceError(cb: (message: string) => void): Promise<UnlistenFn> {
  return listen<string>('nexus://voice/error', (e) => cb(e.payload));
}

/**
 * Resolve a spoken transcript to candidate commands.
 *
 * The registry is sent from the frontend so `src/lib/commands.ts` stays the
 * single source of truth. Only used on the voice path; typing still goes
 * through the unchanged NEXUS-009 matcher.
 */
export function resolveVoiceIntent(
  transcript: string,
  commands: VoiceCommandSpec[],
  projectNames: string[],
): Promise<VoiceIntent> {
  return invoke<VoiceIntent>('nexus_resolve_voice_intent', {
    transcript,
    commands,
    projectNames,
  });
}
