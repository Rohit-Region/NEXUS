import { useCallback, useEffect, useState } from 'react';
import { KeyRound, Check, RefreshCw } from 'lucide-react';
import { vaultSet, vaultStatus } from '../../lib/assistant';

import './VaultPanel.css';

/** The credential this card holds. One for now; the connector allow-lists the
 *  label, so adding another is a line there and a row here. */
const LABEL = 'kai-connect';
const TITLE = 'KAI Connect';

/** How long a stored credential is considered current. The service rotates
 *  weekly, so past this the card asks to be refreshed rather than trusting a
 *  password that has almost certainly changed under it. */
const ROTATE_DAYS = 7;

const DAY = 86_400;

function agoOf(unix: number, now: number): string {
  const days = Math.floor((now - unix) / DAY);
  if (days <= 0) return 'today';
  if (days === 1) return 'yesterday';
  return `${days}d ago`;
}

/**
 * A credential you keep current by hand.
 *
 * Not a vault reader: NEXUS never sees inside Keeper. This is the opposite
 * direction. You paste the week's password once, it goes to the macOS
 * Keychain, and the card's whole job after that is to say when it was last
 * set and to ask for the next one when the week is up. The value is written
 * and never read back to the screen, so it lives in exactly one place a
 * screenshot or this transcript is not.
 */
export function VaultPanel() {
  const [present, setPresent] = useState(false);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const s = await vaultStatus(LABEL);
      setPresent(s.present);
      setUpdatedAt(s.updatedAt);
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  }, []);

  useEffect(() => {
    void refresh();
    // The staleness read only has to move once a day; a slow tick keeps the
    // "due" state honest without a timer doing anything most of the time.
    const timer = window.setInterval(
      () => setNow(Math.floor(Date.now() / 1000)),
      60_000,
    );
    return () => window.clearInterval(timer);
  }, [refresh]);

  async function save() {
    if (draft.trim() === '' || busy) return;
    setBusy(true);
    setError(null);
    try {
      await vaultSet(LABEL, draft);
      // Cleared immediately: the draft is the plaintext, and it has no reason
      // to outlive the write.
      setDraft('');
      setEditing(false);
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  const ageDays = updatedAt === null ? null : Math.floor((now - updatedAt) / DAY);
  const due = present && ageDays !== null && ageDays >= ROTATE_DAYS;
  const state = !present ? 'unset' : due ? 'due' : 'fresh';

  return (
    <section className={`vault vault--${state}`} aria-label={`${TITLE} credential`}>
      <div className="vault__head">
        <KeyRound size={12} strokeWidth={2} aria-hidden="true" />
        <span className="vault__title">{TITLE}</span>
        <span className="vault__state">
          {state === 'unset' && 'not set'}
          {state === 'fresh' && (
            <>
              <Check size={10} strokeWidth={2.5} aria-hidden="true" />
              stored
            </>
          )}
          {state === 'due' && 'refresh due'}
        </span>
      </div>

      {!editing ? (
        <div className="vault__body">
          <p className="vault__line">
            {present ? (
              <>
                Updated {updatedAt !== null ? agoOf(updatedAt, now) : 'at some point'}.
                {due && ' Rotates weekly, so it is likely stale.'}
              </>
            ) : (
              'No credential stored. Add this week’s password.'
            )}
          </p>
          <button
            className="vault__set"
            type="button"
            onClick={() => {
              setEditing(true);
              setError(null);
            }}
          >
            <RefreshCw size={12} strokeWidth={2} aria-hidden="true" />
            {present ? 'Update' : 'Add password'}
          </button>
        </div>
      ) : (
        <div className="vault__body">
          <input
            className="vault__input"
            type="password"
            autoComplete="off"
            autoFocus
            placeholder={`${TITLE} password`}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void save();
              if (e.key === 'Escape') {
                setDraft('');
                setEditing(false);
              }
            }}
            disabled={busy}
            aria-label={`${TITLE} password`}
          />
          <div className="vault__actions">
            <button
              className="vault__save"
              type="button"
              onClick={() => void save()}
              disabled={busy || draft.trim() === ''}
            >
              Save
            </button>
            <button
              className="vault__cancel"
              type="button"
              onClick={() => {
                setDraft('');
                setEditing(false);
                setError(null);
              }}
              disabled={busy}
            >
              Cancel
            </button>
          </div>
          {/* Said where it is decided, not in a policy page nobody reads. */}
          <p className="vault__note">
            Stored in the macOS Keychain. Never shown again, here or anywhere.
          </p>
        </div>
      )}

      {error && (
        <p className="vault__error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
