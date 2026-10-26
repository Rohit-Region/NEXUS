import { useCallback, useEffect, useState } from 'react';
import {
  GraduationCap,
  Plus,
  X,
  Flame,
  RotateCcw,
  BookOpen,
  Layers,
  Check,
} from 'lucide-react';
import {
  addSkill,
  dueCards,
  explainTopic,
  generateCards,
  generatePlan,
  learnStats,
  listSkills,
  removeSkill,
  reviewCard,
  setTopicDone,
  skillTopics,
  type CardView,
  type Grade,
  type LearnStats,
  type SkillSummary,
  type TopicView,
} from '../../lib/nexus-db';
import './LearnScreen.css';

function errText(err: unknown): string {
  return String(err).replace(/^Error:\s*/, '');
}

/**
 * Learn: a tutor that moves you forward a little every day.
 *
 * Pick a skill and the local model drafts a plan of topics, written for you
 * from your Me profile. For any topic it can explain it or write flashcards,
 * and the cards come back on a spaced-repetition schedule: the ones you miss
 * return tomorrow, the ones you know drift further out. A daily streak keeps
 * the habit honest.
 *
 * Everything here runs on the local model, so nothing you study leaves the
 * machine. It is a 3B model: good at fundamentals and well-known topics,
 * shakier on niche facts, and it is told to say when it is unsure. Treat its
 * explanations as a tutor's, not a textbook's.
 */
