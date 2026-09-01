import { useCallback, useEffect, useState } from 'react';
import { ArrowRight, Sparkles } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import './NexusSays.css';

interface Suggestion {
  key: string;
  title: string;
  priority: string;
  acceptLabel: string;
  actionId: string;
  actionInput: unknown;
}

interface Props {
  onOpenAssistant: () => void;
}

const REFRESH_MS = 30000;

/**
 * The one thing NEXUS wants to say, and how many more it is holding back.
 *
 * Suggestions are generated from data NEXUS already has (blocked tasks,
 * things that have not moved, projects with no repository), never inferred
 * from conversation. Showing all of them at once is how a suggestion panel
 * becomes wallpaper, so this leads with one and counts the rest.
 *
 * Nothing here executes. Accepting opens the assistant with the request
 * already phrased, so the action still goes through the gate and is still
 * confirmed if it writes.
 */
export function NexusSays({ onOpenAssistant }: Props) {
  const [items, setItems] = useState<Suggestion[]>([]);

  const load = useCallback(async () => {
    try {
      setItems(await invoke<Suggestion[]>('nexus_list_suggestions'));
    } catch {
      // A quiet panel is the correct failure: suggestions are an offer, and
      // an error where an offer would be is worse than no offer.
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const lead = items[0];
  const more = Math.max(0, items.length - 1);

  return (
    <section className="says" aria-label="What NEXUS suggests">
      <div className="says__head">
        <Sparkles size={12} strokeWidth={2} aria-hidden="true" />
        <span>NEXUS says</span>
      </div>

      {lead ? (
        <>
          <p className="says__lead">{lead.title}</p>
          <button className="says__cta" type="button" onClick={onOpenAssistant}>
            {lead.acceptLabel}
            <ArrowRight size={13} strokeWidth={2} aria-hidden="true" />
          </button>
          {more > 0 && (
            <span className="says__more">
              and {more} other {more === 1 ? 'thing' : 'things'} worth a look
            </span>
          )}
        </>
      ) : (
        <p className="says__lead says__lead--quiet">
          Nothing needs you right now.
        </p>
      )}

      {/* The pulse is the assistant's own presence, not a loading state. It
          sits here because this is the panel that speaks for it. */}
      <span className="says__pulse" aria-hidden="true">
        <span className="says__pulse-core" />
      </span>
    </section>
  );
}
