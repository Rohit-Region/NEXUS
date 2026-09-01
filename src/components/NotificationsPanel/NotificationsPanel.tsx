import { useCallback, useEffect, useState } from 'react';
import {
  deleteCommitment,
  listCommitments,
  notificationPolicy,
  setNotificationPolicy,
  type Aggression,
  type Commitment,
  type NotificationPolicy,
} from '../../lib/assistant';
import './NotificationsPanel.css';

/**
 * NEXUS-024 F-04 and NEXUS-028 F-05.
 *
 * Two things live here because they are the two places NEXUS acts without
 * being asked, and both of them are only defensible if the user can see
 * exactly what they turned on and turn it off again in one click.
 *
 * The applications list is the privacy boundary for the Full Disk Access
 * grant. That grant cannot be narrowed once given; this list is what NEXUS
 * chooses to use, and it starts empty. Nothing is read until a name is typed
 * here on purpose.
 */

/** Suggestions only. Anything can be typed; nothing is watched by default. */
const COMMON_APPS = ['WhatsApp', 'Teams', 'Slack', 'Messages', 'Mail'];

const AGGRESSION_LABELS: Record<Aggression, string> = {
  immediate: 'Speak as soon as anything arrives',
  batched: 'Speak once about the group',
  silent: 'Never speak first',
};

export function NotificationsPanel() {
  const [policy, setPolicy] = useState<NotificationPolicy | null>(null);
  const [commitments, setCommitments] = useState<Commitment[]>([]);
  const [draft, setDraft] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const [found, owed] = await Promise.all([
        notificationPolicy(),
        listCommitments(true),
      ]);
      setPolicy(found);
      setCommitments(owed);
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const save = useCallback(
    async (apps: string[], aggression: Aggression) => {
      setBusy(true);
      try {
        setPolicy(await setNotificationPolicy(apps, aggression));
        setError(null);
      } catch (err) {
        setError(String(err));
      } finally {
        setBusy(false);
      }
    },
    [],
  );

  if (!policy) {
    return (
      <section className="notif-panel">
        {error ? <p className="notif-error">{error}</p> : <p>Loading…</p>}
      </section>
    );
  }

  const addApp = (name: string) => {
    const clean = name.trim();
    if (!clean) return;
    // Case-insensitive, because the list is matched that way in Rust and a
    // duplicate that differs only in case would look like a bug.
    if (policy.apps.some((a) => a.toLowerCase() === clean.toLowerCase())) return;
    void save([...policy.apps, clean], policy.aggression);
    setDraft('');
  };

  return (
    <section className="notif-panel">
      <h2>Messages</h2>

      {/*
        A missing grant and an empty list are different problems with
        different fixes, so they never share a rendering. Conflating them is
        how an afternoon gets spent debugging the wrong one.
      */}
      {policy.blocked ? (
        <p className="notif-blocked">{policy.blocked}</p>
      ) : policy.apps.length === 0 ? (
        <p className="notif-hint">
          NEXUS can see notifications but is not watching anything yet. Add an
          application below and it will tell you when a message arrives.
        </p>
      ) : null}

      <div className="notif-apps">
        {policy.apps.map((app) => (
          <span key={app} className="notif-chip">
            {app}
            <button
              type="button"
              aria-label={`Stop watching ${app}`}
              disabled={busy}
              onClick={() =>
                void save(
                  policy.apps.filter((a) => a !== app),
                  policy.aggression,
                )
              }
            >
              ×
            </button>
          </span>
        ))}
      </div>

      <div className="notif-add">
        <input
          value={draft}
          placeholder="Application name"
          disabled={busy}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') addApp(draft);
          }}
        />
        <button type="button" disabled={busy} onClick={() => addApp(draft)}>
          Watch
        </button>
      </div>

      <div className="notif-suggest">
        {COMMON_APPS.filter(
          (a) => !policy.apps.some((p) => p.toLowerCase() === a.toLowerCase()),
        ).map((a) => (
          <button key={a} type="button" disabled={busy} onClick={() => addApp(a)}>
            + {a}
          </button>
        ))}
      </div>

      <h3>When something arrives</h3>
      <div className="notif-aggression">
        {(Object.keys(AGGRESSION_LABELS) as Aggression[]).map((level) => (
          <label key={level}>
            <input
              type="radio"
              name="aggression"
              checked={policy.aggression === level}
              disabled={busy}
              onChange={() => void save(policy.apps, level)}
            />
            {AGGRESSION_LABELS[level]}
          </label>
        ))}
      </div>

      <h3>Things you said you would do</h3>
      {commitments.length === 0 ? (
        <p className="notif-hint">
          Nothing yet. Say “remind me to call the dentist in twenty minutes”.
        </p>
      ) : (
        <ul className="notif-commitments">
          {commitments.map((c) => (
            <li key={c.id}>
              <span>{c.what}</span>
              <button
                type="button"
                aria-label={`Forget ${c.what}`}
                onClick={async () => {
                  await deleteCommitment(c.id);
                  await load();
                }}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}

      {error ? <p className="notif-error">{error}</p> : null}
    </section>
  );
}
