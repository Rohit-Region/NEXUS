import { useEffect, useRef, useState } from 'react';

export interface Point {
  /** Milliseconds since 1970, UTC midnight. */
  t: number;
  v: number;
}

export interface Series {
  key: string;
  label: string;
  color: string;
  points: Point[];
  /** Drawn heavier: the user's own funds. */
  emphasis?: boolean;
  /** Drawn dashed: a reference line, such as money paid in. */
  dashed?: boolean;
}

interface Props {
  series: Series[];
  format: (v: number) => string;
  /** A reference level drawn dashed, such as the starting amount. */
  baseline?: number;
  height?: number;
  label: string;
}

const PAD = { top: 12, right: 12, bottom: 24, left: 64 };

/** Round tick values covering [lo, hi]. */
function ticks(lo: number, hi: number, count: number): number[] {
  const raw = (hi - lo) / count;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? raw;
  const out = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + step * 1e-9; v += step) out.push(v);
  return out;
}

function dateLabel(t: number, span: number): string {
  const d = new Date(t);
  return span > 400 * 86_400_000
    ? d.toLocaleDateString('en-IN', { month: 'short', year: '2-digit', timeZone: 'UTC' })
    : d.toLocaleDateString('en-IN', { day: 'numeric', month: 'short', timeZone: 'UTC' });
}

/** The point at or just before `t`. */
function at(points: Point[], t: number): Point | null {
  let lo = 0;
  let hi = points.length - 1;
  if (hi < 0 || points[0].t > t) return null;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (points[mid].t <= t) lo = mid;
    else hi = mid - 1;
  }
  return points[lo];
}

/** Lines over time on one shared axis, with a crosshair and tooltip. */
export function LineChart({ series, format, baseline, height = 260, label }: Props) {
  const wrap = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(640);
  const [hover, setHover] = useState<number | null>(null);

  useEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => setWidth(Math.max(280, entry.contentRect.width)));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const drawn = series.filter((s) => s.points.length > 0);
  const geom = (() => {
    const all = drawn.flatMap((s) => s.points);
    if (all.length === 0) return null;
    const t0 = Math.min(...all.map((p) => p.t));
    const t1 = Math.max(...all.map((p) => p.t));
    let lo = Math.min(...all.map((p) => p.v), baseline ?? Infinity);
    let hi = Math.max(...all.map((p) => p.v), baseline ?? -Infinity);
    const pad = (hi - lo || Math.abs(hi) || 1) * 0.06;
    lo -= pad;
    hi += pad;
    const w = width - PAD.left - PAD.right;
    const h = height - PAD.top - PAD.bottom;
    const x = (t: number) => PAD.left + (t1 === t0 ? w / 2 : ((t - t0) / (t1 - t0)) * w);
    const y = (v: number) => PAD.top + (1 - (v - lo) / (hi - lo)) * h;
    return { t0, t1, lo, hi, x, y, w, h };
  })();

  if (!geom) {
    return (
      <div ref={wrap} className="lchart lchart--empty" style={{ height }}>
        No prices in this period.
      </div>
    );
  }
  const { t0, t1, lo, hi, x, y } = geom;
  const span = t1 - t0;
  const xTicks = span === 0 ? [t0] : [0, 0.25, 0.5, 0.75, 1].map((f) => t0 + f * span);
  const hoverRows =
    hover === null
      ? []
      : drawn.map((s) => ({ s, p: at(s.points, hover) })).filter((r): r is { s: Series; p: Point } => r.p !== null);
  const tipLeft = hover !== null && x(hover) > width / 2;

  return (
    <div ref={wrap} className="lchart">
      <svg
        width={width}
        height={height}
        role="img"
        aria-label={label}
        onPointerMove={(e) => {
          const box = e.currentTarget.getBoundingClientRect();
          const px = Math.min(Math.max(e.clientX - box.left, PAD.left), width - PAD.right);
          setHover(t0 + ((px - PAD.left) / geom.w) * span);
        }}
        onPointerLeave={() => setHover(null)}
      >
        {ticks(lo, hi, 4).map((v) => (
          <g key={v}>
            <line className="lchart__grid" x1={PAD.left} x2={width - PAD.right} y1={y(v)} y2={y(v)} />
            <text className="lchart__tick" x={PAD.left - 6} y={y(v)} textAnchor="end" dominantBaseline="middle">
              {format(v)}
            </text>
          </g>
        ))}
        {xTicks.map((t, i) => (
          <text
            key={t}
            className="lchart__tick"
            x={x(t)}
            y={height - 6}
            textAnchor={i === 0 ? 'start' : i === xTicks.length - 1 ? 'end' : 'middle'}
          >
            {dateLabel(t, span)}
          </text>
        ))}
        {baseline !== undefined && (
          <line className="lchart__base" x1={PAD.left} x2={width - PAD.right} y1={y(baseline)} y2={y(baseline)} />
        )}
        {drawn.map((s) => (
          <polyline
            key={s.key}
            className={`lchart__line${s.emphasis ? ' lchart__line--em' : ''}${s.dashed ? ' lchart__line--dashed' : ''}`}
            points={s.points.map((p) => `${x(p.t).toFixed(1)},${y(p.v).toFixed(1)}`).join(' ')}
            style={{ stroke: s.color }}
          />
        ))}
        {hover !== null && (
          <>
            <line className="lchart__cross" x1={x(hover)} x2={x(hover)} y1={PAD.top} y2={height - PAD.bottom} />
            {hoverRows.map(({ s, p }) => (
              <circle key={s.key} className="lchart__dot" cx={x(p.t)} cy={y(p.v)} r={4} style={{ fill: s.color }} />
            ))}
          </>
        )}
      </svg>
      {hover !== null && hoverRows.length > 0 && (
        <div
          className="lchart__tip"
          style={tipLeft ? { right: width - x(hover) + 10 } : { left: x(hover) + 10 }}
          role="status"
        >
          <div className="lchart__tip-date">
            {new Date(hoverRows[0].p.t).toLocaleDateString('en-IN', { day: 'numeric', month: 'short', year: 'numeric', timeZone: 'UTC' })}
          </div>
          {[...hoverRows]
            .sort((a, b) => b.p.v - a.p.v)
            .map(({ s, p }) => (
              <div className="lchart__tip-row" key={s.key}>
                <span className="lchart__swatch" style={{ background: s.color }} aria-hidden="true" />
                <span className="lchart__tip-name">{s.label}</span>
                <span className="lchart__tip-val">{format(p.v)}</span>
              </div>
            ))}
        </div>
      )}
    </div>
  );
}
