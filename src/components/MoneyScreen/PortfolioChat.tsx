import { useState } from 'react';
import { MessageSquareText, Send } from 'lucide-react';
import { portfolioChat, portfolioChatApply, type PortfolioPreview } from '../../lib/nexus-db';

interface Props {
  /** Called after changes are applied, to re-price the portfolio. */
  onApplied: () => void;
}

const EXAMPLES = [
  'bought 5 goldbees in icici direct at 117.25',
  'MUP units 156.1421 nav 9.5085',
  'MVF nav 9.70',
  'paid 8000 tata aia premium',
  'goldbees invested 586.25',
  'sold 2 goldbees',
];

/**
 * Tell NEXUS about the portfolio in plain words. Nothing changes until you
 * have seen what NEXUS understood and pressed Apply.
 */
export function PortfolioChat({ onApplied }: Props) {
  const [text, setText] = useState('');
  const [preview, setPreview] = useState<PortfolioPreview | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const read = async () => {
    if (!text.trim()) return;
    setResult(null);
    setPreview(await portfolioChat(text));
  };

  const apply = async () => {
    if (!preview || preview.actions.length === 0) return;
    setBusy(true);
    try {
      const done = await portfolioChatApply(preview.actions);
      setResult(`Done: ${done.join('; ')}.`);
      setPreview(null);
      setText('');
      onApplied();
    } catch (err) {
      setResult(String(err).replace(/^Error:\s*/, ''));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="pchat" aria-label="Tell NEXUS about your portfolio">
      <div className="pchat__head">
        <MessageSquareText size={13} strokeWidth={2} aria-hidden="true" />
        <span>Tell NEXUS what changed</span>
      </div>
      <form
        className="pchat__form"
        onSubmit={(e) => {
          e.preventDefault();
          void read();
        }}
      >
        <textarea
          className="money__input pchat__input"
          rows={2}
          placeholder={EXAMPLES.slice(0, 2).join('\n')}
          value={text}
          onChange={(e) => {
            setText(e.target.value);
            setPreview(null);
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              void read();
            }
          }}
          aria-label="What changed in your portfolio"
        />
        <button className="pchat__send" type="submit" disabled={!text.trim()} aria-label="Read it">
          <Send size={14} strokeWidth={2} aria-hidden="true" />
        </button>
      </form>

      {preview && (
        <div className="pchat__preview" role="status">
          {preview.lines.length > 0 && (
            <>
              <p className="pchat__label">NEXUS will:</p>
              <ul className="pchat__lines">
                {preview.lines.map((l) => (
                  <li key={l}>{l}</li>
                ))}
              </ul>
            </>
          )}
          {preview.notUnderstood.length > 0 && (
            <p className="pchat__miss">
              Didn't understand: {preview.notUnderstood.map((m) => `"${m}"`).join(', ')}. Try one of the examples below.
            </p>
          )}
          <div className="pchat__actions">
            {preview.actions.length > 0 && (
              <button className="pchat__apply" type="button" onClick={() => void apply()} disabled={busy}>
                {busy ? 'Applying…' : 'Apply'}
              </button>
            )}
            <button className="money__link pchat__cancel" type="button" onClick={() => setPreview(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {result && <p className="pchat__result">{result}</p>}

      <details className="pchat__help">
        <summary>What can I say?</summary>
        <ul>
          {EXAMPLES.map((e) => (
            <li key={e}>
              <button className="pchat__example" type="button" onClick={() => setText(e)}>
                {e}
              </button>
            </li>
          ))}
        </ul>
        <p>One change per line. ULIP fund codes: MUP, NAP, MVF (and the other Tata AIA pension funds).</p>
      </details>
    </section>
  );
}
