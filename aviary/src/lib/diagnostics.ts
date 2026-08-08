import {
  collectDiagnostics,
  reportFrontendFailure,
  type FrontendFailure,
} from "@/lib/api";
import { homeDir } from "@tauri-apps/api/path";

const LOCAL_FIELD_CHARS = 16_000;

function clipped(value: string): string {
  return value.length > LOCAL_FIELD_CHARS
    ? `${value.slice(0, LOCAL_FIELD_CHARS)}\n…[truncated]`
    : value;
}

export function failureFrom(
  source: FrontendFailure["source"],
  error: unknown,
  context?: string,
  componentStack?: string,
): FrontendFailure {
  return {
    source,
    context,
    message: clipped(error instanceof Error ? error.message : String(error)),
    stack: error instanceof Error && error.stack ? clipped(error.stack) : undefined,
    componentStack: componentStack ? clipped(componentStack) : undefined,
  };
}

/**
 * React boundaries do not see event-handler errors or rejected promises. The
 * callback lets the root boundary show the same honest recovery surface for
 * those failures while this module persists the details locally.
 */
export function installGlobalErrorHandlers(
  onFailure: (failure: FrontendFailure) => void,
): () => void {
  const onError = (event: ErrorEvent) => {
    if (!event.error && !event.message) return;
    const context = [event.filename, event.lineno, event.colno]
      .filter(Boolean)
      .join(":");
    const failure = failureFrom(
      "window",
      event.error ?? event.message,
      context || undefined,
    );
    reportFrontendFailure(failure);
    onFailure(failure);
  };

  const onUnhandledRejection = (event: PromiseRejectionEvent) => {
    const failure = failureFrom("unhandled-rejection", event.reason);
    reportFrontendFailure(failure);
    onFailure(failure);
  };

  window.addEventListener("error", onError);
  window.addEventListener("unhandledrejection", onUnhandledRejection);
  return () => {
    window.removeEventListener("error", onError);
    window.removeEventListener("unhandledrejection", onUnhandledRejection);
  };
}

async function redactLocalPaths(value: string): Promise<string> {
  let redacted = value;
  try {
    const home = await homeDir();
    if (home) redacted = redacted.split(home).join("~");
  } catch {
    // Generic patterns below still prevent a collection failure from exposing
    // the common local-account path shapes.
  }
  return redacted
    .replace(/\/Users\/[^/\s]+/g, "~")
    .replace(/\/home\/[^/\s]+/g, "~")
    .replace(/[A-Za-z]:\\Users\\[^\\\s]+/g, "~");
}

async function frontendOnlyBundle(
  failure: FrontendFailure | undefined,
  collectionError: unknown,
): Promise<string> {
  const lines = [
    "Aviary diagnostics (frontend fallback)",
    `Generated: ${new Date().toISOString()}`,
    `Log collection failed: ${
      collectionError instanceof Error
        ? collectionError.message
        : String(collectionError)
    }`,
  ];
  if (failure) {
    lines.push(
      "",
      "Reported failure",
      `Source: ${failure.source}`,
      ...(failure.context ? [`Context: ${failure.context}`] : []),
      `Message: ${failure.message}`,
      ...(failure.stack ? ["Stack:", failure.stack] : []),
      ...(failure.componentStack
        ? ["React component stack:", failure.componentStack]
        : []),
    );
  }
  return redactLocalPaths(lines.join("\n"));
}

export type DiagnosticsCopyResult = {
  text: string;
  copied: boolean;
  logsDir?: string;
};

/** Returns the text even when clipboard access is unavailable for manual copy. */
export async function copyDiagnostics(
  failure?: FrontendFailure,
): Promise<DiagnosticsCopyResult> {
  let text: string;
  let logsDir: string | undefined;
  try {
    const bundle = await collectDiagnostics(failure);
    text = bundle.text;
    logsDir = bundle.logsDir;
  } catch (error) {
    text = await frontendOnlyBundle(failure, error);
  }

  try {
    if (!navigator.clipboard?.writeText) {
      return { text, copied: false, logsDir };
    }
    await navigator.clipboard.writeText(text);
    return { text, copied: true, logsDir };
  } catch {
    return { text, copied: false, logsDir };
  }
}
