import { useCallback, useEffect, useState } from 'react';
import { UserRound } from 'lucide-react';
import { getProfile, setProfileField } from '../../lib/nexus-db';
import { WellbeingSettings } from './WellbeingSettings';
import { PhoneApprovalSettings } from './PhoneApprovalSettings';
import { PhoneCallSettings } from './PhoneCallSettings';
import './MeScreen.css';

/**
 * One editable profile field.
 *
 * `area` renders a multi-line box for the fields that are really a few
 * sentences (goals, about); the rest are single-line.
 */
interface Field {
  key: string;
  label: string;
  hint: string;
  placeholder: string;
  area?: boolean;
}

interface Group {
  title: string;
  blurb: string;
  fields: Field[];
}

/**
 * The profile, grouped the way a person thinks about themselves, not the way
 * the table stores it. Every pillar reads these fields, so the hints say what
 * each one feeds rather than restating the label.
 */
const GROUPS: Group[] = [
  {
    title: 'Who you are',
    blurb: 'The basics NEXUS uses to address you and frame everything else.',
    fields: [
      { key: 'display_name', label: 'Name', hint: 'What NEXUS calls you.', placeholder: 'Rohit' },
      {
        key: 'role_current',
        label: 'Current role',
        hint: 'Where you are now.',
        placeholder: 'Frontend Engineer at Avetta',
      },
      {
        key: 'role_target',
        label: 'Target role',
        hint: 'What the career section aims you at.',
        placeholder: 'Senior Frontend Engineer',
      },
    ],
  },
  {
    title: 'Growth',
    blurb: 'Feeds the learning plans and the job-match scoring.',
    fields: [
      {
        key: 'skills',
        label: 'Skills',
        hint: 'What you can already do. Comma-separated.',
        placeholder: 'React, TypeScript, Rust, Tauri',
      },
      {
        key: 'learning_focus',
        label: 'Learning focus',
        hint: 'What you want to get better at.',
        placeholder: 'System design, Rust, interview prep',
      },
    ],
  },
  {
    title: 'Life & money',
    blurb: 'Feeds the money section: goals, savings targets, retirement.',
    fields: [
      {
        key: 'goals',
        label: 'Goals',
        hint: 'What you are working toward. One per line is fine.',
        placeholder: '₹20L for a house by 2030\nEmergency fund of 6 months',
        area: true,
      },
      {
        key: 'retirement_target_age',
        label: 'Target retirement age',
        hint: 'Used for the retirement projection.',
        placeholder: '55',
      },
    ],
  },
  {
    title: 'For applications',
    blurb:
      'Only for one-click copy when you fill in an application form. Never sent to the model or anywhere else.',
    fields: [
      { key: 'contact_email', label: 'Email', hint: 'Copied into application forms.', placeholder: 'you@example.com' },
      { key: 'contact_phone', label: 'Phone', hint: 'With country code.', placeholder: '+91 90000 00000' },
      {
        key: 'contact_linkedin',
        label: 'LinkedIn',
        hint: 'Your profile link.',
        placeholder: 'https://www.linkedin.com/in/your-name',
      },
      {
        key: 'contact_portfolio',
        label: 'Portfolio / GitHub',
        hint: 'A link that shows your work.',
        placeholder: 'https://github.com/your-name',
      },
    ],
  },
  {
    title: 'About you',
    blurb: 'Anything else worth NEXUS knowing. Written into the briefing context.',
    fields: [
      {
        key: 'about',
        label: 'About',
        hint: 'Context in your own words.',
        placeholder: 'Frontend engineer in India, into building side projects, wants financial independence.',
        area: true,
      },
    ],
  },
];

/**
 * Me: the one place NEXUS learns who it works for.
 *
 * This is the backbone of the whole assistant. Every pillar, news, money,
 * learning, career, the briefing, reads this profile so it personalises from a
 * single source of truth rather than re-asking. Nothing here is inferred;
 * what you type is all NEXUS ever claims to know about you.
 *
 * Fields save on blur, and the stored value is read back, so the field shows
 * exactly what was kept (a cleared field empties, an oversized one is refused
 * with a note).
 */
export function MeScreen() {
  const [values, setValues] = useState<Record<string, string>>({});
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    void getProfile()
      .then((p) => setValues(p))
      .catch(() => setStatus('Could not load your profile.'));
  }, []);

  const commit = useCallback(
    (key: string, label: string) => {
      const value = values[key] ?? '';
      void setProfileField(key, value)
        .then((saved) => {
          setValues((v) => ({ ...v, [key]: saved }));
          setStatus(saved ? `${label} saved.` : `${label} cleared.`);
        })
        .catch((err) => setStatus(`Could not save ${label}: ${String(err)}`));
    },
    [values],
  );

  const onChange = (key: string, next: string) =>
    setValues((v) => ({ ...v, [key]: next }));

  return (
    <div className="me-screen">
      <header className="me-screen__head">
        <UserRound size={18} strokeWidth={1.8} aria-hidden="true" />
        <div>
          <h1 className="me-screen__title">Me</h1>
          <p className="me-screen__subtitle">
            What NEXUS knows about you. Every section personalises from this.
          </p>
        </div>
      </header>

      {status && <p className="me-screen__status">{status}</p>}

      <div className="me-screen__groups">
        {GROUPS.map((group) => (
          <section className="me-group" key={group.title}>
            <div className="me-group__head">
              <h2 className="me-group__title">{group.title}</h2>
              <p className="me-group__blurb">{group.blurb}</p>
            </div>
            <div className="me-group__fields">
              {group.fields.map((field) => (
                <label className="me-field" key={field.key}>
                  <span className="me-field__label">{field.label}</span>
                  {field.area ? (
                    <textarea
                      className="me-field__input me-field__input--area"
                      rows={3}
                      placeholder={field.placeholder}
                      value={values[field.key] ?? ''}
                      onChange={(e) => onChange(field.key, e.target.value)}
                      onBlur={() => commit(field.key, field.label)}
                    />
                  ) : (
                    <input
                      className="me-field__input"
                      type="text"
                      placeholder={field.placeholder}
                      value={values[field.key] ?? ''}
                      onChange={(e) => onChange(field.key, e.target.value)}
                      onBlur={() => commit(field.key, field.label)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
                      }}
                    />
                  )}
                  <span className="me-field__hint">{field.hint}</span>
                </label>
              ))}
            </div>
          </section>
        ))}
        <WellbeingSettings />
        <PhoneCallSettings />
        <PhoneApprovalSettings />
      </div>
    </div>
  );
}
