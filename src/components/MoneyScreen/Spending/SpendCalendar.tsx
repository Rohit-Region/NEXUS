import type { Expense, SpendMonth } from '../../../lib/nexus-db';
import { dayLabel, inr, inrShort, KIND_LABEL, timeLabel } from './format';

const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];

/** Heat level 0-4 for a day: one hue, light to dark, relative to the month's
 *  biggest day, so the calendar shows where spending clusters at a glance. */
function level(paise: number, max: number): number {
  if (paise <= 0 || max <= 0) return 0;
  return Math.max(1, Math.min(4, Math.ceil((paise / max) * 4)));
}

interface Props {
  data: SpendMonth;
  selected: string | null;
  onSelect: (date: string) => void;
  onEdit: (e: Expense) => void;
}

/**
 * The month as a calendar: what was spent each day, shaded by how much, and
 * the selected day's payments beside it. Answers "how much on which day, and
 * where" in one place.
 */
export function SpendCalendar({ data, selected, onSelect, onEdit }: Props) {
  const max = Math.max(0, ...data.days.map((d) => d.spendPaise));
  const blanks = Array.from({ length: data.firstWeekday });
  const dayRows = selected ? data.transactions.filter((t) => t.occurredOn === selected) : [];
  const daySpend = dayRows.filter((t) => t.kind === 'spend').reduce((s, t) => s + t.amountPaise, 0);

  return (
    <section className="spend-cal" aria-label="Spending calendar">
      <div className="spend-cal__grid" aria-label="Days of the month">
        {WEEKDAYS.map((w) => (
          <span className="spend-cal__weekday" key={w} aria-hidden="true">
            {w}
          </span>
        ))}
        {blanks.map((_, i) => (
          <span className="spend-cal__blank" key={`b${i}`} aria-hidden="true" />
        ))}
        {data.days.map((d) => {
          const n = Number(d.date.slice(8));
          const isToday = d.date === data.today;
          const future = d.date > data.today;
          const lv = level(d.spendPaise, max);
          const label = `${dayLabel(d.date)}: ${d.spendPaise > 0 ? `${inr(d.spendPaise)}, ${d.count} payment${d.count === 1 ? '' : 's'}` : 'nothing spent'}`;
          return (
            <button
              key={d.date}
              type="button"
              className={[
                'spend-cal__day',
                `spend-cal__day--l${lv}`,
                isToday ? 'spend-cal__day--today' : '',
                future ? 'spend-cal__day--future' : '',
                selected === d.date ? 'spend-cal__day--on' : '',
              ].join(' ')}
              aria-pressed={selected === d.date}
              aria-label={label}
              title={label}
              onClick={() => onSelect(d.date)}
            >
              <span className="spend-cal__num">{n}</span>
              {d.spendPaise > 0 && <span className="spend-cal__amt">{inrShort(d.spendPaise)}</span>}
            </button>
          );
        })}
      </div>

      <div className="spend-cal__legend" aria-hidden="true">
        <span>Less</span>
        {[0, 1, 2, 3, 4].map((l) => (
          <span key={l} className={`spend-cal__swatch spend-cal__day--l${l}`} />
        ))}
        <span>More</span>
      </div>

      <div className="spend-day" aria-live="polite">
        {!selected && <p className="spend-day__empty">Pick a day to see where the money went.</p>}
        {selected && (
          <>
            <div className="spend-day__head">
              <span className="spend-day__title">{dayLabel(selected)}</span>
              <span className="spend-day__total">{inr(daySpend)}</span>
            </div>
            {dayRows.length === 0 && <p className="spend-day__empty">Nothing recorded this day.</p>}
            <ul className="spend-day__list">
              {dayRows.map((t) => (
                <li key={t.id}>
                  <button className="spend-day__row" type="button" onClick={() => onEdit(t)}>
                    <span className="spend-day__payee">
                      {t.payee}
                      <span className="spend-day__meta">
                        {t.category}
                        {t.kind !== 'spend' && ` · ${KIND_LABEL[t.kind]}`}
                        {t.occurredAt !== null && ` · ${timeLabel(t.occurredAt)}`}
                        {` · ${t.account}`}
                      </span>
                    </span>
                    <span className={`spend-amt spend-amt--${t.kind}`}>
                      {t.kind === 'income' || t.kind === 'refund' ? '+' : ''}
                      {inr(t.amountPaise)}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </>
        )}
      </div>
    </section>
  );
}
