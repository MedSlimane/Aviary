import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import {
  copyDiagnostics,
  failureFrom,
  installGlobalErrorHandlers,
} from "@/lib/diagnostics";
import {
  reportFrontendFailure,
  type FrontendFailure,
} from "@/lib/api";

type Props = { children: ReactNode };
type State = {
  failure: FrontendFailure | null;
  copyState: "idle" | "copying" | "copied" | "manual";
  manualText: string | null;
};

const initialState: State = {
  failure: null,
  copyState: "idle",
  manualText: null,
};

/**
 * A render failure must not turn the window black. Global listeners also cover
 * the event-handler errors and rejected promises React boundaries cannot see.
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = initialState;
  private removeGlobalHandlers: (() => void) | undefined;

  static getDerivedStateFromError(error: Error): State {
    return {
      failure: failureFrom("react", error),
      copyState: "idle",
      manualText: null,
    };
  }

  componentDidMount() {
    this.removeGlobalHandlers = installGlobalErrorHandlers((failure) => {
      this.setState({ failure, copyState: "idle", manualText: null });
    });
  }

  componentWillUnmount() {
    this.removeGlobalHandlers?.();
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    const failure = failureFrom(
      "react",
      error,
      undefined,
      info.componentStack ?? undefined,
    );
    reportFrontendFailure(failure);
    this.setState({ failure });
  }

  private copy = async () => {
    const { failure } = this.state;
    if (!failure) return;
    this.setState({ copyState: "copying", manualText: null });
    const result = await copyDiagnostics(failure);
    this.setState({
      copyState: result.copied ? "copied" : "manual",
      manualText: result.copied ? null : result.text,
    });
  };

  private retry = () => this.setState(initialState);

  render() {
    const { failure, copyState, manualText } = this.state;
    if (!failure) return this.props.children;

    return (
      <div className="flex h-screen flex-col items-center justify-center gap-4 overflow-y-auto bg-background p-8 text-foreground">
        <div className="w-full max-w-[620px] space-y-3 text-center">
          <h1 className="text-lg font-semibold">Something went wrong</h1>
          <p className="text-[13px] text-muted-foreground">
            Aviary recorded local diagnostics when file logging was available.
            Nothing is uploaded — copy the report if you want to share it, and
            review it first.
          </p>
          <pre className="max-h-[180px] overflow-auto rounded-lg border border-border bg-card p-3 text-left font-mono text-[11px] text-destructive">
            {failure.message}
          </pre>
          {manualText && (
            <div className="space-y-2 text-left">
              <p className="text-[11px] text-muted-foreground">
                Clipboard access was unavailable. Select and copy the report below.
              </p>
              <textarea
                readOnly
                value={manualText}
                onFocus={(event) => event.currentTarget.select()}
                className="h-[180px] w-full resize-y rounded-lg border border-border bg-card p-3 font-mono text-[10px] text-foreground outline-none"
              />
            </div>
          )}
        </div>
        <div className="flex flex-wrap justify-center gap-2">
          <Button
            size="sm"
            disabled={copyState === "copying"}
            onClick={() => void this.copy()}
          >
            {copyState === "copying"
              ? "Preparing…"
              : copyState === "copied"
                ? "Copied"
                : "Copy diagnostics"}
          </Button>
          <Button size="sm" variant="outline" onClick={this.retry}>
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
