import { useCallback, useEffect, useState } from 'react';
import { CloudSun, RefreshCw } from 'lucide-react';
import { runAction } from '../../lib/assistant';
import './WeatherCard.css';

interface Reading {
  report: string;
  location: string | null;
}

/**
 * Pull the temperature out of the report.
 *
 * wttr.in returns a sentence, not a structure, so this reads the number back
 * out of it. It fails to `null` rather than guessing: a wrong temperature
 * shown confidently is worse than none, and the full sentence is displayed
 * underneath either way.
 */
function degreesOf(report: string): string | null {
  const match = report.match(/(-?\d+)\s*°?\s*C/i);
  return match ? match[1] : null;
}

export function WeatherCard() {
  const [reading, setReading] = useState<Reading | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    setBusy(true);
    try {
      // Through the gate like anything else: weather leaves the machine, so
      // it is refused outright without the connector's Read grant rather
      // than quietly reaching a service the user never allowed.
      const outcome = await runAction({ actionId: 'weather.current' });
      const output = outcome.output as Reading;
      setReading(output);
      setError(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const degrees = reading ? degreesOf(reading.report) : null;

  return (
    <section className="weather" aria-label="Weather">
      <div className="weather__icon" aria-hidden="true">
        <CloudSun size={38} strokeWidth={1.2} />
      </div>

      <div className="weather__body">
        <span className="weather__place">
          {reading?.location ?? 'Your location'}
        </span>

        {degrees !== null ? (
          <span className="weather__temp">
            {degrees}
            <span className="weather__unit">°C</span>
          </span>
        ) : (
          <span className="weather__temp weather__temp--none">--</span>
        )}

        {/* The connector's own sentence, kept whole. The big number is a
            convenience read out of it, and this is the thing that is
            actually true. */}
        <span className="weather__report">
          {error ?? reading?.report ?? 'Reading...'}
        </span>
      </div>

      <button
        className={`weather__refresh${busy ? ' weather__refresh--busy' : ''}`}
        type="button"
        onClick={() => void load()}
        disabled={busy}
        aria-label="Refresh weather"
      >
        <RefreshCw size={13} strokeWidth={2} aria-hidden="true" />
      </button>
    </section>
  );
}
