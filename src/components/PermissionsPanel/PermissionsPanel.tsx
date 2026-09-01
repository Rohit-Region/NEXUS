import { useCallback, useEffect, useState } from 'react';
import { RefreshCw, ShieldCheck } from 'lucide-react';
import {
  listAudit,
  listConnectors,
  setConnectorConfig,
  setConnectorEnabled,
  setPermissionGrant,
} from '../../lib/assistant';
import type {
  AuditEntry,
  ConnectorStatus,
  ConnectorView,
  Permission,
} from '../../types/assistant';
import './PermissionsPanel.css';

const LEVEL_LABEL: Record<Permission, string> = {
  read: 'Read',
  interact: 'Interact',
  write: 'Write',
  execute: 'Execute',
  destructive: 'Destructive',
};

const LEVEL_HINT: Record<Permission, string> = {
  read: 'Look at things without changing them.',
  interact: 'Open, focus and navigate.',
  write: 'Create and change data. Always asks first.',
  execute: 'Run commands, builds and tests. Always asks first.',
  destructive: 'Delete things. Always asks first, and cannot be undone.',
};

const STATUS_LABEL: Record<ConnectorStatus, string> = {
  ready: 'Ready',
  degraded: 'Partly available',
  needsAuth: 'Needs sign-in',
  disabled: 'Turned off',
  unavailable: 'Not available',
};

const OUTCOME_LABEL: Record<AuditEntry['outcome'], string> = {
  attempted: 'Started',
  succeeded: 'Done',
  failed: 'Failed',
  refused: 'Refused',
};

/** "14:32:07" from the stored ISO stamp, or the raw value if it is unparseable. */
function shortTime(stamp: string): string {
  const parsed = new Date(stamp);
  return Number.isNaN(parsed.getTime())
    ? stamp
    : parsed.toLocaleTimeString([], { hour12: false });
}

/**
 * NEXUS-012: what NEXUS may do, and what it has done.
 *
 * The toggles here reflect the permission model; they do not implement it.
 * Every grant is checked again in Rust the moment an action is attempted, so
 * turning one off stops the action rather than merely hiding a button.
 */
/**
 * Configuration a connector needs before it can do anything.
 *
 * Declared here rather than derived, because an empty config carries no
 * indication of what belongs in it: a connector that has never been set up
 * would otherwise render no fields and look finished. Secrets are absent by
 * design; Rust refuses to store them and they live in the Keychain.
 */
const CONFIG_FIELDS: Record<
  string,
  Array<{ key: string; label: string; placeholder: string; hint: string }>
> = {
  jira: [
    {
      key: 'site',
      label: 'Site address',
      placeholder: 'https://your-team.atlassian.net',
      hint: 'Enough on its own to open issues in the browser.',
    },
    {
      key: 'email',
      label: 'Account email',
      placeholder: 'you@company.com',
      hint: 'Only needed to read issues and comments, which also needs an API token in the Keychain.',
    },
  ],
};

