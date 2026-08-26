import { Logo } from '../Logo/Logo';
import { StatusBar } from '../StatusBar/StatusBar';
import { Dashboard } from '../Dashboard/Dashboard';
import { CommandBar } from '../CommandBar/CommandBar';
import './AppShell.css';

export function AppShell() {
  return (
    <div className="app-shell">
      <header className="app-shell__header">
        <Logo />
        <StatusBar />
      </header>

      <Dashboard />

      <CommandBar />
    </div>
  );
}
