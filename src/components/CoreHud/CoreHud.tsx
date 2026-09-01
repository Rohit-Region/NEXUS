import { useEffect, useMemo, useState } from 'react';
import { listCommitments, listConnectors } from '../../lib/assistant';
import type { Commitment } from '../../lib/assistant';
import type { ConnectorStatus, ConnectorView } from '../../types/assistant';
import type { WorkspaceSummary } from '../../types/db';
import './CoreHud.css';

interface Props {
  summary: WorkspaceSummary | null;
  /** Where a ring or node should take the user when pressed. */
  navigate?: (screen: 'overview' | 'projects' | 'registry' | 'settings') => void;
}

const REFRESH_MS = 8000;

/** Geometry. One source, so the rings cannot drift apart. */
const SIZE = 260;
const C = SIZE / 2;
const RINGS = { outer: 116, mid: 98, inner: 80 };

function arc(radius: number, fraction: number) {
  const circumference = 2 * Math.PI * radius;
  return {
    strokeDasharray: circumference,
    strokeDashoffset: circumference * (1 - Math.max(0, Math.min(1, fraction))),
  };
}

/** Statuses that mean the connector would answer if asked. */
const HEALTHY: ConnectorStatus[] = ['ready'];
const DEGRADED: ConnectorStatus[] = ['degraded', 'needsAuth'];

function countdown(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) {
    return `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}`;
  }
  return `${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}`;
}

/**
 * The state of everything, in one glance.
 *
 * Three concentric arcs, and each is a real proportion rather than a
 * decoration that happens to move:
 *
 * - Outer: how much of the work is finished. The ring read first answers the
 *   question asked most.
 * - Mid: how much of it is moving rather than waiting.
 * - Inner: how close the next reminder is, so the thing about to happen sits
 *   nearest the middle.
 *
 * Around them, one node per connector, coloured by the status the connector
 * reports about itself. Drawing them in a circle is for peripheral vision: a
 * red node is visible without being looked at, which is how you learn Jira is
 * unhappy before you need Jira.
 *
 * Nothing here is invented. Every arc, node and number comes from data the
 * application already holds, because a dial that moves for effect is worse
 * than no dial: it teaches you to ignore the ones that mean something.
 */
