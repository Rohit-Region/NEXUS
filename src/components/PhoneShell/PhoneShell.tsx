import { useCallback, useEffect, useRef, useState } from 'react';
import {
  Bot,
  Briefcase,
  FolderKanban,
  GraduationCap,
  Home,
  LayoutGrid,
  Newspaper,
  Settings as SettingsIcon,
  Sparkles,
  TrendingUp,
  UserRound,
  Wallet,
  X,
} from 'lucide-react';
import { Logo } from '../Logo/Logo';
import { Dashboard } from '../Dashboard/Dashboard';
import { AssistantPanel } from '../AssistantPanel/AssistantPanel';
import type { NexusScreen, NexusView } from '../../types';
import type { Settings } from '../../types/db';
import './PhoneShell.css';

type Icon = typeof Home;

/** The bottom bar: the screens reached most, within a thumb's reach. */
const TABS: { screen: NexusScreen; label: string; Icon: Icon }[] = [
  { screen: 'overview', label: 'Home', Icon: Home },
  { screen: 'money', label: 'Money', Icon: Wallet },
  { screen: 'stocks', label: 'Stocks', Icon: TrendingUp },
];

/** Everything else, behind "More". The Registry of IDEs and agents is left
 *  out: it configures tools that only run on the Mac. */
const MORE: { screen: NexusScreen; label: string; Icon: Icon }[] = [
  { screen: 'me', label: 'Me', Icon: UserRound },
  { screen: 'career', label: 'Career', Icon: Briefcase },
  { screen: 'learn', label: 'Learn', Icon: GraduationCap },
  { screen: 'news', label: 'News', Icon: Newspaper },
  { screen: 'projects', label: 'Projects', Icon: FolderKanban },
  { screen: 'clone', label: 'Clone', Icon: Bot },
  { screen: 'settings', label: 'Settings', Icon: SettingsIcon },
];

/** Away this long and the screen reloads on return, so prices are never
 *  stale. Matches the Money screen's own refresh interval. */
const STALE_AFTER_MS = 300_000;

interface PhoneShellProps {
  settings: Settings;
  onSettingsChange: (next: Settings) => void;
}

/**
 * NEXUS on the phone: the same screens as the Mac through the same
 * `Dashboard`, with the main ones as tabs at the bottom and the rest under
 * "More". The assistant opens full screen. Voice and the command palette stay
 * on the Mac: one uses Apple's speech recognition, the other a keyboard.
 */
export function PhoneShell({ settings, onSettingsChange }: PhoneShellProps) {
  const [view, setView] = useState<NexusView>({ screen: 'overview' });
  const [navSeq, setNavSeq] = useState(0);
  const [moreOpen, setMoreOpen] = useState(false);
  const [assistantOpen, setAssistantOpen] = useState(false);
  const [assistantGreeting, setAssistantGreeting] = useState<string | null>(null);
  // Bumped when the app comes back after a long time away. Part of the
  // screen's key, so the screen remounts and fetches fresh data.
  const [freshness, setFreshness] = useState(0);
  const hiddenAt = useRef<number | null>(null);

  const navigate = useCallback((next: NexusView) => {
    if (next.screen === 'registry') return;
    setMoreOpen(false);
    setAssistantOpen(false);
    setNavSeq((n) => n + 1);
    setView(next);
  }, []);

  const openAssistant = useCallback((greeting?: string) => {
    setAssistantGreeting(greeting ?? null);
    setMoreOpen(false);
    setAssistantOpen(true);
  }, []);

  useEffect(() => {
    function onVisibility() {
      if (document.hidden) {
        hiddenAt.current = Date.now();
      } else if (hiddenAt.current !== null) {
        if (Date.now() - hiddenAt.current >= STALE_AFTER_MS) {
          setFreshness((n) => n + 1);
        }
        hiddenAt.current = null;
      }
    }
    document.addEventListener('visibilitychange', onVisibility);
    return () => document.removeEventListener('visibilitychange', onVisibility);
  }, []);

  const onScreen = (screen: NexusScreen) =>
    !assistantOpen &&
    !moreOpen &&
    (view.screen === screen || (screen === 'projects' && view.screen === 'project-detail'));
  const onMore = !assistantOpen && (moreOpen || MORE.some((m) => onScreen(m.screen)));

  return (
    <div className="phone-shell">
      <header className="phone-shell__header">
        <Logo />
      </header>

      <Dashboard
        key={freshness}
        className="phone-shell__screen"
        view={view}
        navigate={navigate}
        onActiveProjectChange={() => {}}
        settings={settings}
        onSettingsChange={onSettingsChange}
        navSeq={navSeq}
        openConnectors={[]}
        onOpenAssistant={openAssistant}
      />

      {moreOpen && (
        <div className="phone-shell__more" role="dialog" aria-label="More sections">
          <div className="phone-shell__more-head">
            <span>More</span>
            <button type="button" className="phone-shell__close" onClick={() => setMoreOpen(false)} aria-label="Close">
              <X size={20} strokeWidth={1.75} aria-hidden="true" />
            </button>
          </div>
          <ul className="phone-shell__more-list">
            {MORE.map(({ screen, label, Icon }) => (
              <li key={screen}>
                <button
                  type="button"
                  className="phone-shell__more-item"
                  aria-current={view.screen === screen ? 'page' : undefined}
                  onClick={() => navigate({ screen })}
                >
                  <Icon size={20} strokeWidth={1.75} aria-hidden="true" />
                  {label}
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}

      <AssistantPanel
        open={assistantOpen}
        greeting={assistantGreeting}
        onClose={() => {
          setAssistantOpen(false);
          setAssistantGreeting(null);
        }}
        onOpen={() => setAssistantOpen(true)}
        navigate={navigate}
        voiceEnabled={false}
        voiceActive={false}
        onVoiceToggle={() => {}}
        spokenText={null}
        onSpokenConsumed={() => {}}
        onExpectAnswer={() => {}}
      />

      <nav className="phone-shell__tabs" aria-label="Primary">
        {TABS.map(({ screen, label, Icon }) => (
          <button
            key={screen}
            type="button"
            className="phone-shell__tab"
            aria-current={onScreen(screen) ? 'page' : undefined}
            onClick={() => navigate({ screen })}
          >
            <Icon size={20} strokeWidth={1.75} aria-hidden="true" />
            {label}
          </button>
        ))}
        <button
          type="button"
          className="phone-shell__tab"
          aria-current={assistantOpen ? 'page' : undefined}
          onClick={() => (assistantOpen ? setAssistantOpen(false) : openAssistant())}
        >
          <Sparkles size={20} strokeWidth={1.75} aria-hidden="true" />
          Ask
        </button>
        <button
          type="button"
          className="phone-shell__tab"
          aria-current={onMore ? 'page' : undefined}
          aria-expanded={moreOpen}
          onClick={() => {
            setAssistantOpen(false);
            setMoreOpen((open) => !open);
          }}
        >
          <LayoutGrid size={20} strokeWidth={1.75} aria-hidden="true" />
          More
        </button>
      </nav>
    </div>
  );
}
