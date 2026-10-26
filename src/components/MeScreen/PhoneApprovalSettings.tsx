import { useCallback, useEffect, useState } from 'react';
import { Smartphone } from 'lucide-react';
import {
  phoneApprovalTest,
  phoneApprovals,
  setPhoneApprovals,
  type PhoneApprovalStatus,
} from '../../lib/nexus-db';

/**
 * Phone approvals: when NEXUS needs your OK, it asks on your phone too.
 *
 * The request goes to a private ntfy topic NEXUS generates (never the stock
 * alert topic, which anyone who knows its name could read) with Approve and
 * Deny buttons. Approving
 * there redeems the very same approval the app would, so it passes or fails the
 * same checks: single use, the exact action shown, and a short expiry.
 * Finished Clone tasks arrive with Apply and Discard. Nothing can be started
 * from the phone this way.
 */
export function PhoneApprovalSettings() {
  const [s, setS] = useState<PhoneApprovalStatus | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    void phoneApprovals()
      .then(setS)
      .catch(() => setStatus('Could not load phone approval settings.'));
  }, []);

  const toggle = useCallback(async (enabled: boolean) => {
    try {
      setS(await setPhoneApprovals(enabled));
      setStatus(enabled ? 'On. Approval requests will also go to your phone.' : 'Off.');
    } catch (err) {
      setStatus(String(err).replace(/^Error:\s*/, ''));
    }
  }, []);

  const test = useCallback(async () => {
    try {
      await phoneApprovalTest();
      setStatus('Test sent. Tap Approve or Deny on your phone; a confirmation should follow within a few seconds.');
    } catch (err) {
      setStatus(String(err).replace(/^Error:\s*/, ''));
    }
  }, []);

  if (!s) return null;

  return (
    <section className="me-group">
      <div className="me-group__head">
        <h2 className="me-group__title">
          <Smartphone size={12} strokeWidth={2} aria-hidden="true" /> Phone approvals
        </h2>
        <p className="me-group__blurb">
          When NEXUS needs your OK (sending a message, moving a ticket, applying a Clone task), it
          asks on your phone with Approve and Deny buttons. Nothing runs until you tap Approve.
        </p>
      </div>

      <div className="well">
        <div className="well__row">
          <input
            type="checkbox"
            checked={s.enabled}
            onChange={(e) => void toggle(e.target.checked)}
            aria-label="Ask me on my phone when NEXUS needs approval"
          />
          <span className="well__text">Ask me on my phone when NEXUS needs approval</span>
        </div>
        {s.approveTopic && (
          <p className="me-field__hint">
            In the ntfy app, tap + and subscribe to <code>{s.approveTopic}</code>. Keep this
            name private: it is the only thing standing between a stranger and your approval
            requests.
          </p>
        )}
        {s.enabled && (
          <div className="well__row">
            <button className="phone__test" type="button" onClick={() => void test()}>
              Send test approval
            </button>
          </div>
        )}
        <ul className="phone__facts">
          <li>An action request expires after 5 minutes; a finished Clone task waits 12 hours.</li>
          <li>Each request carries a one-time code. A tap counts once; a late or repeated tap runs nothing.</li>
          <li>If NEXUS restarts, waiting requests are dropped rather than run later.</li>
        </ul>
      </div>

      {status && <p className="me-field__hint">{status}</p>}
    </section>
  );
}
