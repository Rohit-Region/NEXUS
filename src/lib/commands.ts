/**
 * The command registry: the single source of truth for every command the
 * Command Palette can run.
 *
 * Deliberately free of React and of any database import, mirroring
 * list-filters.ts. Every function is pure and deterministic.
 *
 * Two rules hold this together:
 *
 * 1. **The registry owns definitions; screens own behaviour.** A command's
 *    `run` calls the existing `navigate` and nothing else. No business logic
 *    moves here, and no command performs a write.
 * 2. **Parameterised commands are expanded, not parsed** (spec 009 2.3).
 *    "New task in X" is one entry per project, generated from the project
 *    list. There is no tokeniser, no argument parsing, and no partial-input
 *    state machine. A future voice transcript is just a query string matched
 *    the same way, with no separate grammar to teach.
 */
import { matchesQuery } from './list-filters';
import type { NexusView, PaletteCommand } from '../types';
import type { Project } from '../types/db';

/** Navigation commands. Always available on every screen. */
export function navigationCommands(): PaletteCommand[] {
  const screens: {
    id: string;
    label: string;
    view: NexusView;
    keywords: string[];
  }[] = [
    {
      id: 'nav-overview',
      label: 'Go to Overview',
      view: { screen: 'overview' },
      keywords: ['overview', 'home', 'dashboard', 'summary', 'stats'],
    },
    {
      id: 'nav-projects',
      label: 'Go to Projects',
      view: { screen: 'projects' },
      keywords: ['projects', 'list', 'workspace'],
    },
    {
      id: 'nav-registry',
      label: 'Go to Registry',
      view: { screen: 'registry' },
      keywords: ['registry', 'ide', 'ides', 'agent', 'agents', 'tools'],
    },
    {
      id: 'nav-settings',
      label: 'Go to Settings',
      view: { screen: 'settings' },
      keywords: ['settings', 'preferences', 'options', 'config'],
    },
  ];

  return screens.map(({ id, label, view, keywords }) => ({
    id,
    label,
    keywords,
    group: 'Navigate' as const,
    run: (navigate) => navigate(view),
  }));
}

/**
 * Create commands.
 *
 * Task creation is expanded to one entry per project, so typing part of a
 * project name narrows to it naturally. With no projects, only the
 * project-creation command is offered: creating a task requires somewhere to
 * put it (spec 009 F-11).
 */
export function createCommands(projects: Project[]): PaletteCommand[] {
  const commands: PaletteCommand[] = [
    {
      id: 'create-project',
      label: 'New Project',
      description: 'Open the project list with the create form ready',
      keywords: ['new', 'create', 'add', 'project'],
      group: 'Create',
      run: (navigate) =>
        navigate({ screen: 'projects', intent: 'create-project' }),
    },
  ];

  for (const project of projects) {
    commands.push({
      id: `create-task-${project.id}`,
      label: `New Task in ${project.name}`,
      description: 'Open the project with the task form ready',
      keywords: ['new', 'create', 'add', 'task', project.name],
      group: 'Create',
      run: (navigate) =>
        navigate({
          screen: 'project-detail',
          projectId: project.id,
          intent: 'create-task',
        }),
    });
  }

  return commands;
}

/** Every command available for the given project list, in display order. */
export function allCommands(projects: Project[]): PaletteCommand[] {
  return [...navigationCommands(), ...createCommands(projects)];
}

/**
 * Filter by label, description and keywords.
 *
 * Reuses matchesQuery from list-filters so the established substring rule has
 * exactly one definition. An empty query returns everything, because
 * matchesQuery treats an empty normalized query as matching.
 */
export function filterCommands(
  commands: PaletteCommand[],
  normalized: string,
): PaletteCommand[] {
  return commands.filter((command) =>
    matchesQuery(normalized, [
      command.label,
      command.description,
      ...command.keywords,
    ]),
  );
}
