import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "@/components/ui/button";

type Props = { children: ReactNode };
type State = { error: Error | null };

/**
 * Without this, any component that throws unmounts the entire tree and the
 * window goes black with no indication of what happened — which is exactly
 * what a misplaced Base UI menu label did.
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Aviary crashed:", error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    return (
      <div className="flex h-screen flex-col items-center justify-center gap-4 bg-background p-8 text-foreground">
        <div className="max-w-[560px] space-y-3 text-center">
          <h1 className="text-lg font-semibold">Something broke</h1>
          <p className="text-[13px] text-muted-foreground">
            The view failed to render. The error is below and in the console.
          </p>
          <pre className="max-h-[220px] overflow-auto rounded-lg border border-border bg-card p-3 text-left font-mono text-[11px] text-destructive">
            {error.message}
          </pre>
        </div>
        <div className="flex gap-2">
          <Button size="sm" onClick={() => this.setState({ error: null })}>
            Try again
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={() => window.location.reload()}
          >
            Reload
          </Button>
        </div>
      </div>
    );
  }
}
