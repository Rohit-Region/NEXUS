/**
 * Typed wrappers around Tauri invoke() for all NEXUS DB commands.
 * Components import from this module — never from @tauri-apps/api directly.
 * Raw SQL never passes through this boundary.
 */
import { invoke } from '@tauri-apps/api/core';
import type {
  CreateProjectInput,
  DbCounts,
  DbStatus,
  Project,
  UpdateProjectInput,
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
