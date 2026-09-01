import { useCallback, useEffect, useState } from 'react';
import { Logo } from '../Logo/Logo';
import { StatusBar } from '../StatusBar/StatusBar';
import { Dashboard } from '../Dashboard/Dashboard';
import { CommandBar } from '../CommandBar/CommandBar';
import { CommandPalette } from '../CommandPalette/CommandPalette';
import { AssistantPanel } from '../AssistantPanel/AssistantPanel';
import { VoiceController } from '../VoiceController/VoiceController';
import type { NexusView } from '../../types';
import { Sidebar } from '../Sidebar/Sidebar';
import type { Settings } from '../../types/db';
import './AppShell.css';

interface AppShellProps {
  /** Supplied by App.tsx. AppShell never loads settings itself (spec 008 N-07). */
  settings: Settings;
  onSettingsChange: (next: Settings) => void;
  settingsError?: string | null;
}

/**
 * Thin orchestrator: owns view state only.
 * All project data and database access live in the project components.
 */
export function AppShell({
  settings,
  onSettingsChange,
  settingsError,
}: AppShellProps) {
  // Seeded once on mount: changing the preference later must not navigate the
  // user away mid-session.
  const [view, setView] = useState<NexusView>({ screen: settings.launchScreen });
  const [errorDismissed, setErrorDismissed] = useState(false);
  // View state only: the palette fetches its own data, so AppShell still
  // imports nothing from src/lib/nexus-db.ts.
  const [paletteOpen, setPaletteOpen] = useState(false);
  // Bumped on every navigation. Destinations consume `intent` as mount state,
  // so an intent aimed at the screen already showing would otherwise never be
  // seen. Dashboard folds this into the key only when an intent is present,
  // leaving ordinary navigation free of spurious remounts.
  const [navSeq, setNavSeq] = useState(0);
  // NEXUS-010. AppShell holds plain state only; VoiceController owns the IPC,
  // so the no-nexus-db rule still holds here.
  const [voiceActive, setVoiceActive] = useState(false);
  // Bumped whenever the assistant asks the user something, so the voice
  // controller can open the microphone for the answer. A counter, because
  // two questions in a row must each reopen it.
  const [expectAnswer, setExpectAnswer] = useState(0);
  // NEXUS-014. The conversation surface. Still plain state: VoiceController
  // and AssistantPanel own their own IPC, so the no-nexus-db rule holds here.
  const [assistantOpen, setAssistantOpen] = useState(false);
  const [spokenText, setSpokenText] = useState<string | null>(null);


  /**
   * A final transcript now goes to the assistant rather than the palette.
   *
   * NEXUS-010 routed it to the palette because the palette was where
   * confirmation lived. Since NEXUS-012 confirmation is enforced in Rust, so
   * the invariant no longer depends on which surface receives the text, and
   * one conversation is better than two. The palette keeps working from
   * Command-K; its voice props are simply no longer used from here.
   */
  const handleFinalTranscript = useCallback((text: string) => {
    setSpokenText(text);
    setAssistantOpen(true);
  }, []);

  // Command-K only. Control-K is the Emacs kill-line binding that macOS text
  // inputs honour, and NEXUS is macOS-first.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.metaKey && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setPaletteOpen((prev) => !prev);
      }
    }
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);
  const [activeProjectName, setActiveProjectName] = useState<string | null>(null);

  const navigate = useCallback((next: NexusView) => {
    if (next.screen !== 'project-detail') {
      setActiveProjectName(null);
    }
    setNavSeq((n) => n + 1);
    setView(next);
  }, []);

  const handleActiveProjectChange = useCallback((name: string | null) => {
    setActiveProjectName(name);
  }, []);

  const onProjects =
    view.screen === 'projects' || view.screen === 'project-detail';
  const showBadge = view.screen === 'project-detail' && activeProjectName !== null;

  return (
    <div className="app-shell app-shell--railed">
      {/* The permanent rail. Grouped navigation plus live connector dots,
          which are the only always-visible answer to "is Jira working". */}
      <Sidebar
        view={view}
        navigate={navigate}
        onOpenAssistant={() => setAssistantOpen(true)}
        suggestionCount={0}
      />

      <div className="app-shell__column">
      <header className="app-shell__header">
        <Logo />

        <nav className="app-shell__nav" aria-label="Primary">
          <button
            className="app-shell__nav-btn"
            type="button"
            onClick={() => navigate({ screen: 'overview' })}
            aria-current={view.screen === 'overview' ? 'page' : undefined}
          >
            Overview
          </button>
          <button
            className="app-shell__nav-btn"
            type="button"
            onClick={() => navigate({ screen: 'projects' })}
            aria-current={onProjects ? 'page' : undefined}
          >
            Projects
          </button>
          <button
            className="app-shell__nav-btn"
            type="button"
            onClick={() => navigate({ screen: 'registry' })}
            aria-current={view.screen === 'registry' ? 'page' : undefined}
          >
            Registry
          </button>
          <button
            className="app-shell__nav-btn"
            type="button"
            onClick={() => navigate({ screen: 'settings' })}
            aria-current={view.screen === 'settings' ? 'page' : undefined}
          >
            Settings
          </button>
        </nav>

        {showBadge && (
          <span className="app-shell__project-badge" title={activeProjectName ?? undefined}>
            <span className="app-shell__project-badge-label">Project</span>
            {activeProjectName}
          </span>
        )}

        <StatusBar />
      </header>

      {settingsError && !errorDismissed && (
        <div className="app-shell__notice" role="status">
          <span>
            Settings could not be loaded, so defaults are in use. Everything
            else works normally.
          </span>
          <button
            className="nexus-btn nexus-btn--secondary"
            type="button"
            onClick={() => setErrorDismissed(true)}
          >
            Dismiss
          </button>
        </div>
      )}

      <Dashboard
        view={view}
        navigate={navigate}
        onActiveProjectChange={handleActiveProjectChange}
        settings={settings}
        onSettingsChange={onSettingsChange}
        navSeq={navSeq}
      />

      <CommandBar
        onOpenPalette={() => setPaletteOpen(true)}
        voiceEnabled={settings.voiceEnabled}
        voiceActive={voiceActive}
        onVoiceToggle={() => setVoiceActive((prev) => !prev)}
        assistantOpen={assistantOpen}
        onOpenAssistant={() => setAssistantOpen((prev) => !prev)}
      />
      </div>

      <AssistantPanel
        open={assistantOpen}
        onClose={() => setAssistantOpen(false)}
        onOpen={() => setAssistantOpen(true)}
        navigate={navigate}
        voiceEnabled={settings.voiceEnabled}
        voiceActive={voiceActive}
        onVoiceToggle={() => setVoiceActive((prev) => !prev)}
        spokenText={spokenText}
        onSpokenConsumed={() => setSpokenText(null)}
        onExpectAnswer={() => setExpectAnswer((n) => n + 1)}
      />

      <VoiceController
        enabled={settings.voiceEnabled}
        alwaysListening={settings.alwaysListening}
        expectAnswer={expectAnswer}
        active={voiceActive}
        onActiveChange={setVoiceActive}
        onFinalTranscript={handleFinalTranscript}
      />

      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        navigate={navigate}
      />
    </div>
  );
}
