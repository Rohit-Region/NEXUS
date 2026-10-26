import { useCallback, useEffect, useRef, useState } from 'react';
import {
  getProfile,
  retirementProjection,
  setProfileField,
  type RetirementProjection,
} from '../../lib/nexus-db';

function inr(n: number): string {
  return new Intl.NumberFormat('en-IN', {
    style: 'currency',
    currency: 'INR',
    maximumFractionDigits: 0,
  }).format(n);
}

/** A large rupee figure as "₹1.2 Cr" / "₹45.0 L", the way Indians read corpus. */
function compact(n: number): string {
  if (n >= 1e7) return `₹${(n / 1e7).toFixed(2)} Cr`;
  if (n >= 1e5) return `₹${(n / 1e5).toFixed(1)} L`;
  return inr(n);
}

/** One numeric input in the form. */
interface FieldDef {
  key: string;
  label: string;
  suffix?: string;
  profileKey: string;
  fallback: string;
}

const FIELDS: FieldDef[] = [
  { key: 'currentAge', label: 'Current age', profileKey: 'retire_current_age', fallback: '30' },
  { key: 'retirementAge', label: 'Retire at', profileKey: 'retire_retirement_age', fallback: '60' },
  { key: 'currentCorpus', label: 'Current corpus', suffix: '₹', profileKey: 'retire_corpus', fallback: '0' },
  { key: 'monthlySip', label: 'Monthly investment', suffix: '₹', profileKey: 'retire_sip', fallback: '0' },
  { key: 'expectedReturn', label: 'Expected return', suffix: '%', profileKey: 'retire_return', fallback: '10' },
  { key: 'inflation', label: 'Inflation', suffix: '%', profileKey: 'retire_inflation', fallback: '6' },
  { key: 'monthlyExpenseToday', label: "Today's monthly expense", suffix: '₹', profileKey: 'retire_expense', fallback: '40000' },
];

/**
 * Retirement: where your current corpus and monthly investing land by the age
 * you pick, and whether that covers what you'll need.
 *
 * The target is sized at 25x your inflated annual expenses (the 4% rule), which
 * is named on screen so you can judge it. Everything here is a projection under
 * the assumptions you enter, a return rate, an inflation rate, a horizon. It
 * compounds them honestly; it does not predict or promise them.
 *
 * Inputs persist in your profile, so the projection is there next time.
 */
export function RetirementPanel() {
  const [values, setValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(FIELDS.map((f) => [f.key, f.fallback])),
  );
  const [result, setResult] = useState<RetirementProjection | null>(null);

  // Load saved inputs from the profile on mount.
  useEffect(() => {
    void getProfile()
      .then((p) => {
        setValues((prev) => {
          const next = { ...prev };
          for (const f of FIELDS) {
            if (p[f.profileKey]) next[f.key] = p[f.profileKey];
          }
          return next;
        });
      })
      .catch(() => {});
  }, []);

  // Recompute (and persist) a short beat after the last edit, so typing stays
  // responsive and the projection follows without a button.
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      const input = {
        currentAge: Number(values.currentAge) || 0,
        retirementAge: Number(values.retirementAge) || 0,
        currentCorpus: Number(values.currentCorpus) || 0,
        monthlySip: Number(values.monthlySip) || 0,
        expectedReturn: Number(values.expectedReturn) || 0,
        inflation: Number(values.inflation) || 0,
        monthlyExpenseToday: Number(values.monthlyExpenseToday) || 0,
      };
      void retirementProjection(input).then(setResult).catch(() => {});
    }, 350);
    return () => window.clearTimeout(timer.current);
  }, [values]);

  const onChange = (key: string, v: string) => setValues((prev) => ({ ...prev, [key]: v }));

  const persist = useCallback((profileKey: string, v: string) => {
    void setProfileField(profileKey, v).catch(() => {});
  }, []);

  return (
    <div className="retire">
      <p className="money__note">
        A projection under the assumptions you enter, not a prediction. Target corpus
        is 25x your inflated annual expenses (the 4% rule).
      </p>

      <div className="retire__form">
        {FIELDS.map((f) => (
          <label className="retire__field" key={f.key}>
            <span className="retire__label">{f.label}</span>
            <span className="retire__input-wrap">
              <input
                className="money__input"
                type="number"
                step="any"
                value={values[f.key] ?? ''}
                onChange={(e) => onChange(f.key, e.target.value)}
                onBlur={(e) => persist(f.profileKey, e.target.value)}
                aria-label={f.label}
              />
              {f.suffix && <span className="retire__suffix">{f.suffix}</span>}
            </span>
          </label>
        ))}
      </div>

      {result && (
        <div className="retire__result">
          <div className={`retire__verdict retire__verdict--${result.onTrack ? 'ok' : 'short'}`}>
            {result.onTrack ? 'On track' : 'Shortfall'}
            <span className="retire__verdict-sub">
              over {result.years.toFixed(0)} years
            </span>
          </div>

          <div className="retire__figures">
            <div className="retire__fig">
              <span className="retire__fig-label">Projected corpus</span>
              <span className="retire__fig-value">{compact(result.projectedCorpus)}</span>
            </div>
            <div className="retire__fig">
              <span className="retire__fig-label">Target corpus</span>
              <span className="retire__fig-value">{compact(result.targetCorpus)}</span>
            </div>
            {!result.onTrack && (
              <>
                <div className="retire__fig">
                  <span className="retire__fig-label">Shortfall</span>
                  <span className="retire__fig-value retire__fig-value--short">
                    {compact(result.shortfall)}
                  </span>
                </div>
                <div className="retire__fig">
                  <span className="retire__fig-label">Invest this much more / mo</span>
                  <span className="retire__fig-value retire__fig-value--short">
                    {inr(result.extraMonthly)}
                  </span>
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
