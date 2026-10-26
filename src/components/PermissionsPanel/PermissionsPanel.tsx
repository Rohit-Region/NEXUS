import { useCallback, useEffect, useState } from 'react';
import { RefreshCw, ShieldCheck } from 'lucide-react';
import {
  listAudit,
  refreshConnectors,
  connectorSecretPresent,
  setConnectorConfig,
  storeConnectorSecret,
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
  Array<{
    key: string;
    label: string;
    placeholder: string;
    hint: string;
    /**
     * A credential rather than a setting.
     *
     * These never go into `config_json`, which refuses secret-looking keys.
     * They go to the Keychain through their own command, which is why they
     * are marked here rather than being just another text field.
     */
    secret?: boolean;
  }>
> = {
  github: [
    {
      key: 'accounts',
      label: 'Work account',
      placeholder: 'owner=login, e.g. acme=jane-at-acme',
      hint: 'Which GitHub login can see which organisation. Needed only when one machine has two GitHub identities; without it NEXUS uses whichever account gh has active.',
    },
    {
      key: 'token',
      label: 'Token for that account',
      placeholder: 'github_pat_...',
      hint: 'A fine-grained token with read access to that organisation. Stored in the Keychain, never in the database. Leave blank to let gh supply it instead.',
      secret: true,
    },
  ],
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

/**
 * Render a stored config value back into the text the user typed.
 *
 * GitHub's mapping is stored as an object because that is the shape the
 * connector reads, but it is *entered* as `owner=login`. Without this the
 * field rendered blank whenever a mapping already existed, so it looked
 * unset, and saving a token beside it failed with "fill in the account
 * first" against an account that was in fact already configured.
 */
function asText(value: unknown): string {
  if (typeof value === 'string') return value;
  if (value && typeof value === 'object') {
    return Object.entries(value as Record<string, unknown>)
      .map(([k, v]) => `${k}=${String(v)}`)
      .join(', ');
  }
  return '';
}


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
      const [rows, history] = await Promise.all([refreshConnectors(), listAudit(25)]);
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

  // Asked once the connector list is in, because the account a credential is
  // keyed by comes from that connector's own configuration.
  useEffect(() => {
    void (async () => {
      const found: Record<string, boolean> = {};
      for (const connector of connectors) {
        if (!CONFIG_FIELDS[connector.id]?.some((f) => f.secret)) continue;
        const account = secretAccount(connector, connector.config ?? {});
        if (!account) continue;
        try {
          found[connector.id] = await connectorSecretPresent(connector.id, account);
        } catch {
          // Not knowing is the same as not showing a reassurance.
        }
      }
      setHasSecret(found);
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connectors]);

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
  /**
   * Which connectors already hold a credential.
   *
   * Presence only. The value is never read back into the UI: a Settings
   * screen that can display a stored token is a Settings screen that can
   * leak one.
   */
  const [hasSecret, setHasSecret] = useState<Record<string, boolean>>({});

  /**
   * Which account a credential belongs to.
   *
   * For GitHub that is the login on the right of the `owner=login` mapping,
   * because the token is that account's, not the organisation's. Everything
   * else keys its credential by the configured email.
   */
  function secretAccount(
    connector: ConnectorView,
    values: Record<string, unknown>,
  ): string {
    if (connector.id === 'github') {
      const mapping = values.accounts;
      // Two shapes reach here: the `owner=login` text the user is typing,
      // and the stored object it becomes. Both have to resolve to a login,
      // or a saved credential could be keyed differently from a checked one.
      if (typeof mapping === 'string') {
        return mapping.split('=')[1]?.trim() ?? '';
      }
      if (mapping && typeof mapping === 'object') {
        const first = Object.values(mapping as Record<string, unknown>)[0];
        return typeof first === 'string' ? first.trim() : '';
      }
      return '';
    }
    return typeof values.email === 'string' ? values.email : '';
  }

  async function saveConfig(connector: ConnectorView) {
    const fields = CONFIG_FIELDS[connector.id] ?? [];
    const next: Record<string, string> = {};
    const values: Record<string, string> = {};
    let secret = '';

    for (const field of fields) {
      const draft = drafts[`${connector.id}.${field.key}`];
      // Through `asText`, because a stored value is not always a string:
      // GitHub's mapping is an object, and calling `.trim()` on it threw a
      // TypeError that aborted the whole save. The display path was fixed
      // and this one was not, so the field looked right and saving did
      // nothing.
      const value = draft ?? asText(connector.config?.[field.key]);
      if (field.secret) {
        // Deliberately not put in `next`: it must never reach config_json.
        secret = value.trim();
        continue;
      }
      values[field.key] = value.trim();
      // Empty values are dropped rather than stored: a blank string reads as
      // configured-but-wrong, and the connectors test for emptiness anyway.
      if (value.trim().length > 0) next[field.key] = value.trim();
    }

    // GitHub's mapping is entered as `owner=login` and stored as an object,
    // because that is the shape the connector reads.
    if (connector.id === 'github' && next.accounts) {
      const [owner, login] = next.accounts.split('=').map((p) => p.trim());
      if (!owner || !login) {
        setError('Write the work account as owner=login.');
        return;
      }
      delete next.accounts;
      (next as Record<string, unknown>).accounts = { [owner]: login };
    }
    setBusy(true);
    setError(null);
    try {
      await setConnectorConfig(connector.id, next);
      // The credential goes second and separately, so a rejected config
      // never leaves a token stored against settings that were not saved.
      if (secret.length > 0) {
        // Falls back to what is already stored, so a token can be added or
        // rotated without retyping the account it belongs to.
        const account =
          secretAccount(connector, values) ||
          secretAccount(connector, connector.config ?? {});
        if (!account) {
          setError('Fill in the account first, so the credential has an owner.');
          return;
        }
        await storeConnectorSecret(connector.id, account, secret);
      }
      setDrafts((prev) => {
        const rest = { ...prev };
        for (const field of fields) delete rest[`${connector.id}.${field.key}`];
        return rest;
      });
      await load();
    } catch (err) {
      // Named with the connector, because this panel saves several and a
      // bare message leaves the user guessing which one refused.
      setError(`${connector.displayName}: ${String(err)}`);
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
                        /* A credential is masked and never prefilled: the
                           stored value is not readable from here at all. */
                        type={field.secret ? 'password' : 'text'}
                        spellCheck={false}
                        autoComplete="off"
                        placeholder={
                          field.secret && hasSecret[connector.id]
                            ? 'stored; type to replace'
                            : field.placeholder
                        }
                        value={
                          drafts[id] ??
                          (field.secret ? '' : asText(connector.config?.[field.key]))
                        }
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
