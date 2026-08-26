import { useState } from 'react';
import { Mic } from 'lucide-react';
import type { CommandState } from '../../types';
import './CommandBar.css';

export function CommandBar() {
  const [state, setState] = useState<CommandState>({
    value: '',
    isListening: false, // always false in NEXUS-001
  });

  function handleChange(e: React.ChangeEvent<HTMLInputElement>) {
    setState((prev) => ({ ...prev, value: e.target.value }));
  }

  // Mic button is UI-only — no speech recognition in NEXUS-001
  function handleMicClick() {
    // No-op: voice recognition is out of scope for NEXUS-001
  }

  return (
    <div className="command-bar" role="search" aria-label="NEXUS command input">
      <div className="command-bar__input-wrapper">
        <span className="command-bar__prefix" aria-hidden="true">&gt;</span>
        <input
          className="command-bar__input"
          type="text"
          placeholder="Ask Nexus anything..."
          value={state.value}
          onChange={handleChange}
          aria-label="Command input"
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          spellCheck={false}
        />
      </div>

      <button
        className="command-bar__mic-btn"
        onClick={handleMicClick}
        aria-label="Voice input (not available)"
        title="Voice input — coming soon"
        type="button"
      >
        <Mic size={18} strokeWidth={2} />
      </button>
    </div>
  );
}
