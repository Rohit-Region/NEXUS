import { useCallback, useEffect, useState } from 'react';
import { HeartPulse } from 'lucide-react';
import {
  setWellbeingSettings,
  wellbeingSettings,
  type WellbeingSettings as Settings,
} from '../../lib/nexus-db';

/**
 * Wellbeing settings: break, water and wind-down nudges.
 *
 * Breaks follow real activity (time since your last keyboard or mouse input),
 * so a nudge never arrives while you are already away. Nothing fires during a
 * calendar meeting; it waits until the meeting ends. Saved on every change,
 * and the stored (clamped) values are shown back, so the form never claims a
 * setting that was not kept.
 */
export function WellbeingSettings() {
  const [s, setS] = useState<Settings | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    void wellbeingSettings()
      .then(setS)
      .catch(() => setStatus('Could not load wellbeing settings.'));
  }, []);

  const save = useCallback(async (next: Settings) => {
    setS(next);
    try {
      setS(await setWellbeingSettings(next));
      setStatus('Saved.');
    } catch (err) {
      setStatus(`Could not save: ${String(err).replace(/^Error:\s*/, '')}`);
    }
  }, []);

  if (!s) return null;

  const num = (key: 'breakMinutes' | 'hydrationMinutes' | 'awayMinutes', raw: string) => {
    const n = Number(raw);
    if (Number.isFinite(n)) void save({ ...s, [key]: Math.round(n) });
  };

  return (
    <section className="me-group">
      <div className="me-group__head">
        <h2 className="me-group__title">
          <HeartPulse size={12} strokeWidth={2} aria-hidden="true" /> Wellbeing
        </h2>
        <p className="me-group__blurb">
          Gentle nudges as macOS notifications. Breaks count real time at the keyboard; stepping
          away for a few minutes resets them. Nothing fires during a meeting.
        </p>
      </div>

      <div className="well">
        <div className="well__row">
          <input
            type="checkbox"
            checked={s.breaks}
            onChange={(e) => void save({ ...s, breaks: e.target.checked })}
            aria-label="Break and posture reminders"
          />
          <span className="well__text">Break and posture reminder after</span>
          <input
            className="me-field__input well__num"
            type="number"
            min={15}
            max={180}
            defaultValue={s.breakMinutes}
            key={`b-${s.breakMinutes}`}
            onBlur={(e) => num('breakMinutes', e.target.value)}
            disabled={!s.breaks}
            aria-label="Minutes before a break reminder"
          />
          <span className="well__text">minutes at the keyboard</span>
        </div>

        <div className="well__row">
          <input
            type="checkbox"
            checked={s.hydration}
            onChange={(e) => void save({ ...s, hydration: e.target.checked })}
            aria-label="Water reminders"
          />
          <span className="well__text">Water reminder every</span>
          <input
            className="me-field__input well__num"
            type="number"
            min={30}
            max={240}
            defaultValue={s.hydrationMinutes}
            key={`h-${s.hydrationMinutes}`}
            onBlur={(e) => num('hydrationMinutes', e.target.value)}
            disabled={!s.hydration}
            aria-label="Minutes between water reminders"
          />
          <span className="well__text">active minutes</span>
        </div>

        <div className="well__row">
          <input
            type="checkbox"
            checked={s.winddown}
            onChange={(e) => void save({ ...s, winddown: e.target.checked })}
            aria-label="Wind-down reminder"
          />
          <span className="well__text">Wind-down reminder at</span>
          <input
            className="me-field__input well__time"
            type="time"
            defaultValue={s.winddownAt}
            key={`w-${s.winddownAt}`}
            onBlur={(e) => void save({ ...s, winddownAt: e.target.value })}
            disabled={!s.winddown}
            aria-label="Wind-down time"
          />
          <span className="well__text">if you're still at the Mac</span>
        </div>

        <div className="well__row well__row--sub">
          <span className="well__text">Count it as a break after</span>
          <input
            className="me-field__input well__num"
            type="number"
            min={2}
            max={30}
            defaultValue={s.awayMinutes}
            key={`a-${s.awayMinutes}`}
            onBlur={(e) => num('awayMinutes', e.target.value)}
            aria-label="Minutes away that count as a break"
          />
          <span className="well__text">minutes away from the keyboard</span>
        </div>
      </div>

      {status && <p className="me-field__hint">{status}</p>}
    </section>
  );
}
