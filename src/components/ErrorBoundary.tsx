// Contains render errors of one view (Library / Develop / Export) so the rest of the app keeps working.
import { Component, type ErrorInfo, type ReactNode } from "react";
import { AlertTriangle } from "lucide-react";

interface Props {
  /** Shown in the fallback and the log: "Library", "Develop", "Export". */
  view: string;
  children: ReactNode;
  /** Called after "Reload view" (remount the subtree, refetch data). */
  onReload?: () => void;
  /** Fill the parent absolutely (Develop / overlays) instead of flowing in the layout. */
  overlay?: boolean;
  /** Extra action, e.g. "Back to grid". */
  onExit?: () => void;
}

interface State {
  error: Error | null;
  epoch: number;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, epoch: 0 };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`[sieve] ${this.props.view} view crashed:`, error, info.componentStack);
  }

  reload = () => {
    this.setState((s) => ({ error: null, epoch: s.epoch + 1 }));
    this.props.onReload?.();
  };

  render() {
    const { error, epoch } = this.state;
    if (!error) return <div key={epoch} className="contents">{this.props.children}</div>;
    return (
      <div
        role="alert"
        data-testid={`error-boundary-${this.props.view.toLowerCase()}`}
        className={`${this.props.overlay ? "absolute inset-0 z-20 bg-neutral-950" : "flex-1"} flex flex-col items-center justify-center gap-3 p-6 text-center`}
      >
        <AlertTriangle className="size-8 text-amber-400" />
        <h2 className="text-base font-semibold text-neutral-100">Something went wrong in {this.props.view}</h2>
        <p className="max-w-md break-words text-xs text-neutral-400" data-testid="error-boundary-message">
          {error.message || String(error)}
        </p>
        <div className="flex gap-2">
          <button onClick={this.reload} data-testid="error-boundary-reload" className="rounded bg-sky-700 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-600">
            Reload view
          </button>
          {this.props.onExit && (
            <button onClick={this.props.onExit} data-testid="error-boundary-exit" className="rounded bg-neutral-800 px-3 py-1.5 text-sm hover:bg-neutral-700">
              Back to grid
            </button>
          )}
        </div>
        <p className="text-[11px] text-neutral-400">Your catalog and edits are safe. The error was logged to the console.</p>
      </div>
    );
  }
}
