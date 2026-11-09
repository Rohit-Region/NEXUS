import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { storeConnectorSecret } from '../../lib/assistant';
import { IS_PHONE } from '../../lib/platform';
import './ReasoningPanel.css';

interface ProviderRow {
  id: string;
  model: string;
  reach: 'localOnly' | 'leavesMachine';
  available: boolean;
}

interface CloudVendor {
  id: string;
  model: string;
  configured: boolean;
}

interface ReasoningStatus {
  providers: ProviderRow[];
  cloudVendors: CloudVendor[];
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
const VENDOR_NAME: Record<string, string> = {
  anthropic: 'Claude (Anthropic)',
  openai: 'OpenAI',
  groq: 'Groq (free, used when the others cannot answer)',
};

/** Where "this Mac" is, in the copy below. */
const DEVICE = IS_PHONE ? 'phone' : 'Mac';

export function ReasoningPanel() {
  const [status, setStatus] = useState<ReasoningStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // API keys being typed, by vendor. Cleared once saved; a saved key is
  // never read back into the page.
  const [keys, setKeys] = useState<Record<string, string>>({});

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

  async function saveKey(vendor: string) {
    setBusy(true);
    setError(null);
    try {
      await storeConnectorSecret('reasoning', vendor, keys[vendor] ?? '');
      setKeys((k) => ({ ...k, [vendor]: '' }));
      await load();
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
              {p.reach === 'localOnly' ? `stays on this ${DEVICE}` : `leaves this ${DEVICE}`}
              {' · '}
              {p.available ? 'installed' : 'not installed'}
            </span>
          </li>
        ))}
      </ul>

      <ul className="reasoning-panel__keys" aria-label="API keys">
        {status.cloudVendors.map((v) => (
          <li className="reasoning-panel__key" key={v.id}>
            <span className="reasoning-panel__name">
              {VENDOR_NAME[v.id] ?? v.id}
              <span className="reasoning-panel__meta">{v.configured ? 'key saved' : 'no key'}</span>
            </span>
            <span className="reasoning-panel__key-row">
              <input
                className="nexus-field"
                type="password"
                autoComplete="off"
                placeholder={v.configured ? 'Replace the key' : 'Paste an API key'}
                value={keys[v.id] ?? ''}
                onChange={(e) => setKeys((k) => ({ ...k, [v.id]: e.target.value }))}
                aria-label={`${VENDOR_NAME[v.id] ?? v.id} API key`}
              />
              <button
                className="nexus-btn nexus-btn--secondary"
                type="button"
                onClick={() => void saveKey(v.id)}
                disabled={busy || !(keys[v.id] ?? '').trim()}
              >
                Save
              </button>
            </span>
          </li>
        ))}
      </ul>
      <p className="reasoning-panel__hint">
        Kept {IS_PHONE ? "encrypted in this phone's secure storage" : 'in the macOS Keychain'}, never in the
        database. Used only with the switch below on.
      </p>

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
        <span>Let NEXUS ask a model that is not on this {DEVICE}</span>
      </label>
      <p className="reasoning-panel__hint">
        Off by default. With it off, a provider marked &ldquo;leaves this{' '}
        {DEVICE}&rdquo; is never consulted even when it is installed and working.
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
