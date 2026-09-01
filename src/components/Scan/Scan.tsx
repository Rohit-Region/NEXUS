import './Scan.css';

interface Props {
  /** What is being waited for, said plainly. */
  label?: string;
  /** Rows of placeholder to hold the space the content will take. */
  rows?: number;
}

/**
 * The waiting state, in the same idiom as everything else.
 *
 * Two things happening at once, and both earn their place:
 *
 * - A **determinate-looking sweep** that is honestly indeterminate. It runs
 *   edge to edge rather than filling up, because a bar that fills implies a
 *   proportion NEXUS does not know. A progress bar that lies about progress
 *   is worse than one that only says "still going".
 * - **Skeleton rows** the height of the real ones, so the panel does not
 *   jump when the content lands. Layout shift on arrival is the thing that
 *   makes an interface feel unfinished.
 *
 * Announced politely to a screen reader once, not on every frame.
 */
export function Scan({ label = 'Reading', rows = 3 }: Props) {
  return (
    <div className="scan" role="status" aria-live="polite">
      <div className="scan__bar" aria-hidden="true">
        <span className="scan__sweep" />
      </div>
      <span className="scan__label">{label}</span>
      <div className="scan__rows" aria-hidden="true">
        {Array.from({ length: rows }, (_, i) => (
          <span
            className="scan__row"
            key={i}
            /* Staggered so the rows breathe out of step. In phase they read
               as one block flashing, which is harder to look at. */
            style={{ animationDelay: `${i * 0.12}s` }}
          />
        ))}
      </div>
    </div>
  );
}
