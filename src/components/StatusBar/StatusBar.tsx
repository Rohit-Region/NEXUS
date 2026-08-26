import { useState, useEffect } from 'react';
import type { SystemStatus } from '../../types';
import './StatusBar.css';

function formatDateTime(date: Date): string {
  return date.toLocaleString('en-US', {
    weekday: 'short',
    month:   'short',
    day:     'numeric',
    hour:    '2-digit',
    minute:  '2-digit',
    second:  '2-digit',
    hour12:  false,
  });
}

export function StatusBar() {
  const status: SystemStatus = 'ONLINE';
  const [time, setTime] = useState<string>(() => formatDateTime(new Date()));

  useEffect(() => {
    const id = setInterval(() => {
      setTime(formatDateTime(new Date()));
    }, 1000);
    return () => clearInterval(id);
  }, []);

  return (
    <div className="status-bar" role="status" aria-live="polite">
      <div className="status-bar__indicator" aria-label={`System status: ${status}`}>
        <span className="status-bar__dot" aria-hidden="true" />
        <span className="status-bar__label">SYSTEM {status}</span>
      </div>
      <time className="status-bar__clock" dateTime={new Date().toISOString()}>
        {time}
      </time>
    </div>
  );
}
