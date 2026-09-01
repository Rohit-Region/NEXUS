import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import './ReasoningPanel.css';

interface ProviderRow {
  id: string;
  model: string;
  reach: 'localOnly' | 'leavesMachine';
  available: boolean;
}

interface ReasoningStatus {
  providers: ProviderRow[];
  activeProvider: string | null;
  unavailableReason: string | null;
  externalReasoningAllowed: boolean;
  contentSharingAllowed: boolean;
  localProviderIsLoopback: boolean;
}

/**
 * Who NEXUS may ask when it cannot work something out itself.
 *
 * The two switches were in Rust from the start and nothing ever called them,
 * so the answer was always no and the reason was always "the reasoning
 * provider could not be reached". This is the missing surface.
 *
 * They stay two switches rather than one because they are two decisions:
 * consulting a model at all is not the same as letting your messages and
 * notes be part of the question.
 */
export function ReasoningPanel() {
  const [status, setStatus] = useState<ReasoningStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setStatus(await invoke<ReasoningStatus>('nexus_reasoning_status'));
    } catch (err) {
      setError(String(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function set(external: boolean, content: boolean) {
    setBusy(true);
    setError(null);
    try {
      setStatus(
        await invoke<ReasoningStatus>('nexus_set_reasoning_policy', {
          externalReasoningAllowed: external,
          // Turning the outer switch off turns this off with it. Leaving it
          // armed underneath would mean flipping one control back on
          // silently restores a permission the user thought they revoked.
          contentSharingAllowed: external && content,
        }),
      );
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  if (!status) {
    return (
      <section className="reasoning-panel" aria-label="Reasoning">
        <h3 className="reasoning-panel__title">Reasoning</h3>
        <p className="reasoning-panel__hint">{error ?? 'Loading...'}</p>
      </section>
    );
  }

  return (
    <section className="reasoning-panel" aria-label="Reasoning">
      <h3 className="reasoning-panel__title">Reasoning</h3>
      <p className="reasoning-panel__hint">
        Who NEXUS asks when it cannot work something out on its own. Everything
        it already understands stays local and keeps working with all of this
        off.
      </p>

      {error && (
        <p className="reasoning-panel__error" role="alert">
          {error}
        </p>
      )}

      <ul className="reasoning-panel__providers">
        {status.providers.map((p) => (
          <li className="reasoning-panel__provider" key={p.id}>
            <span className="reasoning-panel__name">
              {p.id}
              {status.activeProvider === p.id && (
                <span className="reasoning-panel__badge">in use</span>
              )}
            </span>
            <span className="reasoning-panel__meta">
              {p.reach === 'localOnly' ? 'stays on this Mac' : 'leaves this Mac'}
              {' · '}
              {p.available ? 'installed' : 'not installed'}
            </span>
          </li>
        ))}
      </ul>

      {status.activeProvider === null && status.unavailableReason && (
        <p className="reasoning-panel__hint">{status.unavailableReason}</p>
      )}

      <label className="reasoning-panel__toggle">
        <input
          type="checkbox"
          checked={status.externalReasoningAllowed}
          onChange={(e) => void set(e.target.checked, status.contentSharingAllowed)}
          disabled={busy}
        />
        <span>Let NEXUS ask a model that is not on this Mac</span>
      </label>
      <p className="reasoning-panel__hint">
        Off by default. With it off, a provider marked &ldquo;leaves this
        Mac&rdquo; is never consulted even when it is installed and working.
      </p>

      <label className="reasoning-panel__toggle">
        <input
          type="checkbox"
          checked={status.contentSharingAllowed}
          onChange={(e) => void set(status.externalReasoningAllowed, e.target.checked)}
          disabled={busy || !status.externalReasoningAllowed}
        />
        <span>Include your own content in the question</span>
      </label>
      <p className="reasoning-panel__hint">
        Separate on purpose. Without it the question carries the shape of your
        request but not the text of your notes, tasks or messages.
      </p>
    </section>
  );
}
