/**
 * NEXUS-012: the assistant action layer's IPC surface.
 *
 * Parallel to nexus-db.ts, and subject to the same rule: AppShell imports
 * neither. Everything here goes through one Rust command, which goes through
 * one Rust function, which is the only thing that can reach a connector.
 *
 * Errors from `runAction` arrive as structured ActionError values rather than
 * strings, because the caller has to distinguish "not allowed" from "waiting
 * for you" from "that broke", and string matching on error text is how those
 * distinctions get lost.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  ActionError,
  ActionOutcome,
  ActionRequest,
  AssistantContext,
  AuditEntry,
  ConnectorView,
  NeedsApproval,
  AssistantReply,
  Permission,
  Resolution,
  SessionSnapshot,
} from '../types/assistant';
import type { VoiceCommandSpec } from '../types/db';

/** Narrow an unknown rejection to a structured action error. */
export function isActionError(value: unknown): value is ActionError {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as { kind?: unknown }).kind === 'string'
  );
}

export function isNeedsApproval(value: unknown): value is NeedsApproval {
  return isActionError(value) && value.kind === 'needsApproval';
}

/**
 * Human-readable text for an error.
 *
 * Says what went wrong and what to do about it. Mirrors the Display impl in
 * Rust rather than reusing it, because the UI can point at a specific screen
 * and the backend cannot.
 */
export function describeActionError(error: ActionError): string {
  switch (error.kind) {
    case 'unknownAction':
      return `NEXUS has no action called ${error.actionId}.`;
    case 'connectorDisabled':
      return `The ${error.connectorId} connector is turned off. Turn it back on in Settings.`;
    case 'notPermitted':
      return `NEXUS is not allowed to ${error.level} on your behalf. Enable it under Permissions in Settings.`;
    case 'needsApproval':
      return error.summary;
    case 'invalidApproval':
      return `That approval is no longer valid: ${error.reason}. Try again.`;
    case 'invalidInput':
      return `That request was malformed: ${error.detail}`;
    case 'failed':
      return error.detail;
  }
}

/**
 * Perform one action.
 *
 * Rejects with an ActionError. A `needsApproval` rejection is not a failure:
 * show the prompt, then call again with `approval` set to the token.
 */
export function runAction(request: ActionRequest): Promise<ActionOutcome> {
  return invoke<ActionOutcome>('nexus_execute_action', { request });
}

/** Withdraw a pending approval. Returns how many are still waiting. */
export function cancelApproval(token: number): Promise<number> {
  return invoke<number>('nexus_cancel_approval', { token });
}

export function listConnectors(): Promise<ConnectorView[]> {
  return invoke<ConnectorView[]>('nexus_list_connectors');
}

/** Grant or revoke a level. Returns the refreshed connector list. */
export function setPermissionGrant(
  connectorId: string,
  level: Permission,
  granted: boolean,
): Promise<ConnectorView[]> {
  return invoke<ConnectorView[]>('nexus_set_permission_grant', {
    connectorId,
    level,
    granted,
  });
}

export function setConnectorEnabled(
  connectorId: string,
  enabled: boolean,
): Promise<ConnectorView[]> {
  return invoke<ConnectorView[]>('nexus_set_connector_enabled', {
    connectorId,
    enabled,
  });
}

export function listAudit(limit = 50): Promise<AuditEntry[]> {
  return invoke<AuditEntry[]>('nexus_list_audit', { limit });
}

// ── Assistant state and context (NEXUS-013) ─────────────────────────────────
//
// State is read on demand; the event below is the hint to re-read rather than
// a payload to trust. Nothing here is persisted: the conversation lives in
// memory and is gone on restart, deliberately.

/** What NEXUS is doing, plus the conversation so far. */
export function assistantSnapshot(): Promise<SessionSnapshot> {
  return invoke<SessionSnapshot>('nexus_assistant_snapshot');
}

/** Conversation, work context and recent actions, assembled to a budget. */
export function assistantContext(): Promise<AssistantContext> {
  return invoke<AssistantContext>('nexus_assistant_context');
}

/**
 * Resolve a phrase like "the PR" or "the first one" against the conversation.
 *
 * Deterministic and provider-free. An `ambiguous` result carries the
 * candidates so the caller can ask which one rather than picking.
 */
export function assistantResolve(phrase: string): Promise<Resolution> {
  return invoke<Resolution>('nexus_assistant_resolve', { phrase });
}

/**
 * Record that NEXUS rendered a list, in the order the user saw it.
 *
 * Ids must come from a snapshot, so a caller can only compose a list out of
 * referents NEXUS itself created. Returns null for an empty list.
 */
export function assistantRememberList(items: number[]): Promise<number | null> {
  return invoke<number | null>('nexus_assistant_remember_list', { items });
}

/** Return to rest once a finished turn has been shown. */
export function assistantSettle(): Promise<SessionSnapshot> {
  return invoke<SessionSnapshot>('nexus_assistant_settle');
}

/** Forget the conversation. */
export function assistantClear(): Promise<SessionSnapshot> {
  return invoke<SessionSnapshot>('nexus_assistant_clear');
}

/** Fires whenever assistant state changes, so the UI never has to poll. */
export function onAssistantState(
  cb: (snapshot: SessionSnapshot) => void,
): Promise<UnlistenFn> {
  return listen<SessionSnapshot>('nexus://assistant/state', (e) => cb(e.payload));
}

/**
 * Ask NEXUS something.
 *
 * The command registry travels with the request so src/lib/commands.ts stays
 * the single definition of what NEXUS can do, exactly as the voice path
 * already does it. No reasoning provider is involved: resolution is a local
 * answer, then the deterministic matcher, then a plain refusal.
 */
export function assistantAsk(
  text: string,
  spoken: boolean,
  commands: VoiceCommandSpec[],
  projectNames: string[],
): Promise<AssistantReply> {
  return invoke<AssistantReply>('nexus_assistant_ask', {
    text,
    spoken,
    commands,
    projectNames,
  });
}

/** Abandon the open turn. */
export function assistantCancelTurn(): Promise<SessionSnapshot> {
  return invoke<SessionSnapshot>('nexus_assistant_cancel_turn');
}
