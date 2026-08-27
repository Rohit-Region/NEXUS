import './StatTile.css';

interface StatTileProps {
  label: string;
  value: number;
  detail?: string;
  accent?: boolean;
}

/** Presentational. Renders 0 as "0"; a falsy blank would hide a real zero. */
export function StatTile({ label, value, detail, accent = false }: StatTileProps) {
  return (
    <div className={`stat-tile${accent ? ' stat-tile--accent' : ''}`}>
      <span className="stat-tile__label">{label}</span>
      <span className="stat-tile__value">{value}</span>
      {detail !== undefined && <span className="stat-tile__detail">{detail}</span>}
    </div>
  );
}
