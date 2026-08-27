/**
 * NEXUS-012: the bridge from palette entries to typed actions.
 *
 * The NEXUS-009 registry stays exactly as it was: its ids are the matching
 * vocabulary, and touching them would change what the keyboard and voice
 * matchers see. So the mapping lives here instead, as a pure function.
 *
 * Parameterised commands are expanded per project for *matching*
 * ("New Task in Atlas"), but an action is parameterised for *execution*
 * (`nexus.new_task` with a projectId). This function is where those two
 * models meet, and it is the only place that knows the `create-task-<id>`
 * id format.
 */
import type { ActionRequest } from '../types/assistant';
import type { NexusScreen, NexusView } from '../types';
import type { SearchResult } from '../types/db';

const CREATE_TASK_PREFIX = 'create-task-';

/** Fixed one-to-one mappings. */
const DIRECT: Record<string, string> = {
  'nav-overview': 'nexus.open_overview',
  'nav-projects': 'nexus.open_projects',
  'nav-registry': 'nexus.open_registry',
  'nav-settings': 'nexus.open_settings',
  'create-project': 'nexus.new_project',
};

/**
 * The action a palette command should run, or null if the id is unknown.
 *
 * Null is returned rather than a guess: a plausible wrong action is worse
 * than none, and the caller surfaces it as an error the user can see.
 */
export function actionForCommand(commandId: string): ActionRequest | null {
  const direct = DIRECT[commandId];
  if (direct) return { actionId: direct };

  if (commandId.startsWith(CREATE_TASK_PREFIX)) {
    const raw = commandId.slice(CREATE_TASK_PREFIX.length);
    const projectId = Number(raw);
    // A non-numeric suffix means the id did not come from this registry.
    if (!Number.isInteger(projectId) || raw.length === 0) return null;
    return { actionId: 'nexus.new_task', input: { projectId } };
  }

  return null;
}

/** The action that opens a search result. */
export function actionForResult(result: SearchResult): ActionRequest {
  if (
    (result.kind === 'project' || result.kind === 'task') &&
    result.projectId !== null
  ) {
    return {
      actionId: 'nexus.open_project',
      input: { projectId: result.projectId },
    };
  }
  // IDEs and agents live on one screen, so opening either means going there.
  return { actionId: 'nexus.open_registry' };
}

const SCREENS: NexusScreen[] = [
  'overview',
  'projects',
  'project-detail',
  'registry',
  'settings',
];

function isScreen(value: unknown): value is NexusScreen {
  return typeof value === 'string' && SCREENS.includes(value as NexusScreen);
}

/**
 * Turn a navigation action's output into a view.
 *
 * Validated rather than cast: the value crossed an IPC boundary, and a screen
 * name NEXUS does not have should fail loudly here rather than render a blank
 * shell.
 */
export function viewFromOutput(output: unknown): NexusView | null {
  if (typeof output !== 'object' || output === null) return null;
  const record = output as Record<string, unknown>;
  if (!isScreen(record.screen)) return null;

  const view: NexusView = { screen: record.screen };
  if (typeof record.projectId === 'number') {
    view.projectId = record.projectId;
  }
  if (record.intent === 'create-project' || record.intent === 'create-task') {
    view.intent = record.intent;
  }
  return view;
}
