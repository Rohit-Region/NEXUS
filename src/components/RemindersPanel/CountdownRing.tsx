interface Props {
  /** Seconds left. Negative is overdue. */
  left: number;
  /** Seconds the reminder was set for, so the ring has something to empty. */
  span: number;
  imminent: boolean;
}

const SIZE = 26;
const STROKE = 2.5;
const R = (SIZE - STROKE) / 2;
const CIRCUMFERENCE = 2 * Math.PI * R;

/**
 * A ring that empties as a reminder approaches.
 *
 * The number beside it is the truth; this is for reading across the panel
 * without focusing on any one row. Proportion is what a ring is good at and
 * exact time is what it is bad at, so it carries only the former.
 *
 * Capped at an hour of arc. A reminder three days out would otherwise sit at
 * a full ring for two and a half days and look identical to one set a minute
 * ago; within the last hour the movement is worth watching.
 */
export function CountdownRing({ left, span, imminent }: Props) {
  const window = Math.min(span, 3600);
  const fraction = Math.max(0, Math.min(1, left / window));
  const offset = CIRCUMFERENCE * (1 - fraction);

  return (
    <svg
      className={`ring${imminent ? ' ring--imminent' : ''}`}
      width={SIZE}
      height={SIZE}
      viewBox={`0 0 ${SIZE} ${SIZE}`}
      aria-hidden="true"
      focusable="false"
    >
      <circle
        className="ring__track"
        cx={SIZE / 2}
        cy={SIZE / 2}
        r={R}
        strokeWidth={STROKE}
        fill="none"
      />
      <circle
        className="ring__arc"
        cx={SIZE / 2}
        cy={SIZE / 2}
        r={R}
        strokeWidth={STROKE}
        fill="none"
        strokeDasharray={CIRCUMFERENCE}
        strokeDashoffset={offset}
        strokeLinecap="round"
        /* Starts at twelve o'clock and empties clockwise, which is the
           direction every clock face has trained people to read. */
        transform={`rotate(-90 ${SIZE / 2} ${SIZE / 2})`}
      />
    </svg>
  );
}
