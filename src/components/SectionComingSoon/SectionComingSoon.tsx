import type { ReactNode } from 'react';
import './SectionComingSoon.css';

interface Props {
  icon: ReactNode;
  title: string;
  blurb: string;
  /** What this section will hold once built, so the placeholder is honest
   *  about what is coming rather than a bare "coming soon". */
  planned: string[];
}

/**
 * A placeholder for a life-area section that is planned but not yet built.
 *
 * It exists so the new sectioned navigation is real and navigable from day one
 * while the sections fill in one at a time. It states plainly what will live
 * here, so it reads as a roadmap, not a dead end.
 */
export function SectionComingSoon({ icon, title, blurb, planned }: Props) {
  return (
    <div className="soon">
      <div className="soon__card">
        <span className="soon__icon" aria-hidden="true">
          {icon}
        </span>
        <h1 className="soon__title">{title}</h1>
        <p className="soon__blurb">{blurb}</p>
        <span className="soon__tag">Being built</span>
        <ul className="soon__list">
          {planned.map((item) => (
            <li className="soon__item" key={item}>
              {item}
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
