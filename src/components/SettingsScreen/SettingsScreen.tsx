import { useCallback, useEffect, useState } from 'react';
import { RotateCcw } from 'lucide-react';
import {
  getSettings,
  listAgents,
  listIdes,
  resetSettings,
  updateSettings,
  voiceListVoices,
} from '../../lib/nexus-db';
import type { RegistryEntry, Settings, VoiceOption } from '../../types/db';
import { NotificationsPanel } from '../NotificationsPanel/NotificationsPanel';
import { PermissionsPanel } from '../PermissionsPanel/PermissionsPanel';
import { ContactsPanel } from '../ContactsPanel/ContactsPanel';
import { ReasoningPanel } from '../ReasoningPanel/ReasoningPanel';
import { SettingsForm } from '../SettingsForm/SettingsForm';
import './SettingsScreen.css';

interface SettingsScreenProps {
  settings: Settings;
  onSettingsChange: (next: Settings) => void;
}

export function SettingsScreen({ settings, onSettingsChange }: SettingsScreenProps) {
  // enabledOnly = false: a currently-selected default may be disabled and must
  // still render by name (spec 008 F-11, via RegistrySelect).
  const [ides, setIdes] = useState<RegistryEntry[]>([]);
  const [agents, setAgents] = useState<RegistryEntry[]>([]);
  // NEXUS-011. Read separately from the settings load: this one call reaches
  // the speech synthesizer rather than the database, and a machine with no
  // usable voices must still be able to open Settings.
  const [voices, setVoices] = useState<VoiceOption[]>([]);
  // Re-read on mount rather than trusting the prop, so the screen shows what
  // is actually stored. The prop stays authoritative for the rest of the app.
  const [values, setValues] = useState<Settings>(settings);
  const [formKey, setFormKey] = useState(0);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [confirmReset, setConfirmReset] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [stored, ideRows, agentRows] = await Promise.all([
        getSettings(),
        listIdes(false),
        listAgents(false),
      ]);
      setValues(stored);
      setIdes(ideRows);
      setAgents(agentRows);
      setFormKey((k) => k + 1);

      // Deliberately after the form data and deliberately not fatal. An empty
      // list degrades the picker to "System default", which still speaks.
      try {
        setVoices(await voiceListVoices());
      } catch {
        setVoices([]);
      }
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function handleSave(next: Settings) {
    setSaving(true);
    setError(null);
    setSaved(false);
    try {
      // The returned struct is what was persisted, not what was submitted.
      const result = await updateSettings(next);
      setValues(result);
      setFormKey((k) => k + 1);
      onSettingsChange(result);
      setSaved(true);
    } catch (err) {
      // The form keeps the attempted values: it owns its own state and is
      // not re-keyed on failure.
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function handleReset() {
    setResetting(true);
    setError(null);
    setSaved(false);
    try {
      const result = await resetSettings();
      setValues(result);
      setFormKey((k) => k + 1);
      onSettingsChange(result);
      setConfirmReset(false);
    } catch (err) {
      setError(String(err));
    } finally {
      setResetting(false);
    }
  }

  const busy = saving || resetting;

  return (
    <section className="settings-screen" aria-label="Settings">
      <div className="settings-screen__header">
        <div>
          <h2 className="settings-screen__title">Settings</h2>
          <p className="settings-screen__subtitle">
            Workspace-wide preferences, stored locally in your NEXUS database.
          </p>
        </div>
        <button
          className="nexus-btn nexus-btn--danger"
          type="button"
          onClick={() => setConfirmReset(true)}
          disabled={busy || loading || confirmReset}
        >
          <RotateCcw size={12} strokeWidth={2} aria-hidden="true" />
          Reset
        </button>
      </div>

      {confirmReset && (
        <div
          className="settings-screen__confirm"
          role="alertdialog"
          aria-label="Confirm settings reset"
        >
          <span className="settings-screen__confirm-text">
            Reset all settings to their defaults? This cannot be undone.
          </span>
          <div className="settings-screen__confirm-actions">
            <button
              className="nexus-btn nexus-btn--secondary"
              type="button"
              onClick={() => setConfirmReset(false)}
              disabled={resetting}
            >
              Cancel
            </button>
            <button
              className="nexus-btn nexus-btn--primary"
              type="button"
              onClick={() => void handleReset()}
              disabled={resetting}
            >
              {resetting ? 'Resetting...' : 'Confirm Reset'}
            </button>
          </div>
        </div>
      )}

      {error && (
        <p className="settings-screen__error" role="alert">
          {error}
        </p>
      )}

      {saved && !error && (
        <p className="settings-screen__saved" role="status">
          Settings saved.
        </p>
      )}

      {loading && <p className="settings-screen__loading">Loading settings...</p>}

      {!loading && (
        <SettingsForm
          key={formKey}
          values={values}
          ides={ides}
          agents={agents}
          voices={voices}
          onSubmit={handleSave}
          onCancel={() => void load()}
          submitting={busy}
        />
      )}

      {/* NEXUS-012. Its own panel rather than a fieldset in SettingsForm:
          the form is presentational and calls no command, and permissions
          are read and written live rather than saved with the rest. */}
      <ReasoningPanel />

      <ContactsPanel />

      <PermissionsPanel />

      <NotificationsPanel />
    </section>
  );
}
