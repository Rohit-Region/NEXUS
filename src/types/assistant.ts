/**
 * NEXUS-012: the assistant action layer, as the UI sees it.
 *
 * Mirrors src-tauri/src/assistant/. Every type here is a view of something
 * Rust owns; none of it is authoritative. In particular, a grant shown as
 * off is a courtesy, not a control: the gate refuses regardless of what the
 * UI renders.
 */

export type Permission =
  | 'read'
  | 'interact'
  | 'write'
  | 'execute'
  | 'destructive';

export type ConfirmPolicy = 'never' | 'always';

export type Reach = 'localOnly' | 'leavesMachine';

export type ConnectorStatus =
  | 'ready'
  | 'degraded'
  | 'needsAuth'
  | 'disabled'
  | 'unavailable';

export interface ActionSpec {
  id: string;
  connectorId: string;
  summary: string;
  permission: Permission;
  confirm: ConfirmPolicy;
  reach: Reach;
  reversible: boolean;
}

export interface UnavailableAction {
  actionId: string;
  reason: string;
}

export interface Capabilities {
  available: string[];
  unavailable: UnavailableAction[];
}

export interface ConnectorView {
  id: string;
  displayName: string;
  status: ConnectorStatus;
  enabled: boolean;
  capabilities: Capabilities;
  actions: ActionSpec[];
  granted: Permission[];
  /** Levels this connector's actions actually use. */
  requiredLevels: Permission[];
  /**
   * Stored configuration: endpoints and account names. Safe to display, as
   * the setter refuses any key that looks like a secret. Null when the
   * connector has never been configured.
   */
  config: Record<string, string> | null;
}

export interface ActionRequest {
  actionId: string;
  input?: unknown;
  /** Supplied on the second call, once the user has approved. */
  approval?: number;
}

export interface ActionOutcome {
  actionId: string;
  output: unknown;
  summary: string;
  /** What the action found, described by the connector. */
  detail?: string;
  auditId: number;
}

/**
 * Everything that can go wrong, as data.
 *
 * `needsApproval` is deliberately an error rather than a success: nothing
 * ran. A caller that swallows errors must not mistake it for completion.
 */
export type ActionError =
  | { kind: 'unknownAction'; actionId: string }
  | { kind: 'connectorDisabled'; connectorId: string }
  | { kind: 'notPermitted'; connectorId: string; level: Permission }
  | {
      kind: 'needsApproval';
      token: number;
      summary: string;
      permission: Permission;
      reversible: boolean;
      expiresInMs: number;
    }
  | { kind: 'invalidApproval'; reason: string }
  | { kind: 'invalidInput'; detail: string }
  | { kind: 'failed'; detail: string };

export type NeedsApproval = Extract<ActionError, { kind: 'needsApproval' }>;

export interface AuditEntry {
  id: number;
  actionId: string;
  connectorId: string;
  permission: string;
  summary: string;
  outcome: 'attempted' | 'succeeded' | 'failed' | 'refused';
  error: string | null;
  durationMs: number | null;
  approved: boolean;
  createdAt: string;
}

/** What a navigation action returns: a directive, not a side effect. */
export interface NavigateOutput {
  screen: string;
  projectId?: number;
  intent?: string;
}

// ── NEXUS-013: assistant state, conversation and referents ──────────────────

/**
 * What NEXUS is doing.
 *
 * `listening` is derived on the Rust side from the microphone's own flag, not
 * stored separately, so this can never disagree with the voice indicator.
 */
export type AssistantState =
  | 'idle'
  | 'listening'
  | 'thinking'
  | 'awaitingConfirmation'
  | 'executing'
  | 'completed'
  | 'failed'
  | 'cancelled';

export type TurnInput =
  | { source: 'voice'; text: string }
  | { source: 'text'; text: string }
  | { source: 'ui'; actionId: string };

export interface Turn {
  id: number;
  input: TurnInput;
  state: AssistantState;
  summary: string | null;
  error: string | null;
}

export type ReferentKind =
  | 'project'
  | 'task'
  | 'pullRequest'
  | 'jiraIssue'
  | 'teamsMessage'
  | 'person'
  | 'conversation'
  | 'browserTab'
  | 'ideWorkspace'
  | 'suggestion';

/** One thing NEXUS mentioned, and how to act on it later. */
export interface Referent {
  id: number;
  kind: ReferentKind;
  displayName: string;
  source: string;
  /** Enough to act on it. By convention a row id lives under `id`. */
  metadata: unknown;
  turn: number;
}

/** A list NEXUS actually rendered: the only thing an ordinal may index. */
export interface RenderedList {
  id: number;
  turn: number;
  items: number[];
}

/**
 * The result of resolving a phrase like "the PR" or "the first one".
 *
 * `ambiguous` returns the candidates named rather than picking one, so the UI
 * can ask instead of guessing.
 */
export type Resolution =
  | { kind: 'resolved'; referent: Referent }
  | { kind: 'ambiguous'; candidates: Referent[] }
  | {
      kind: 'unresolved';
      reason: string;
      /**
       * True when NEXUS followed the request and the answer is no (no such
       * contact, connector not set up). False when it never made sense of
       * the words, which with an open microphone is usually a fragment of
       * the room rather than anything addressed to it.
       */
      understood: boolean;
    };

export interface SessionSnapshot {
  state: AssistantState;
  turns: Turn[];
  referents: Referent[];
  lists: RenderedList[];
  pendingApprovals: number;
  /**
   * An action a bare "yes" would run, offered by whatever ran last. Absent
   * once it expires, so a stale offer cannot be answered.
   */
  pendingFollowUp: { actionId: string; prompt: string } | null;
}

export interface ProjectContext {
  id: number;
  name: string;
  openTasks: number;
  blockedTasks: number;
}

export interface TaskContext {
  id: number;
  title: string;
  status: string;
  projectId: number;
}

/**
 * What NEXUS is working on, derived from the conversation rather than tracked
 * separately, and re-checked against the database on every read.
 */
export interface WorkContext {
  currentProject: ProjectContext | null;
  currentTask: TaskContext | null;
}

export interface AssistantContext {
  session: SessionSnapshot;
  work: WorkContext;
  recentActions: AuditEntry[];
}

// ── NEXUS-014: the conversation ─────────────────────────────────────────────

export interface Choice {
  /** A registry id from src/lib/commands.ts, not a typed action id. */
  commandId: string;
  label: string;
  /**
   * Arguments to run this choice with, when the options differ by their
   * input rather than by which action they are: two similarly-named
   * contacts are the same action twice, distinguished only by the number.
   */
  input?: unknown;
}

/**
 * What NEXUS made of a request, resolved deterministically.
 *
 * An `action` reply carries a *registry* id, not an action id: only the
 * palette bridge knows how the two relate, and a second mapping would drift.
 */
export type AssistantReply =
  | { kind: 'answer'; text: string; cited: string[] }
  | { kind: 'action'; commandId: string; summary: string; input?: unknown }
  | { kind: 'choices'; candidates: Choice[] }
  | {
      kind: 'unresolved';
      reason: string;
      /**
       * True when NEXUS followed the request and the answer is no (no such
       * contact, connector not set up). False when it never made sense of
       * the words, which with an open microphone is usually a fragment of
       * the room rather than anything addressed to it.
       */
      understood: boolean;
    };
