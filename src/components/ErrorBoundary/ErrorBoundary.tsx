import { Component, type ErrorInfo, type ReactNode } from 'react';
import './ErrorBoundary.css';

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
}

/**
 * The last thing between a render-time throw and a black screen.
 *
 * Without this, one component throwing during render unmounts the entire
 * React tree, and all that is left is the window's own background: a black
 * rectangle with no title, no error, nothing to act on. That is the worst
 * possible failure because it is indistinguishable from the app being dead.
 *
 * With it, the same throw shows what broke and offers the one action that
 * usually helps, a reload, so a bad render is a legible fault instead of a
 * blank window.
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Kept in the console so a black-screen report has a stack behind it.
    // eslint-disable-next-line no-console
    console.error('NEXUS render error:', error, info.componentStack);
  }

  render() {
    if (!this.state.error) {
      return this.props.children;
    }
    return (
      <div className="boundary" role="alert">
        <div className="boundary__mark" aria-hidden="true">
          &gt;_
        </div>
        <h1 className="boundary__title">NEXUS hit a rendering error</h1>
        <p className="boundary__detail">{this.state.error.message}</p>
        <button
          className="boundary__reload"
          type="button"
          onClick={() => window.location.reload()}
        >
          Reload
        </button>
      </div>
    );
  }
}
