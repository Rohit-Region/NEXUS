/**
 * Typed wrappers around Tauri invoke() for all NEXUS DB commands.
 * Components import from this module — never from @tauri-apps/api directly.
 * Raw SQL never passes through this boundary.
 */
import { invoke } from '@tauri-apps/api/core';
import type {
  AssignTaskAgentInput,
  CreateProjectInput,
  CreateRegistryEntryInput,
  CreateTaskInput,
  DbCounts,
  DbStatus,
  Project,
  RegistryEntry,
  Task,
  UpdateProjectInput,
  UpdateRegistryEntryInput,
  UpdateTaskInput,
  UpdateTaskStatusInput,
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

export function deleteProject(id: number): Promise<void> {
  return invoke<void>('nexus_delete_project', { id });
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

export function deleteTask(id: number): Promise<void> {
  return invoke<void>('nexus_delete_task', { id });
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
