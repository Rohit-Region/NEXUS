import { useCallback, useEffect, useState } from 'react';
import { Sparkles, RefreshCw, ExternalLink } from 'lucide-react';
import {
  fetchNews,
  newsBriefing,
  openUrl,
  today as loadToday,
  wellbeingStatus,
  type NewsItem,
  type TodaySummary,
  type WellbeingStatus,
} from '../../lib/nexus-db';
import type { NexusView } from '../../types';
import './Briefing.css';

/** Your day refreshes more often than news: a reminder can fall due any time. */
const TODAY_REFRESH_MS = 300_000;

interface Props {
  /** For the day's items to open the section they belong to. */
  navigate: (view: NexusView) => void;
}

/** News changes over minutes, not seconds, and the model call is not free, so
 *  this refreshes slowly. The manual button is there for "now". */
const REFRESH_MS = 900_000;

/** How many headlines the feed shows. */
const FEED_LIMIT = 10;

/** A short "2h" style age from whatever date a feed wrote. Feeds disagree on
 *  format; Date.parse handles both RFC822 (pubDate) and ISO (updated). An
 *  unparseable or missing date just shows nothing rather than "NaN". */
function agoOf(published: string | null): string {
  if (!published) return '';
  const then = Date.parse(published);
  if (Number.isNaN(then)) return '';
  const mins = Math.max(0, Math.floor((Date.now() - then) / 60000));
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

/**
 * The hero: what matters to you today, not what your connectors did.
 *
 * This took over the two slots that used to hold "NEXUS says" (dev-workflow
 * nudges) and "Live activity" (a connector audit log). Neither was worth the
 * top of the screen. In their place: real news from public feeds, with a
 * one-line heads-up the local model writes from the headlines.
 *
 * The feed loads first and fast; the model-written lead can take a few seconds
 * on a local model, so the headlines are shown immediately and the lead settles
 * in when it is ready. A model that is missing or busy is not an error: the
 * lead falls back to the top headline, and the news still shows.
 *
 * This is the first pillar of the assistant. News is the one that needs no
 * personal data; learning, career and wealth land in the same hero as their
 * own feeds are built.
 */
export function Briefing({ navigate }: Props) {
  const [items, setItems] = useState<NewsItem[]>([]);
  const [headline, setHeadline] = useState<string | null>(null);
  const [generated, setGenerated] = useState(false);
  const [busy, setBusy] = useState(false);
  const [everLoaded, setEverLoaded] = useState(false);
  const [day, setDay] = useState<TodaySummary | null>(null);
  const [well, setWell] = useState<WellbeingStatus | null>(null);

  // The break countdown moves by the minute, so it is polled on its own,
  // cheaply: it reads an in-memory counter, not the calendar or the network.
  useEffect(() => {
    const read = () =>
      void wellbeingStatus()
        .then(setWell)
        .catch(() => {});
    read();
    const timer = window.setInterval(read, 60_000);
    return () => window.clearInterval(timer);
  }, []);

  const loadDay = useCallback(async () => {
    try {
      setDay(await loadToday());
    } catch {
      // Keep the last good day rather than blanking the headline.
    }
  }, []);

  useEffect(() => {
    void loadDay();
    const timer = window.setInterval(() => void loadDay(), TODAY_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [loadDay]);

  const load = useCallback(async () => {
    setBusy(true);
    try {
      // Fast path first: show the headlines without waiting on the model.
      try {
        const quick = await fetchNews(FEED_LIMIT);
        if (quick.length > 0) setItems(quick);
      } catch {
        // The briefing call below is the real one; a failed quick fetch just
        // means the feed fills a moment later.
      }
      // Then the model-written lead, which also returns the (same) items.
      const brief = await newsBriefing(FEED_LIMIT);
      setItems(brief.items);
      setHeadline(brief.headline);
      setGenerated(brief.generated);
    } catch {
      // Leave whatever is already shown; a transient feed error should not
      // blank the hero.
    } finally {
      setBusy(false);
      setEverLoaded(true);
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const open = (url: string) => void openUrl(url).catch(() => {});

  return (
    <section className="brief" aria-label="Your briefing">
      <div className="brief__head">
        <Sparkles size={12} strokeWidth={2} aria-hidden="true" />
        <span className="brief__kicker">NEXUS says</span>
        <button
          className={`brief__refresh${busy ? ' brief__refresh--busy' : ''}`}
          type="button"
          onClick={() => {
            void load();
            void loadDay();
          }}
          disabled={busy}
          aria-label="Refresh briefing"
        >
          <RefreshCw size={12} strokeWidth={2} aria-hidden="true" />
        </button>
      </div>

      {/* Your day leads. Assembled from your own data, not generated, so it
          can't invent a meeting or a bill. */}
      <p className="brief__lead">{day ? day.line : 'Getting your day ready…'}</p>

      {day && (
        <div className="brief__day">
          {day.billsDue.length > 0 && (
            <button className="brief__chip" type="button" onClick={() => navigate({ screen: 'money' })}>
              {day.billsDue.length} bill{day.billsDue.length === 1 ? '' : 's'} this week
            </button>
          )}
          {day.taxSoon && (
            <button className="brief__chip" type="button" onClick={() => navigate({ screen: 'money' })}>
              {day.taxSoon.name}: {day.taxSoon.daysUntil}d
            </button>
          )}
          {day.followUpsDue > 0 && (
            <button className="brief__chip" type="button" onClick={() => navigate({ screen: 'career' })}>
              {day.followUpsDue} follow-up{day.followUpsDue === 1 ? '' : 's'}
            </button>
          )}
          {day.interviewing > 0 && (
            <button className="brief__chip" type="button" onClick={() => navigate({ screen: 'career' })}>
              {day.interviewing} interviewing
            </button>
          )}
          {day.cardsDue > 0 && (
            <button className="brief__chip" type="button" onClick={() => navigate({ screen: 'learn' })}>
              {day.cardsDue} card{day.cardsDue === 1 ? '' : 's'} to review
            </button>
          )}
          {day.streak > 0 && (
            <button className="brief__chip brief__chip--quiet" type="button" onClick={() => navigate({ screen: 'learn' })}>
              {day.streak}-day study streak
            </button>
          )}
          {well?.enabled && well.activeMinutes > 0 && (
            <button
              className="brief__chip brief__chip--quiet"
              type="button"
              onClick={() => navigate({ screen: 'me' })}
              title="Time at the keyboard since your last real break. Settings are in Me."
            >
              Active {well.activeMinutes}m
              {well.nextBreakIn !== null && ` · break in ${well.nextBreakIn}m`}
            </button>
          )}
          {day.calendar === null && (
            <span className="brief__chip brief__chip--quiet" title="Calendar access is off, so meetings aren't counted.">
              calendar not connected
            </span>
          )}
        </div>
      )}

      <div className="brief__feed-head">
        <span>Today’s news</span>
        {items.length > 0 && <span className="brief__count">{items.length}</span>}
      </div>
      {headline && (
        <p className="brief__news-line">
          {headline}
          {generated && (
            <span className="brief__by" title="Written by your local model from today’s headlines">
              {' '}· composed by NEXUS
            </span>
          )}
        </p>
      )}

      <ul className="brief__feed">
        {items.length === 0 && everLoaded && (
          <li className="brief__empty">No headlines could be read right now.</li>
        )}
        {items.map((item) => (
          <li className="brief__item" key={item.link}>
            <button className="brief__open" type="button" onClick={() => open(item.link)} title={item.title}>
              <span className="brief__source">{item.source}</span>
              <span className="brief__title">{item.title}</span>
              <span className="brief__ago">{agoOf(item.published)}</span>
              <ExternalLink size={11} strokeWidth={2} aria-hidden="true" />
            </button>
          </li>
        ))}
      </ul>

      {/* The assistant's own presence, not a loading state. */}
      <span className="brief__pulse" aria-hidden="true">
        <span className="brief__pulse-core" />
      </span>
    </section>
  );
}
