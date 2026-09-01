import { useState } from 'react';
import {
  PROJECT_SORT_OPTIONS,
  REGISTRY_SORT_OPTIONS,
  TASK_SORT_OPTIONS,
} from '../../lib/list-filters';
import type {
  ProjectSortMode,
  RegistrySortMode,
  SortOption,
  TaskSortMode,
} from '../../types';
import type {
  RegistryEntry,
  Settings,
  TaskStatus,
  VoiceOption,
} from '../../types/db';
import { RegistrySelect } from '../RegistryPanel/RegistrySelect';
import { StatusPill, TASK_STATUS_ORDER, formatStatus } from '../TaskCard/TaskCard';
import './SettingsForm.css';

interface SettingsFormProps {
  values: Settings;
  /** Supplied by the screen; the form itself never calls a command. */
  ides: RegistryEntry[];
  agents: RegistryEntry[];
  /**
   * NEXUS-011. Voices installed on this machine, read at display time because
   * the list is machine-specific. Empty is a legitimate state: the picker
   * then offers only the system default.
   */
  voices: VoiceOption[];
  onSubmit: (values: Settings) => Promise<void>;
  onCancel: () => void;
  submitting: boolean;
}

function SortField<T extends string>({
  id,
  label,
  value,
  options,
  onChange,
  disabled,
}: {
  id: string;
  label: string;
  value: T;
  options: SortOption<T>[];
  onChange: (value: T) => void;
  disabled: boolean;
}) {
  return (
    <div className="settings-form__field">
      <label className="settings-form__label" htmlFor={id}>
        {label}
      </label>
      <select
        id={id}
        className="nexus-select"
        value={value}
        onChange={(e) => onChange(e.target.value as T)}
        disabled={disabled}
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </div>
  );
}

