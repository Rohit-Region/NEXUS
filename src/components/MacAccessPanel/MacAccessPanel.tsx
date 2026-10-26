import { useEffect, useState } from 'react';
import { calendarEnabled, setCalendarEnabled } from '../../lib/nexus-db';
import './MacAccessPanel.css';

interface Props {
  voiceEnabled: boolean;
  onVoiceChange: (on: boolean) => void;
  disabled: boolean;
}

/**
 * Switches for the features that make macOS ask for access.
 *
 * Off means NEXUS never touches the device, so macOS never asks. These do not
 * revoke a grant already given; that lives in System Settings.
 */
export function MacAccessPanel({ voiceEnabled, onVoiceChange, disabled }: Props) {
  const [calendar, setCalendar] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void calendarEnabled()
      .then(setCalendar)
      .catch(() => setError('Could not read the calendar switch.'));
  }, []);

  const toggleCalendar = async (on: boolean) => {
    try {
      setCalendar(await setCalendarEnabled(on));
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  };

  return (
    <section className="mac-access" aria-label="Mac access">
      <h2>Mac access</h2>
      <p className="mac-access__hint">
        Turn off a feature you do not use and NEXUS stops asking macOS for its access.
      </p>

      <label className="mac-access__row">
        <input
          type="checkbox"
          checked={voiceEnabled}
          disabled={disabled}
          onChange={(e) => onVoiceChange(e.target.checked)}
        />
        <span>
          <strong>Microphone</strong>
          <span className="mac-access__what">Voice commands</span>
        </span>
      </label>

      <label className="mac-access__row">
        <input
          type="checkbox"
          checked={calendar ?? false}
          disabled={calendar === null}
          onChange={(e) => void toggleCalendar(e.target.checked)}
        />
        <span>
          <strong>Calendar</strong>
          <span className="mac-access__what">Schedule card, Today line, quiet nudges during meetings</span>
        </span>
      </label>

      {error && <p className="mac-access__error">{error}</p>}
    </section>
  );
}
