import { useCallback, useEffect, useState } from 'react';
import { RefreshCw, ShieldCheck } from 'lucide-react';
import {
  listAudit,
  listConnectors,
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
