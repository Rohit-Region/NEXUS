import type { Expense, ExpenseInput, SpendKind } from '../../../lib/nexus-db';

const INR = new Intl.NumberFormat('en-IN', { style: 'currency', currency: 'INR', maximumFractionDigits: 2 });
const INR_WHOLE = new Intl.NumberFormat('en-IN', { style: 'currency', currency: 'INR', maximumFractionDigits: 0 });

/** Full amount: ₹2,295.99, or ₹2,296 when `whole`. */
export function inr(paise: number, whole = false): string {
  return (whole ? INR_WHOLE : INR).format(paise / 100);
}

/** Short amount for tight spaces: ₹950, ₹2.3k, ₹1.2L, ₹1.1Cr. */
export function inrShort(paise: number): string {
  const r = paise / 100;
  const trim = (n: number) => (n >= 10 ? Math.round(n).toString() : n.toFixed(1).replace(/\.0$/, ''));
  if (r >= 1e7) return `₹${trim(r / 1e7)}Cr`;
  if (r >= 1e5) return `₹${trim(r / 1e5)}L`;
  if (r >= 1e3) return `₹${trim(r / 1e3)}k`;
  return `₹${Math.round(r)}`;
}

function parts(iso: string): [number, number, number] {
  const [y, m, d] = iso.split('-').map(Number);
  return [y, m, d ?? 1];
}

/** "2026-10" as "October 2026". */
export function monthLabel(month: string): string {
  const [y, m] = parts(month);
  return new Date(y, m - 1, 1).toLocaleDateString('en-IN', { month: 'long', year: 'numeric' });
}

export function shiftMonth(month: string, by: number): string {
  const [y, m] = parts(month);
  const d = new Date(y, m - 1 + by, 1);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}`;
}

/** "2026-10-24" as "Sat, 24 Oct". */
export function dayLabel(iso: string): string {
  const [y, m, d] = parts(iso);
  return new Date(y, m - 1, d).toLocaleDateString('en-IN', { weekday: 'short', day: 'numeric', month: 'short' });
}

/** Unix seconds as "10:42 am". */
export function timeLabel(unix: number | null): string {
  if (unix === null) return '';
  return new Date(unix * 1000).toLocaleTimeString('en-IN', { hour: 'numeric', minute: '2-digit' });
}

/** "3 min ago" style, for the last sync. */
export function ago(unix: number): string {
  const s = Math.max(0, Math.floor(Date.now() / 1000) - unix);
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

export const KIND_LABEL: Record<SpendKind, string> = {
  spend: 'Spending',
  income: 'Income',
  refund: 'Refund',
  transfer: 'Moved',
};

export const KIND_HINT: Record<SpendKind, string> = {
  spend: 'Counts toward spending',
  income: 'Money in, not counted as spending',
  refund: 'Money back, not counted as spending',
  transfer: 'Card bill, investment or your own account: not spending',
};

export function toInput(e: Expense): ExpenseInput {
  return {
    date: e.occurredOn,
    amount: e.amountPaise / 100,
    payee: e.payee,
    category: e.category,
    account: e.account,
    kind: e.kind,
    note: e.note,
  };
}

export function errorText(err: unknown): string {
  return String(err).replace(/^Error:\s*/, '');
}
