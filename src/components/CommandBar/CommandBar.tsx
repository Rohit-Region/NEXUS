import { Mic, Sparkles } from 'lucide-react';
import './CommandBar.css';

interface CommandBarProps {
  /** NEXUS-009: the input is an affordance that opens the palette overlay. */
  onOpenPalette: () => void;
  /** NEXUS-010: mirrors settings.voiceEnabled. */
  voiceEnabled: boolean;
  /** NEXUS-014: opens the conversation surface. */
  onOpenAssistant: () => void;
  assistantOpen: boolean;
  voiceActive: boolean;
  onVoiceToggle: () => void;
}

export function CommandBar({
  onOpenPalette,
  voiceEnabled,
  onOpenAssistant,
  assistantOpen,
  voiceActive,
  onVoiceToggle,
}: CommandBarProps) {


  return (
    <div className="command-bar" role="search" aria-label="NEXUS command input">
      {/*
        The palette restores focus to whatever held it on open. If this input
        took focus, that restore would re-fire and reopen the palette, making
        Escape impossible. It opens on mousedown with preventDefault so focus
        never lands here; Tab users get Enter and Space instead.
      */}
      <div className="command-bar__input-wrapper">
        <span className="command-bar__prefix" aria-hidden="true">&gt;</span>
        <input
          className="command-bar__input"
          type="text"
          placeholder="Search or run a command  ⌘K"
          value=""
          readOnly
          onMouseDown={(e) => {
            e.preventDefault();
            onOpenPalette();
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              onOpenPalette();
            }
          }}
          aria-label="Open the command palette"
          aria-keyshortcuts="Meta+K"
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          spellCheck={false}
        />
      </div>

      <button
        className={`command-bar__mic-btn${assistantOpen ? ' command-bar__mic-btn--active' : ''}`}
        onClick={onOpenAssistant}
        aria-pressed={assistantOpen}
        aria-label="Open the assistant"
        title="Ask NEXUS"
        type="button"
      >
        <Sparkles size={18} strokeWidth={2} />
      </button>

      <button
        className={`command-bar__mic-btn${voiceActive ? ' command-bar__mic-btn--active' : ''}`}
        onClick={onVoiceToggle}
        disabled={!voiceEnabled}
        aria-pressed={voiceActive}
        aria-label={
          voiceEnabled
            ? voiceActive
              ? 'Stop listening'
              : 'Start voice command'
            : 'Voice input (disabled in Settings)'
        }
        title={
          voiceEnabled
            ? voiceActive
              ? 'Stop listening'
              : 'Voice command (on-device)'
            : 'Voice is disabled in Settings'
        }
        type="button"
      >
        <Mic size={18} strokeWidth={2} />
      </button>
    </div>
  );
}