export function CoreHud({ summary, navigate }: Props) {
  // Drawn once on mount, then never again. A boot sequence that replays on
  // every re-render is a distraction; one that plays when the application
  // starts is the machine coming up.
  const [booted, setBooted] = useState(false);
  useEffect(() => {
    const timer = window.setTimeout(() => setBooted(true), 40);
    return () => window.clearTimeout(timer);
  }, []);

  const [connectors, setConnectors] = useState<ConnectorView[]>([]);
  const [commitments, setCommitments] = useState<Commitment[]>([]);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  const [clock, setClock] = useState(() => new Date());

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const [c, k] = await Promise.all([listConnectors(), listCommitments(true)]);
        if (!alive) return;
        setConnectors(c);
        setCommitments(k);
      } catch {
        // A HUD that throws is worse than one that is briefly stale. The
        // panels below report their own failures in their own words.
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    const timer = window.setInterval(() => {
      setNow(Math.floor(Date.now() / 1000));
      setClock(new Date());
    }, 1000);
    return () => window.clearInterval(timer);
  }, []);

  const next = useMemo(() => {
    const armed = commitments
      .filter((c) => c.dueAt !== null && c.raisedAt === null && c.dueAt > now)
      .sort((a, b) => (a.dueAt ?? 0) - (b.dueAt ?? 0));
    return armed[0] ?? null;
  }, [commitments, now]);

  const total = summary?.tasks ?? 0;
  const done = summary?.tasksDone ?? 0;
  const moving = summary?.tasksInProgress ?? 0;
  const blocked = summary?.tasksBlocked ?? 0;

  const doneFraction = total > 0 ? done / total : 0;
  const movingFraction = total > 0 ? moving / total : 0;
  // Capped at an hour, the same window the row rings use, so a reminder days
  // out does not sit full for days looking identical to a fresh one.
  const nextLeft = next?.dueAt ? next.dueAt - now : 0;
  const nextFraction = next ? Math.min(1, nextLeft / 3600) : 0;

  const healthy = connectors.filter((c) => HEALTHY.includes(c.status)).length;
  const unhappy = connectors.filter((c) => DEGRADED.includes(c.status)).length;

  return (
    <div className={`hud${booted ? ' hud--booted' : ''}`}>
      <div className="hud__core">
        <svg
          className="hud__rings"
          width={SIZE}
          height={SIZE}
          viewBox={`0 0 ${SIZE} ${SIZE}`}
          aria-hidden="true"
        >
          {/* Fixed ticks, so the moving arcs have something still to move
              against. Without them the rings read as floating. */}
          <g className="hud__ticks">
            {Array.from({ length: 60 }, (_, i) => {
              const angle = (i / 60) * Math.PI * 2 - Math.PI / 2;
              const r1 = RINGS.outer + 8;
              const r2 = r1 + (i % 5 === 0 ? 6 : 3);
              return (
                <line
                  key={i}
                  x1={C + Math.cos(angle) * r1}
                  y1={C + Math.sin(angle) * r1}
                  x2={C + Math.cos(angle) * r2}
                  y2={C + Math.sin(angle) * r2}
                  className={i % 5 === 0 ? 'hud__tick hud__tick--major' : 'hud__tick'}
                />
              );
            })}
          </g>

          {(['outer', 'mid', 'inner'] as const).map((key) => (
            <circle
              key={key}
              className="hud__track"
              cx={C}
              cy={C}
              r={RINGS[key]}
              fill="none"
            />
          ))}

          <circle
            className="hud__arc hud__arc--done"
            cx={C}
            cy={C}
            r={RINGS.outer}
            fill="none"
            transform={`rotate(-90 ${C} ${C})`}
            {...arc(RINGS.outer, doneFraction)}
          />
          <circle
            className="hud__arc hud__arc--moving"
            cx={C}
            cy={C}
            r={RINGS.mid}
            fill="none"
            transform={`rotate(-90 ${C} ${C})`}
            {...arc(RINGS.mid, movingFraction)}
          />
          <circle
            className={`hud__arc hud__arc--next${
              nextLeft > 0 && nextLeft <= 60 ? ' hud__arc--urgent' : ''
            }`}
            cx={C}
            cy={C}
            r={RINGS.inner}
            fill="none"
            transform={`rotate(-90 ${C} ${C})`}
            {...arc(RINGS.inner, nextFraction)}
          />

          {/* One node per connector, evenly spaced on the outer track. */}
          {connectors.map((c, i) => {
            const angle =
              (i / Math.max(1, connectors.length)) * Math.PI * 2 - Math.PI / 2;
            const state = HEALTHY.includes(c.status)
              ? 'ok'
              : DEGRADED.includes(c.status)
                ? 'warn'
                : 'off';
            return (
              <circle
                key={c.id}
                className={`hud__node hud__node--${state}${navigate ? ' hud__node--live' : ''}`}
                cx={C + Math.cos(angle) * RINGS.outer}
                cy={C + Math.sin(angle) * RINGS.outer}
                r={3.5}
                role={navigate ? 'button' : undefined}
                tabIndex={navigate ? 0 : undefined}
                onClick={navigate ? () => navigate('settings') : undefined}
                onKeyDown={
                  navigate
                    ? (e) => {
                        if (e.key === 'Enter' || e.key === ' ') navigate('settings');
                      }
                    : undefined
                }
              >
                <title>
                  {c.displayName}: {c.status}
                  {navigate ? ' (open Permissions)' : ''}
                </title>
              </circle>
            );
          })}
        </svg>

        <div className="hud__centre">
          <span className="hud__clock">
            {String(clock.getHours()).padStart(2, '0')}
            <span className="hud__colon">:</span>
            {String(clock.getMinutes()).padStart(2, '0')}
          </span>
          <span className="hud__date">
            {clock.toLocaleDateString(undefined, {
              weekday: 'short',
              day: 'numeric',
              month: 'short',
            })}
          </span>
          {next ? (
            <span className="hud__next">
              <span className="hud__next-time">{countdown(nextLeft)}</span>
              <span className="hud__next-what">{next.what}</span>
            </span>
          ) : (
            <span className="hud__next hud__next--idle">nothing scheduled</span>
          )}
        </div>
      </div>

      {/* Not decoration: three unlabelled arcs are a puzzle. */}
      <dl className="hud__legend">
        <button
          className="hud__legend-item hud__legend-item--go"
          type="button"
          onClick={() => navigate?.('projects')}
        >
          <dt>
            <span className="hud__key hud__key--done" aria-hidden="true" />
            Done
          </dt>
          <dd>
            {done}
            <span className="hud__of">/{total}</span>
          </dd>
        </button>
        <div className="hud__legend-item">
          <dt>
            <span className="hud__key hud__key--moving" aria-hidden="true" />
            Moving
          </dt>
          <dd>{moving}</dd>
        </div>
        <div className="hud__legend-item">
          <dt>
            <span className="hud__key hud__key--blocked" aria-hidden="true" />
            Blocked
          </dt>
          <dd className={blocked > 0 ? 'hud__alarm' : undefined}>{blocked}</dd>
        </div>
        <div className="hud__legend-item">
          <dt>
            <span className="hud__key hud__key--link" aria-hidden="true" />
            Linked
          </dt>
          <dd className={unhappy > 0 ? 'hud__alarm' : undefined}>
            {healthy}
            <span className="hud__of">/{connectors.length}</span>
          </dd>
        </div>
      </dl>
    </div>
  );
}
