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
export function App() {
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const stored = await getSettings();
        if (!cancelled) setSettings(stored);
      } catch (err) {
        // A preferences failure is not a reason to make the application
        // unusable: fall back to defaults and surface it non-blockingly
        // (spec 008 2.7, F-18).
        if (!cancelled) {
          setSettings(DEFAULT_SETTINGS);
          setLoadError(String(err));
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
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
    return <div className="app-boot" aria-busy="true" />;
  }

  return (
    <AppShell
      settings={settings}
      onSettingsChange={handleSettingsChange}
      settingsError={loadError}
    />
  );
}
