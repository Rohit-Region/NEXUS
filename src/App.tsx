import { useCallback, useEffect, useState } from 'react';
import { AppShell } from './components/AppShell/AppShell';
import { getSettings } from './lib/nexus-db';
import { DEFAULT_SETTINGS } from './types';
import type { Settings } from './types/db';

/**
 * Owns settings so AppShell does not have to.
 *
 * The NEXUS-005 boundary stands: AppShell imports nothing from
 * src/lib/nexus-db.ts. Settings reach it as props from here (spec 008 7.1).
 * React Context is deliberately not used.
 */
/** Reject if a promise has not settled within `ms`. */
function withTimeout<T>(work: Promise<T>, ms: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = window.setTimeout(
      () => reject(new Error(`timed out after ${ms}ms`)),
      ms,
    );
    work.then(
      (v) => {
        window.clearTimeout(timer);
        resolve(v);
      },
      (e) => {
        window.clearTimeout(timer);
        reject(e);
      },
    );
  });
}

const BOOT_ATTEMPTS = 6;
const BOOT_ATTEMPT_MS = 2000;
const BOOT_GAP_MS = 350;

/**
 * Read settings, tolerating a backend that is not ready yet.
 *
 * Returns the settings on success, or the last error after every attempt is
 * spent. Never rejects, never hangs: the caller can always stop showing the
 * boot screen once this returns.
 */
async function loadSettingsWithRetry(): Promise<{
  settings: Settings | null;
  error?: string;
}> {
  let lastError = 'Settings could not be read.';
  for (let attempt = 0; attempt < BOOT_ATTEMPTS; attempt += 1) {
    try {
      const stored = await withTimeout(getSettings(), BOOT_ATTEMPT_MS);
      return { settings: stored };
    } catch (err) {
      lastError = String(err);
      // A settled failure that is not a timeout (bad data, real error) will
      // not fix itself on retry, but the IPC-not-ready case will, and the two
      // are indistinguishable here, so give every attempt the same short gap.
      await new Promise((r) => window.setTimeout(r, BOOT_GAP_MS));
    }
  }
  return { settings: null, error: lastError };
}

export function App() {
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      // The first IPC call of the session can race the backend: the webview
      // is painting before Tauri has finished wiring its command handlers, so
      // an early invoke sometimes hangs or errors. On the empty boot screen
      // that showed as a black window that never resolved. So this retries a
      // few times with a per-attempt deadline, and whatever happens it stops
      // waiting and renders the app rather than sitting blank forever.
      const result = await loadSettingsWithRetry();
      if (cancelled) return;
      if (result.settings) {
        setSettings(result.settings);
      } else {
        setSettings(DEFAULT_SETTINGS);
        setLoadError(result.error ?? 'Settings could not be read.');
      }
      setLoading(false);
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const handleSettingsChange = useCallback((next: Settings) => {
    setSettings(next);
  }, []);

  // AppShell seeds its initial screen from settings.launchScreen and must not
  // mount with a value it would immediately contradict. One local round trip.
  if (loading) {
    // Visibly loading, not visibly dead. An empty div here is the same black
    // as a crashed webview, so the boot screen says its own name and breathes.
    return (
      <div className="app-boot" aria-busy="true" aria-label="Starting NEXUS">
        <span className="app-boot__mark">&gt;_</span>
        <span className="app-boot__word">
          NE<span className="app-boot__x">X</span>US
        </span>
      </div>
    );
  }

  return (
    <AppShell
      settings={settings}
      onSettingsChange={handleSettingsChange}
      settingsError={loadError}
    />
  );
}