/** Presentational. Mirrors ProjectForm and RegistryForm: no command calls. */
export function SettingsForm({
  values: initial,
  ides,
  agents,
  voices,
  onSubmit,
  onCancel,
  submitting,
}: SettingsFormProps) {
  const [values, setValues] = useState<Settings>(initial);

  function setField<K extends keyof Settings>(field: K, value: Settings[K]) {
    setValues((prev) => ({ ...prev, [field]: value }));
  }

  function toggleStatus(status: TaskStatus) {
    setValues((prev) => ({
      ...prev,
      taskStatusFilter: prev.taskStatusFilter.includes(status)
        ? prev.taskStatusFilter.filter((s) => s !== status)
        : [...prev.taskStatusFilter, status],
    }));
  }

  // The stored preference may be a voice *name* (the shipped default is
  // "Tara") or a system identifier chosen from this list. Match either, so a
  // name-based preference selects the right row instead of appearing unset.
  const matchedVoice = voices.find(
    (v) =>
      v.id === values.voiceName ||
      v.name.toLowerCase() === values.voiceName.trim().toLowerCase(),
  );
  const selectedVoiceValue = matchedVoice ? matchedVoice.id : values.voiceName;
  const missingVoice =
    !matchedVoice && values.voiceName.trim().length > 0
      ? values.voiceName.trim()
      : null;
  // What the synthesizer's fallback chain will actually reach for. Shown so an
  // unavailable preference reads as "substituted" rather than "broken": the
  // set of voices AVSpeechSynthesis exposes is smaller than the set macOS
  // lists elsewhere, so a perfectly reasonable choice can be absent here.
  const fallbackVoice = missingVoice
    ? (voices.find((v) => v.preferredLocale) ?? voices[0] ?? null)
    : null;

  // Grouped from what the system reports, never from a fixed list of names.
  // The recognizer's own locale leads; female voices come next because on
  // some Macs, including this one, no en-IN voice is female and hunting for
  // one through an alphabetical list is miserable.
  const localeVoices = voices.filter((v) => v.preferredLocale);
  const femaleVoices = voices.filter(
    (v) => !v.preferredLocale && v.gender === 'female',
  );
  const otherVoices = voices.filter(
    (v) => !v.preferredLocale && v.gender !== 'female',
  );
  const femaleGroupLabel = localeVoices.some((v) => v.gender === 'female')
    ? 'Female voices'
    : 'Female voices (other English locales)';

  function handleSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    if (submitting) return;
    void onSubmit(values);
  }

  return (
    <form className="settings-form" onSubmit={handleSubmit} noValidate>
      <fieldset className="settings-form__group">
        <legend className="settings-form__legend">Startup</legend>
        <div className="settings-form__field">
          <span className="settings-form__label">Launch screen</span>
          <div className="nexus-filter-bar">
            {(['overview', 'projects'] as const).map((screen) => (
              <button
                key={screen}
                className={`nexus-btn ${
                  values.launchScreen === screen
                    ? 'nexus-btn--primary'
                    : 'nexus-btn--secondary'
                }`}
                type="button"
                onClick={() => setField('launchScreen', screen)}
                disabled={submitting}
                aria-pressed={values.launchScreen === screen}
              >
                {screen}
              </button>
            ))}
          </div>
        </div>
      </fieldset>

      <fieldset className="settings-form__group">
        <legend className="settings-form__legend">Default list order</legend>
        <div className="settings-form__row">
          <SortField<ProjectSortMode>
            id="settings-project-sort"
            label="Projects"
            value={values.projectSort}
            options={PROJECT_SORT_OPTIONS}
            onChange={(v) => setField('projectSort', v)}
            disabled={submitting}
          />
          <SortField<TaskSortMode>
            id="settings-task-sort"
            label="Tasks"
            value={values.taskSort}
            options={TASK_SORT_OPTIONS}
            onChange={(v) => setField('taskSort', v)}
            disabled={submitting}
          />
          <SortField<RegistrySortMode>
            id="settings-registry-sort"
            label="Registry"
            value={values.registrySort}
            options={REGISTRY_SORT_OPTIONS}
            onChange={(v) => setField('registrySort', v)}
            disabled={submitting}
          />
        </div>
      </fieldset>

      <fieldset className="settings-form__group">
        <legend className="settings-form__legend">Default task filter</legend>
        <div className="settings-form__field">
          <span className="settings-form__label">
            Statuses shown when a project opens
          </span>
          <div className="nexus-filter-bar">
            {TASK_STATUS_ORDER.map((status) => (
              <StatusPill
                key={status}
                status={status}
                as="button"
                selected={values.taskStatusFilter.includes(status)}
                disabled={submitting}
                onClick={() => toggleStatus(status)}
                ariaPressed={values.taskStatusFilter.includes(status)}
                ariaLabel={`Default filter ${formatStatus(status)}`}
              />
            ))}
          </div>
          <span className="settings-form__hint">
            Select none to show every status.
          </span>
        </div>
      </fieldset>

      <fieldset className="settings-form__group">
        <legend className="settings-form__legend">New project defaults</legend>
        <div className="settings-form__row">
          <div className="settings-form__field">
            <label className="settings-form__label" htmlFor="settings-default-ide">
              Default IDE
            </label>
            <RegistrySelect
              id="settings-default-ide"
              entries={ides}
              value={values.newProjectDefaultIdeId}
              onChange={(id) => setField('newProjectDefaultIdeId', id)}
              disabled={submitting}
            />
          </div>
          <div className="settings-form__field">
            <label className="settings-form__label" htmlFor="settings-default-agent">
              Default Agent
            </label>
            <RegistrySelect
              id="settings-default-agent"
              entries={agents}
              value={values.newProjectDefaultAgentId}
              onChange={(id) => setField('newProjectDefaultAgentId', id)}
              disabled={submitting}
            />
          </div>
        </div>
        <span className="settings-form__hint">
          Applied to newly created projects only. Existing projects are not
          changed, and the values remain overridable in the create form.
        </span>
      </fieldset>

      <fieldset className="settings-form__group">
        <legend className="settings-form__legend">Voice</legend>
        <label className="settings-form__toggle">
          <input
            type="checkbox"
            checked={values.voiceEnabled}
            onChange={(e) => setField('voiceEnabled', e.target.checked)}
            disabled={submitting}
          />
          <span className="settings-form__label">
            Enable on-device voice commands
          </span>
        </label>
        <span className="settings-form__hint">
          Off by default. The microphone never starts while this is off.
          Recognition runs entirely on this Mac; audio and transcripts are
          never stored or sent anywhere. Voice can only run commands the
          keyboard palette already offers, and nothing executes without your
          confirmation.
        </span>

        <label className="settings-form__toggle">
          <input
            type="checkbox"
            checked={values.alwaysListening}
            onChange={(e) => setField('alwaysListening', e.target.checked)}
            disabled={submitting || !values.voiceEnabled}
          />
          <span className="settings-form__label">
            Always listening for &ldquo;Nexus&rdquo;
          </span>
        </label>
        <span className="settings-form__hint">
          Keeps the microphone open so you can call NEXUS by name instead of
          pressing the button. Only sentences that start with
          &ldquo;Nexus&rdquo; are acted on; everything else it hears is
          discarded without being matched or shown. Recognition stays on this
          Mac, and still nothing is stored. Expect noticeably more battery use
          on an unplugged laptop.
        </span>

        <div className="settings-form__field">
          <label className="settings-form__label" htmlFor="settings-wake-replies">
            Replies when called
          </label>
          <textarea
            id="settings-wake-replies"
            className="nexus-input"
            rows={3}
            value={values.wakeReplies.join('\n')}
            onChange={(e) =>
              setField('wakeReplies', e.target.value.split('\n'))
            }
            disabled={submitting || !values.voiceEnabled || !values.alwaysListening}
          />
          <span className="settings-form__hint">
            One per line, spoken in rotation. Blank lines are ignored; clearing
            the field restores the defaults.
          </span>
        </div>

        <div className="settings-form__field">
          <label className="settings-form__label" htmlFor="settings-voice-name">
            Response voice
          </label>
          <select
            id="settings-voice-name"
            className="nexus-select"
            value={selectedVoiceValue}
            onChange={(e) => setField('voiceName', e.target.value)}
            disabled={submitting || !values.voiceEnabled}
          >
            {localeVoices.length > 0 && (
              <optgroup label="English (India)">
                {localeVoices.map((voice) => (
                  <option key={voice.id} value={voice.id}>
                    {voiceLabel(voice)}
                  </option>
                ))}
              </optgroup>
            )}
            {femaleVoices.length > 0 && (
              <optgroup label={femaleGroupLabel}>
                {femaleVoices.map((voice) => (
                  <option key={voice.id} value={voice.id}>
                    {voiceLabel(voice)}
                  </option>
                ))}
              </optgroup>
            )}
            {otherVoices.length > 0 && (
              <optgroup label="Other voices">
                {otherVoices.map((voice) => (
                  <option key={voice.id} value={voice.id}>
                    {voiceLabel(voice)}
                  </option>
                ))}
              </optgroup>
            )}
            {/*
              A saved preference that no longer resolves still renders, rather
              than the browser silently selecting the first option and making
              it look as though the user chose it. Mirrors how RegistrySelect
              keeps a disabled default visible.
            */}
            {missingVoice && (
              <option value={missingVoice}>
                {missingVoice} (not available on this Mac)
              </option>
            )}
            <option value="">System default</option>
          </select>
        </div>
        <span className="settings-form__hint">
          {missingVoice
            ? `${missingVoice} is not available to NEXUS on this Mac, so responses use ${
                fallbackVoice ? fallbackVoice.name : 'the system default voice'
              } instead. Pick a voice above to choose deliberately.`
            : 'Only voices macOS exposes to app speech are listed, which is a smaller set than the Dictation voice list. More can be installed in System Settings, under Accessibility then Spoken Content.'}{' '}
          Spoken with a voice installed on this Mac; nothing is generated
          online. NEXUS speaks only after a command runs, or after a phrase
          definitively matches nothing, and never while it is listening.
        </span>
      </fieldset>

      <div className="settings-form__actions">
        <button
          className="nexus-btn nexus-btn--secondary"
          type="button"
          onClick={onCancel}
          disabled={submitting}
        >
          Revert
        </button>
        <button
          className="nexus-btn nexus-btn--primary"
          type="submit"
          disabled={submitting}
        >
          {submitting ? 'Saving...' : 'Save Settings'}
        </button>
      </div>
    </form>
  );
}

/**
 * e.g. "Tara - en-IN (enhanced)". Quality is shown only when it is not the
 * default, because that is the part worth choosing between.
 */
function voiceLabel(voice: VoiceOption): string {
  const quality = voice.quality === 'default' ? '' : ` (${voice.quality})`;
  return `${voice.name} - ${languageLabel(voice.language)}${quality}`;
}

/** "en-IN" reads as a locale code; "English (India)" reads as a voice. */
function languageLabel(language: string): string {
  const named = new Intl.DisplayNames(['en'], { type: 'language' });
  try {
    return named.of(language) ?? language;
  } catch {
    return language;
  }
}
