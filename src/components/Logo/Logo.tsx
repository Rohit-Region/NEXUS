import { Terminal } from 'lucide-react';
import './Logo.css';

export function Logo() {
  return (
    <div className="logo" aria-label="NEXUS">
      <div className="logo__icon" aria-hidden="true">
        <Terminal size={16} strokeWidth={2} />
      </div>
      <span className="logo__wordmark">
        NE<span className="accent">X</span>US
      </span>
    </div>
  );
}
