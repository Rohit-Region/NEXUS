import { useCallback, useEffect, useState } from 'react';
import { PhoneCall, X } from 'lucide-react';
import { callSettings, callTest, setCallSettings, type CallSettings, type CallView } from '../../lib/nexus-db';

const LEADS = [1, 2, 5, 10, 15];

/**
 * Phone call alerts: the phone rings, alarm-style, and says why. A meeting
 * about to start, a message from someone you picked, a reminder that is due.
 *
 * Free, through ntfy on a private topic NEXUS makes. Not a real phone call:
 * it needs the phone online and the Mac awake.
 */
export function PhoneCallSettings() {
  const [view, setView] = useState<CallView | null>(null);
  const [person, setPerson] = useState('');
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    void callSettings()
      .then(setView)
      .catch(() => setStatus('Could not load phone call settings.'));
  }, []);

  const save = useCallback(async (next: CallSettings) => {
    try {
      setView(await setCallSettings(next));
      setStatus(null);
    } catch (err) {
      setStatus(String(err).replace(/^Error:\s*/, ''));
    }
  }, []);

  if (!view) return status ? <p className="me-field__hint">{status}</p> : null;
  const s = view.settings;

  const addPerson = () => {
    const name = person.trim();
    if (name.length < 3) {
      setStatus('Use at least 3 letters of the name, as it shows in WhatsApp, Teams or Mail.');
      return;
    }
    setPerson('');
    void save({ ...s, people: [...s.people, name] });
  };

  return (
    <section className="me-group">
      <div className="me-group__head">
        <h2 className="me-group__title">
          <PhoneCall size={12} strokeWidth={2} aria-hidden="true" /> Phone call alerts
        </h2>
        <p className="me-group__blurb">
          Your phone rings like an alarm and tells you why: a meeting about to start, a message from someone you
          choose, or a reminder that is due. Free, through the ntfy app.
        </p>
      </div>

      <div className="well">
        <label className="well__row">
          <input type="checkbox" checked={s.enabled} onChange={(e) => void save({ ...s, enabled: e.target.checked })} />
          <span className="well__text">Ring my phone</span>
        </label>

        {s.enabled && view.topic && (
          <p className="me-field__hint">
            In the ntfy app, tap + and subscribe to <code>{view.topic}</code>. Then open that subscription's settings
            and turn on <strong>Keep alerting for highest priority</strong>, so it keeps ringing until you look. Keep
            the name private.
          </p>
        )}

        {s.enabled && (
          <>
            <label className="well__row well__row--sub">
              <input type="checkbox" checked={s.meetings} onChange={(e) => void save({ ...s, meetings: e.target.checked })} />
              <span className="well__text">Meetings, </span>
              <select
                className="well__num"
                value={s.meetingLead}
                onChange={(e) => void save({ ...s, meetingLead: Number(e.target.value) })}
                aria-label="Minutes before the meeting"
              >
                {LEADS.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
              <span className="well__text">min before they start</span>
            </label>

            <label className="well__row well__row--sub">
              <input type="checkbox" checked={s.reminders} onChange={(e) => void save({ ...s, reminders: e.target.checked })} />
              <span className="well__text">Reminders, when they are due</span>
            </label>

            <div className="well__row well__row--sub calls__people">
              <span className="well__text">Messages from:</span>
              {s.people.map((p) => (
                <span className="calls__chip" key={p}>
                  {p}
                  <button
                    type="button"
                    onClick={() => void save({ ...s, people: s.people.filter((x) => x !== p) })}
                    aria-label={`Stop ringing for ${p}`}
                  >
                    <X size={10} strokeWidth={2.5} aria-hidden="true" />
                  </button>
                </span>
              ))}
              <input
                className="calls__input"
                type="text"
                placeholder="Name, e.g. Ghazal"
                value={person}
                onChange={(e) => setPerson(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && addPerson()}
                aria-label="Add a person to ring for"
              />
              <button className="phone__test" type="button" onClick={addPerson}>
                Add
              </button>
            </div>

            <label className="well__row well__row--sub">
              <input
                type="checkbox"
                checked={s.includeText}
                onChange={(e) => void save({ ...s, includeText: e.target.checked })}
              />
              <span className="well__text">Include what the message says (off: only who sent it)</span>
            </label>

            <label className="well__row well__row--sub">
              <input
                type="checkbox"
                checked={s.onlyWhenAway}
                onChange={(e) => void save({ ...s, onlyWhenAway: e.target.checked })}
              />
              <span className="well__text">Only when I'm away from the Mac (no keyboard or mouse for 2 minutes)</span>
            </label>

            <div className="well__row">
              <button
                className="phone__test"
                type="button"
                onClick={() =>
                  void callTest()
                    .then(() => setStatus('Test sent. Your phone should ring now.'))
                    .catch((err) => setStatus(String(err).replace(/^Error:\s*/, '')))
                }
              >
                Send a test call
              </button>
            </div>

            <ul className="phone__facts">
              <li>Messages come from the apps NEXUS watches (Settings, Messages): WhatsApp, Teams and Mail.</li>
              <li>Each meeting, message and reminder rings once. Old overdue reminders do not go off.</li>
              <li>Needs the Mac awake and the phone online. A closed lid puts the Mac to sleep.</li>
            </ul>
          </>
        )}
      </div>

      {status && <p className="me-field__hint">{status}</p>}
    </section>
  );
}
