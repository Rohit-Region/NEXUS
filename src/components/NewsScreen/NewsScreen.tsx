import { useCallback, useEffect, useMemo, useState } from 'react';
import { ExternalLink, Newspaper, Plus, RefreshCw, Search, Settings2, Sparkles, X } from 'lucide-react';
import {
  newsDesk,
  newsSettings,
  openUrl,
  setNewsSettings,
  type NewsDesk,
  type NewsSettings,
} from '../../lib/nexus-db';
import './NewsScreen.css';

/** "5 min ago", "3 h ago", "2 d ago". */
function ago(unix: number | null, now: number): string {
  if (unix === null) return '';
  const s = Math.max(0, now - unix);
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

const FOR_YOU = 'For you';
const ALL = 'All';
/** The list grows as you scroll down, a page at a time. */
const PAGE = 40;

/**
 * News: Indian, business and markets, world and tech headlines from public
 * feeds, newest first. Stories mentioning your interests are marked and
 * gathered under "For you". Sources and interests are yours to change.
 */
export function NewsScreen() {
  const [desk, setDesk] = useState<NewsDesk | null>(null);
  const [settings, setSettings] = useState<NewsSettings | null>(null);
  const [topic, setTopic] = useState(ALL);
  const [query, setQuery] = useState('');
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [showSources, setShowSources] = useState(false);
  const [interest, setInterest] = useState('');
  const [feed, setFeed] = useState({ label: '', url: '', topic: 'Tech' });
  const [shown, setShown] = useState(PAGE);

  const load = useCallback(async (refresh: boolean) => {
    setBusy(true);
    try {
      const [d, s] = await Promise.all([newsDesk(refresh), newsSettings()]);
      setDesk(d);
      setSettings(s);
    } catch (err) {
      setStatus(`Could not load the news: ${String(err).replace(/^Error:\s*/, '')}`);
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load(false);
  }, [load]);

  const save = useCallback(
    async (next: NewsSettings, note?: string) => {
      try {
        setSettings(await setNewsSettings(next));
        setStatus(note ?? null);
        await load(false);
      } catch (err) {
        setStatus(String(err).replace(/^Error:\s*/, ''));
      }
    },
    [load],
  );

  const items = useMemo(() => {
    if (!desk) return [];
    const q = query.trim().toLowerCase();
    return desk.items.filter(
      (i) =>
        (topic === ALL || (topic === FOR_YOU ? i.matches.length > 0 : i.topic === topic)) &&
        (!q || i.title.toLowerCase().includes(q) || (i.summary ?? '').toLowerCase().includes(q)),
    );
  }, [desk, topic, query]);

  useEffect(() => setShown(PAGE), [topic, query]);

  if (!desk || !settings) {
    return (
      <div className="news">
        <p className="news__note">{status ?? 'Reading the news…'}</p>
      </div>
    );
  }

  const now = Math.floor(Date.now() / 1000);
  const forYouCount = desk.items.filter((i) => i.matches.length > 0).length;
  const tabs = [FOR_YOU, ALL, ...desk.topics];

  const toggleSource = (id: string, on: boolean, defaultOn: boolean) => {
    const enabled = settings.enabled.filter((x) => x !== id);
    const disabled = settings.disabled.filter((x) => x !== id);
    if (on && !defaultOn) enabled.push(id);
    if (!on && defaultOn) disabled.push(id);
    void save({ ...settings, enabled, disabled });
  };

  const addInterest = () => {
    const word = interest.trim();
    if (word.length < 2) return;
    setInterest('');
    void save({ ...settings, interests: [...settings.interests, word] });
  };

  const addFeed = () => {
    if (!feed.label.trim() || !feed.url.trim()) {
      setStatus('Give the feed a name and its https:// address.');
      return;
    }
    setStatus('Checking the feed…');
    void save({ ...settings, custom: [...settings.custom, { ...feed, url: feed.url.trim() }] }, `Added ${feed.label}.`).then(
      () => setFeed({ label: '', url: '', topic: feed.topic }),
    );
  };

  return (
    <div className="news">
      <header className="news__head">
        <Newspaper size={18} strokeWidth={1.8} aria-hidden="true" />
        <div className="news__headings">
          <h1 className="news__title">News</h1>
          <p className="news__subtitle">
            {desk.items.length} headlines from {desk.sources.filter((s) => s.on).length} sources · updated{' '}
            {ago(desk.fetchedAt, now)}
          </p>
        </div>
        <button
          className="news__icon"
          type="button"
          onClick={() => setShowSources((s) => !s)}
          aria-expanded={showSources}
          aria-label="Sources and interests"
          title="Sources and interests"
        >
          <Settings2 size={14} strokeWidth={2} aria-hidden="true" />
        </button>
        <button
          className={`news__icon${busy ? ' news__icon--busy' : ''}`}
          type="button"
          onClick={() => void load(true)}
          disabled={busy}
          aria-label="Refresh"
          title="Refresh"
        >
          <RefreshCw size={14} strokeWidth={2} aria-hidden="true" />
        </button>
      </header>

      {status && <p className="news__status">{status}</p>}

      {showSources && (
        <section className="news-panel" aria-label="Sources and interests">
          <div className="news-panel__block">
            <h2 className="news-panel__title">Your interests</h2>
            <p className="news-panel__hint">Stories that mention these are marked and gathered under "For you".</p>
            <div className="news-chips">
              {settings.interests.map((w) => (
                <span className="news-chip" key={w}>
                  {w}
                  <button
                    type="button"
                    onClick={() => void save({ ...settings, interests: settings.interests.filter((x) => x !== w) })}
                    aria-label={`Remove ${w}`}
                  >
                    <X size={10} strokeWidth={2.5} aria-hidden="true" />
                  </button>
                </span>
              ))}
              <input
                className="news-input news-input--short"
                placeholder="e.g. Sensex, gold, React"
                value={interest}
                onChange={(e) => setInterest(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && addInterest()}
                aria-label="Add an interest"
              />
              <button className="news-btn" type="button" onClick={addInterest}>
                Add
              </button>
            </div>
          </div>

          <div className="news-panel__block">
            <h2 className="news-panel__title">Sources</h2>
            <ul className="news-sources">
              {desk.sources.map((s) => {
                return (
                  <li key={s.id} className="news-sources__row">
                    <label>
                      <input
                        type="checkbox"
                        checked={s.on}
                        disabled={s.custom}
                        onChange={(e) => toggleSource(s.id, e.target.checked, s.defaultOn)}
                      />
                      <span>{s.label}</span>
                      <span className="news-sources__topic">{s.topic}</span>
                    </label>
                    {s.error && s.on && <span className="news-sources__err" title={s.error}>not responding</span>}
                    {s.custom && (
                      <button
                        className="news-link"
                        type="button"
                        onClick={() =>
                          void save({ ...settings, custom: settings.custom.filter((c) => c.label !== s.label) })
                        }
                      >
                        Remove
                      </button>
                    )}
                  </li>
                );
              })}
            </ul>
            <div className="news-addfeed">
              <input
                className="news-input"
                placeholder="Name"
                value={feed.label}
                onChange={(e) => setFeed({ ...feed, label: e.target.value })}
                aria-label="Feed name"
              />
              <input
                className="news-input news-input--wide"
                placeholder="https://… RSS or Atom feed address"
                value={feed.url}
                onChange={(e) => setFeed({ ...feed, url: e.target.value })}
                aria-label="Feed address"
              />
              <select
                className="news-input"
                value={feed.topic}
                onChange={(e) => setFeed({ ...feed, topic: e.target.value })}
                aria-label="Feed topic"
              >
                {desk.topics.map((t) => (
                  <option key={t} value={t}>
                    {t}
                  </option>
                ))}
              </select>
              <button className="news-btn" type="button" onClick={addFeed}>
                <Plus size={12} strokeWidth={2.5} aria-hidden="true" /> Add feed
              </button>
            </div>
          </div>
        </section>
      )}

      <div className="news-tabs" role="group" aria-label="Topics">
        {tabs.map((t) => (
          <button
            key={t}
            type="button"
            className={`news-tab${topic === t ? ' news-tab--on' : ''}`}
            aria-pressed={topic === t}
            onClick={() => setTopic(t)}
          >
            {t === FOR_YOU && <Sparkles size={11} strokeWidth={2} aria-hidden="true" />}
            {t}
            {t === FOR_YOU && forYouCount > 0 && <span className="news-tab__count">{forYouCount}</span>}
          </button>
        ))}
        <label className="news-search">
          <Search size={12} strokeWidth={2} aria-hidden="true" />
          <input
            type="search"
            placeholder="Search headlines"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            aria-label="Search headlines"
          />
        </label>
      </div>

      {topic === FOR_YOU && settings.interests.length === 0 && (
        <p className="news__note">
          Add a few interests (the ⚙ button above), such as "Sensex", "gold" or "React", and matching stories collect
          here.
        </p>
      )}
      {items.length === 0 && !(topic === FOR_YOU && settings.interests.length === 0) && (
        <p className="news__note">Nothing here right now.</p>
      )}

      <ol className="news-list">
        {items.slice(0, shown).map((i) => (
          <li key={i.link} className={`news-item${i.matches.length ? ' news-item--match' : ''}`}>
            <button className="news-item__main" type="button" onClick={() => void openUrl(i.link)} title="Open in your browser">
              <span className="news-item__meta">
                <span className="news-item__source">{i.source}</span>
                <span>{i.topic}</span>
                {i.published !== null && <span>{ago(i.published, now)}</span>}
                {i.matches.map((m) => (
                  <span className="news-item__match" key={m}>
                    {m}
                  </span>
                ))}
              </span>
              <span className="news-item__title">
                {i.title}
                <ExternalLink size={11} strokeWidth={2} aria-hidden="true" className="news-item__ext" />
              </span>
              {i.summary && <span className="news-item__summary">{i.summary}</span>}
            </button>
          </li>
        ))}
      </ol>
      {items.length > shown && (
        <button className="news-more" type="button" onClick={() => setShown((n) => n + PAGE)}>
          Show more ({items.length - shown} left)
        </button>
      )}
    </div>
  );
}