export function LearnScreen() {
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [stats, setStats] = useState<LearnStats | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [topics, setTopics] = useState<TopicView[]>([]);
  const [newSkill, setNewSkill] = useState('');
  const [status, setStatus] = useState<string | null>(null);
  // What is busy, so each button can show its own spinner state.
  const [busy, setBusy] = useState<string | null>(null);
  const [explanation, setExplanation] = useState<{ topic: string; text: string } | null>(null);

  // Review mode.
  const [deck, setDeck] = useState<CardView[] | null>(null);
  const [index, setIndex] = useState(0);
  const [revealed, setRevealed] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const [s, st] = await Promise.all([listSkills(), learnStats()]);
      setSkills(s);
      setStats(st);
      setSelected((cur) => cur ?? s[0]?.id ?? null);
    } catch (err) {
      setStatus(`Could not load: ${errText(err)}`);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (selected === null) {
      setTopics([]);
      return;
    }
    setExplanation(null);
    void skillTopics(selected)
      .then(setTopics)
      .catch(() => setTopics([]));
  }, [selected]);

  const add = useCallback(async () => {
    const name = newSkill.trim();
    if (!name) return;
    setBusy('add');
    setStatus('Drafting your plan…');
    try {
      const id = await addSkill(name);
      setNewSkill('');
      setSelected(id);
      setTopics(await generatePlan(id));
      setStatus(null);
    } catch (err) {
      setStatus(`Could not draft a plan: ${errText(err)}`);
    } finally {
      setBusy(null);
      void refresh();
    }
  }, [newSkill, refresh]);

  const redraft = useCallback(async () => {
    if (selected === null) return;
    setBusy('plan');
    setStatus('Redrafting the plan…');
    try {
      setTopics(await generatePlan(selected));
      setStatus(null);
    } catch (err) {
      setStatus(`Could not redraft: ${errText(err)}`);
    } finally {
      setBusy(null);
      void refresh();
    }
  }, [selected, refresh]);

  const toggle = useCallback(
    async (t: TopicView) => {
      setTopics((cur) => cur.map((x) => (x.id === t.id ? { ...x, done: !t.done } : x)));
      try {
        await setTopicDone(t.id, !t.done);
        void refresh();
      } catch {
        // Revert on failure so the box never lies about what was saved.
        setTopics((cur) => cur.map((x) => (x.id === t.id ? { ...x, done: t.done } : x)));
      }
    },
    [refresh],
  );

  const explain = useCallback(async (t: TopicView) => {
    setBusy(`explain-${t.id}`);
    setStatus(null);
    try {
      setExplanation({ topic: t.title, text: await explainTopic(t.id) });
    } catch (err) {
      setStatus(`Could not explain: ${errText(err)}`);
    } finally {
      setBusy(null);
    }
  }, []);

  const makeCards = useCallback(
    async (t: TopicView) => {
      setBusy(`cards-${t.id}`);
      setStatus(null);
      try {
        const n = await generateCards(t.id);
        setStatus(`Added ${n} cards for "${t.title}". They're due now.`);
        if (selected !== null) setTopics(await skillTopics(selected));
        void refresh();
      } catch (err) {
        setStatus(`Could not make cards: ${errText(err)}`);
      } finally {
        setBusy(null);
      }
    },
    [selected, refresh],
  );

  const startReview = useCallback(async (skillId?: number) => {
    try {
      const cards = await dueCards(skillId, 30);
      if (cards.length === 0) {
        setStatus('Nothing due. Make some cards from a topic first.');
        return;
      }
      setDeck(cards);
      setIndex(0);
      setRevealed(false);
      setStatus(null);
    } catch (err) {
      setStatus(`Could not start review: ${errText(err)}`);
    }
  }, []);

  const grade = useCallback(
    async (g: Grade) => {
      if (!deck) return;
      const card = deck[index];
      try {
        await reviewCard(card.id, g);
      } catch (err) {
        setStatus(`Could not save that review: ${errText(err)}`);
        return;
      }
      if (index + 1 >= deck.length) {
        setDeck(null);
        setStatus(`Session done: ${deck.length} card${deck.length === 1 ? '' : 's'} reviewed.`);
        void refresh();
      } else {
        setIndex(index + 1);
        setRevealed(false);
      }
    },
    [deck, index, refresh],
  );

  const remove = useCallback(
    async (id: number) => {
      await removeSkill(id).catch(() => {});
      if (selected === id) setSelected(null);
      void refresh();
    },
    [selected, refresh],
  );

  const current = skills.find((s) => s.id === selected) ?? null;

  // -- Review mode takes over the screen. ------------------------------------
  if (deck) {
    const card = deck[index];
    return (
      <div className="learn">
        <div className="review">
          <div className="review__top">
            <span className="review__skill">{card.skillName}</span>
            <span className="review__count">
              {index + 1} / {deck.length}
            </span>
            <button className="review__quit" type="button" onClick={() => setDeck(null)}>
              End session
            </button>
          </div>
          <div className="review__card">
            <p className="review__front">{card.front}</p>
            {revealed ? (
              <p className="review__back">{card.back}</p>
            ) : (
              <button className="review__reveal" type="button" onClick={() => setRevealed(true)}>
                Show answer
              </button>
            )}
          </div>
          {revealed && (
            <div className="review__grades">
              <button className="review__grade review__grade--again" type="button" onClick={() => void grade('again')}>
                Again
                <span className="review__hint">tomorrow</span>
              </button>
              <button className="review__grade review__grade--good" type="button" onClick={() => void grade('good')}>
                Good
              </button>
              <button className="review__grade review__grade--easy" type="button" onClick={() => void grade('easy')}>
                Easy
              </button>
            </div>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="learn">
      <header className="learn__head">
        <GraduationCap size={18} strokeWidth={1.8} aria-hidden="true" />
        <div className="learn__headings">
          <h1 className="learn__title">Learn</h1>
          <p className="learn__subtitle">
            Plans, explanations and flashcards from your local model. Nothing leaves this Mac.
          </p>
        </div>
        {stats && (
          <div className="learn__stats">
            <span className={`learn__streak${stats.streak > 0 ? ' learn__streak--on' : ''}`}>
              <Flame size={13} strokeWidth={2} aria-hidden="true" />
              {stats.streak} day{stats.streak === 1 ? '' : 's'}
            </span>
            <button
              className="learn__review-all"
              type="button"
              onClick={() => void startReview()}
              disabled={stats.dueToday === 0}
            >
              <RotateCcw size={12} strokeWidth={2} aria-hidden="true" />
              Review {stats.dueToday} due
            </button>
          </div>
        )}
      </header>

      {status && <p className="learn__status">{status}</p>}

      <div className="learn__body">
        <aside className="learn__skills">
          <div className="learn__add">
            <input
              className="learn__input"
              type="text"
              placeholder="Learn a skill, e.g. System design"
              value={newSkill}
              onChange={(e) => setNewSkill(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void add();
              }}
              disabled={busy === 'add'}
              aria-label="New skill"
            />
            <button
              className="learn__add-btn"
              type="button"
              onClick={() => void add()}
              disabled={busy === 'add' || !newSkill.trim()}
              aria-label="Add skill"
            >
              <Plus size={15} strokeWidth={2.5} aria-hidden="true" />
            </button>
          </div>

          {skills.length === 0 && (
            <p className="learn__empty">
              Add a skill and NEXUS drafts a plan for it, using your Me profile.
            </p>
          )}

          <ul className="learn__skill-list">
            {skills.map((s) => {
              const pct = s.topicsTotal > 0 ? (s.topicsDone / s.topicsTotal) * 100 : 0;
              return (
                <li key={s.id}>
                  <button
                    type="button"
                    className={`learn__skill${s.id === selected ? ' learn__skill--on' : ''}`}
                    onClick={() => setSelected(s.id)}
                  >
                    <span className="learn__skill-name">{s.name}</span>
                    <span className="learn__skill-meta">
                      {s.topicsDone}/{s.topicsTotal} topics
                      {s.cardsDue > 0 && <span className="learn__due-dot">{s.cardsDue} due</span>}
                    </span>
                    <span className="learn__skill-bar">
                      <span className="learn__skill-fill" style={{ width: `${pct}%` }} />
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        </aside>

        <section className="learn__plan">
          {current ? (
            <>
              <div className="learn__plan-head">
                <h2 className="learn__plan-title">{current.name}</h2>
                <button
                  className="learn__ghost"
                  type="button"
                  onClick={() => void startReview(current.id)}
                  disabled={current.cardsDue === 0}
                >
                  <RotateCcw size={12} strokeWidth={2} aria-hidden="true" />
                  Review {current.cardsDue}
                </button>
                <button
                  className="learn__ghost"
                  type="button"
                  onClick={() => void redraft()}
                  disabled={busy === 'plan'}
                  title="Ask the model for a fresh plan. Cards you made are kept."
                >
                  {busy === 'plan' ? 'Drafting…' : 'Redraft plan'}
                </button>
                <button
                  className="learn__remove"
                  type="button"
                  onClick={() => void remove(current.id)}
                  aria-label={`Remove ${current.name}`}
                >
                  <X size={13} strokeWidth={2.5} aria-hidden="true" />
                </button>
              </div>

              {topics.length === 0 && busy !== 'add' && (
                <p className="learn__empty">
                  No plan yet. <button className="learn__link" type="button" onClick={() => void redraft()}>Draft one</button>.
                </p>
              )}

              <ol className="learn__topics">
                {topics.map((t) => (
                  <li className={`learn__topic${t.done ? ' learn__topic--done' : ''}`} key={t.id}>
                    <button
                      className={`learn__check${t.done ? ' learn__check--on' : ''}`}
                      type="button"
                      onClick={() => void toggle(t)}
                      aria-pressed={t.done}
                      aria-label={`Mark "${t.title}" ${t.done ? 'not done' : 'done'}`}
                    >
                      {t.done && <Check size={11} strokeWidth={3} aria-hidden="true" />}
                    </button>
                    <span className="learn__topic-title">{t.title}</span>
                    {t.cards > 0 && <span className="learn__topic-cards">{t.cards} cards</span>}
                    <button
                      className="learn__ghost learn__ghost--sm"
                      type="button"
                      onClick={() => void explain(t)}
                      disabled={busy !== null}
                    >
                      <BookOpen size={11} strokeWidth={2} aria-hidden="true" />
                      {busy === `explain-${t.id}` ? 'Thinking…' : 'Explain'}
                    </button>
                    <button
                      className="learn__ghost learn__ghost--sm"
                      type="button"
                      onClick={() => void makeCards(t)}
                      disabled={busy !== null}
                    >
                      <Layers size={11} strokeWidth={2} aria-hidden="true" />
                      {busy === `cards-${t.id}` ? 'Writing…' : 'Make cards'}
                    </button>
                  </li>
                ))}
              </ol>

              {explanation && (
                <div className="learn__explain">
                  <div className="learn__explain-head">
                    <span>{explanation.topic}</span>
                    <button className="learn__remove" type="button" onClick={() => setExplanation(null)} aria-label="Close explanation">
                      <X size={12} strokeWidth={2.5} aria-hidden="true" />
                    </button>
                  </div>
                  <p className="learn__explain-text">{explanation.text}</p>
                  <span className="learn__by">composed by NEXUS · check anything that matters</span>
                </div>
              )}
            </>
          ) : (
            skills.length > 0 && <p className="learn__empty">Pick a skill on the left.</p>
          )}
        </section>
      </div>
    </div>
  );
}