export function PermissionsPanel() {
  const [connectors, setConnectors] = useState<ConnectorView[]>([]);
  const [audit, setAudit] = useState<AuditEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [rows, history] = await Promise.all([listConnectors(), listAudit(25)]);
      setConnectors(rows);
      setAudit(history);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function toggleGrant(
    connectorId: string,
    level: Permission,
    granted: boolean,
  ) {
    setBusy(true);
    setError(null);
    try {
      setConnectors(await setPermissionGrant(connectorId, level, granted));
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  /** Pending edits, keyed `connectorId.field`. Absent means unedited. */
  const [drafts, setDrafts] = useState<Record<string, string>>({});

  async function saveConfig(connector: ConnectorView) {
    const fields = CONFIG_FIELDS[connector.id] ?? [];
    const next: Record<string, string> = {};
    for (const field of fields) {
      const draft = drafts[`${connector.id}.${field.key}`];
      const value = draft ?? connector.config?.[field.key] ?? '';
      // Empty values are dropped rather than stored: a blank string reads as
      // configured-but-wrong, and the connectors test for emptiness anyway.
      if (value.trim().length > 0) next[field.key] = value.trim();
    }
    setBusy(true);
    setError(null);
    try {
      await setConnectorConfig(connector.id, next);
      setDrafts((prev) => {
        const rest = { ...prev };
        for (const field of fields) delete rest[`${connector.id}.${field.key}`];
        return rest;
      });
      await load();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function toggleConnector(connectorId: string, enabled: boolean) {
    setBusy(true);
    setError(null);
    try {
      setConnectors(await setConnectorEnabled(connectorId, enabled));
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="permissions-panel" aria-label="Permissions and activity">
      <div className="permissions-panel__head">
        <h3 className="permissions-panel__title">
          <ShieldCheck size={13} strokeWidth={2} aria-hidden="true" />
          Permissions
        </h3>
        <button
          className="nexus-btn nexus-btn--secondary"
          type="button"
          onClick={() => void load()}
          disabled={loading || busy}
        >
          <RefreshCw size={12} strokeWidth={2} aria-hidden="true" />
          Refresh
        </button>
      </div>

      <p className="permissions-panel__intro">
        What NEXUS is allowed to do on your behalf. These are enforced when an
        action runs, not by hiding buttons, so turning one off stops the action
        itself. Anything that writes, runs or deletes also asks you each time.
      </p>

      {error && (
        <p className="permissions-panel__error" role="alert">
          {error}
        </p>
      )}

      {loading && <p className="permissions-panel__status">Loading...</p>}

      {!loading &&
        connectors.map((connector) => (
          <div className="permissions-panel__connector" key={connector.id}>
            <div className="permissions-panel__connector-head">
              <span className="permissions-panel__connector-name">
                {connector.displayName}
              </span>
              <span className="nexus-chip">{STATUS_LABEL[connector.status]}</span>
              <span className="permissions-panel__count">
                {connector.actions.length} actions
              </span>
              <label className="permissions-panel__switch">
                <input
                  type="checkbox"
                  checked={connector.enabled}
                  onChange={(e) =>
                    void toggleConnector(connector.id, e.target.checked)
                  }
                  disabled={busy}
                />
                <span>Enabled</span>
              </label>
            </div>

            <ul className="permissions-panel__grants">
              {connector.requiredLevels.map((level) => (
                <li className="permissions-panel__grant" key={level}>
                  <label className="permissions-panel__switch">
                    <input
                      type="checkbox"
                      checked={connector.granted.includes(level)}
                      onChange={(e) =>
                        void toggleGrant(connector.id, level, e.target.checked)
                      }
                      disabled={busy || !connector.enabled}
                    />
                    <span className="permissions-panel__grant-label">
                      {LEVEL_LABEL[level]}
                    </span>
                  </label>
                  <span className="permissions-panel__grant-hint">
                    {LEVEL_HINT[level]}
                  </span>
                </li>
              ))}
            </ul>

            {CONFIG_FIELDS[connector.id] && (
              <div className="permissions-panel__config">
                {CONFIG_FIELDS[connector.id].map((field) => {
                  const id = `${connector.id}.${field.key}`;
                  return (
                    <label className="permissions-panel__config-field" key={id}>
                      <span className="permissions-panel__config-label">
                        {field.label}
                      </span>
                      <input
                        className="nexus-input"
                        type="text"
                        spellCheck={false}
                        autoComplete="off"
                        placeholder={field.placeholder}
                        value={drafts[id] ?? connector.config?.[field.key] ?? ''}
                        onChange={(e) =>
                          setDrafts((prev) => ({ ...prev, [id]: e.target.value }))
                        }
                        disabled={busy}
                      />
                      <span className="permissions-panel__grant-hint">
                        {field.hint}
                      </span>
                    </label>
                  );
                })}
                <button
                  className="nexus-btn nexus-btn--secondary"
                  type="button"
                  onClick={() => void saveConfig(connector)}
                  disabled={busy}
                >
                  Save {connector.displayName} settings
                </button>
              </div>
            )}
          </div>
        ))}

      <h3 className="permissions-panel__title permissions-panel__title--sub">
        Recent activity
      </h3>
      <p className="permissions-panel__intro">
        Every action NEXUS took or refused, newest first. Deliberately records
        what it did, never what it read.
      </p>

      {!loading && audit.length === 0 && (
        <p className="permissions-panel__status">Nothing yet.</p>
      )}

      {audit.length > 0 && (
        <ul className="permissions-panel__audit">
          {audit.map((entry) => (
            <li
              className={`permissions-panel__audit-row permissions-panel__audit-row--${entry.outcome}`}
              key={entry.id}
            >
              <span className="permissions-panel__audit-time">
                {shortTime(entry.createdAt)}
              </span>
              <span className="permissions-panel__audit-summary">
                {entry.summary}
              </span>
              {entry.approved && <span className="nexus-chip">Approved</span>}
              <span className="permissions-panel__audit-outcome">
                {OUTCOME_LABEL[entry.outcome]}
                {entry.error ? ` (${entry.error})` : ''}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
