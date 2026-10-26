import { useCallback, useEffect, useState } from 'react';
import { Plus, X, Check } from 'lucide-react';
import {
  addGoal,
  listGoals,
  removeGoal,
  updateGoalAmount,
  type GoalProgress,
} from '../../lib/nexus-db';

function inr(n: number): string {
  return new Intl.NumberFormat('en-IN', {
    style: 'currency',
    currency: 'INR',
    maximumFractionDigits: 0,
  }).format(n);
}

/**
 * Goals: what you are saving toward, and whether you are on pace.
 *
 * For each goal it shows how much of the target is saved and the monthly saving
 * needed from now to hit it by the date, computed against today and the return
 * you assumed. It is a tracker and a calculator, not advice: it says what the
 * arithmetic of your own numbers works out to, nothing more.
 */
export function GoalsPanel() {
  const [goals, setGoals] = useState<GoalProgress[]>([]);
  const [status, setStatus] = useState<string | null>(null);

  // Add-form fields.
  const [name, setName] = useState('');
  const [target, setTarget] = useState('');
  const [date, setDate] = useState('');
  const [ret, setRet] = useState('8');

  const load = useCallback(async () => {
    try {
      setGoals(await listGoals());
    } catch (err) {
      setStatus(`Could not load goals: ${String(err).replace(/^Error:\s*/, '')}`);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const add = useCallback(async () => {
    const amount = Number(target);
    const rate = Number(ret);
    if (!name.trim() || !(amount > 0) || !date) {
      setStatus('Enter a name, a target amount, and a target date.');
      return;
    }
    try {
      await addGoal(name.trim(), amount, date, Number.isFinite(rate) ? rate : 8);
      setName('');
      setTarget('');
      setDate('');
      setRet('8');
      setStatus(null);
      await load();
    } catch (err) {
      setStatus(`Could not add: ${String(err).replace(/^Error:\s*/, '')}`);
    }
  }, [name, target, date, ret, load]);

  const saveAmount = useCallback(
    async (id: number, value: string) => {
      const amount = Number(value);
      if (!Number.isFinite(amount) || amount < 0) return;
      try {
        await updateGoalAmount(id, amount);
        await load();
      } catch {
        // Leave the shown value; the next load reconciles.
      }
    },
    [load],
  );

  return (
    <div className="goals">
      <p className="money__note">
        Progress and the monthly saving needed are worked out from your numbers and
        today's date. A calculator, not advice.
      </p>

      {status && <p className="money__status">{status}</p>}

      <div className="goals__add">
        <input
          className="money__input"
          type="text"
          placeholder="Goal (e.g. House down payment)"
          value={name}
          onChange={(e) => setName(e.target.value)}
          aria-label="Goal name"
        />
        <input
          className="money__input money__input--qty"
          type="number"
          min="0"
          step="any"
          placeholder="Target ₹"
          value={target}
          onChange={(e) => setTarget(e.target.value)}
          aria-label="Target amount"
        />
        <input
          className="money__input money__input--date"
          type="date"
          value={date}
          onChange={(e) => setDate(e.target.value)}
          aria-label="Target date"
        />
        <input
          className="money__input money__input--rate"
          type="number"
          min="0"
          step="any"
          placeholder="Return %"
          value={ret}
          onChange={(e) => setRet(e.target.value)}
          title="Assumed annual return while saving"
          aria-label="Expected return percent"
        />
        <button className="money__add-btn" type="button" onClick={() => void add()} aria-label="Add goal">
          <Plus size={15} strokeWidth={2.5} aria-hidden="true" />
        </button>
      </div>

      {goals.length === 0 && <p className="money__empty">No goals yet. Add one above.</p>}

      <div className="goals__list">
        {goals.map((g) => (
          <div className="goal" key={g.id}>
            <div className="goal__head">
              <span className="goal__name">{g.name}</span>
              <span className="goal__target">{inr(g.targetAmount)}</span>
              <button
                className="money__remove"
                type="button"
                onClick={() => void removeGoal(g.id).then(load)}
                aria-label={`Remove ${g.name}`}
              >
                <X size={12} strokeWidth={2.5} aria-hidden="true" />
              </button>
            </div>

            <div className="goal__bar" role="img" aria-label={`${Math.round(g.percent)}% saved`}>
              <div className="goal__bar-fill" style={{ width: `${g.percent}%` }} />
            </div>

            <div className="goal__stats">
              <span className="goal__pct">{Math.round(g.percent)}% saved</span>
              <span className="goal__years">
                {g.yearsLeft > 0 ? `${g.yearsLeft.toFixed(1)} yrs left` : 'due'}
              </span>
              <span className="goal__need">
                {g.requiredMonthly === null
                  ? 'check the date'
                  : g.requiredMonthly <= 0
                    ? 'target reached'
                    : `${inr(g.requiredMonthly)}/mo to stay on track`}
              </span>
            </div>

            <label className="goal__saved">
              <span className="goal__saved-label">Saved so far</span>
              <span className="goal__saved-edit">
                <input
                  className="money__input money__input--qty"
                  type="number"
                  min="0"
                  step="any"
                  defaultValue={g.currentAmount}
                  onBlur={(e) => void saveAmount(g.id, e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
                  }}
                  aria-label={`Amount saved toward ${g.name}`}
                />
                <Check size={12} strokeWidth={2} aria-hidden="true" className="goal__saved-icon" />
              </span>
            </label>
          </div>
        ))}
      </div>
    </div>
  );
}
