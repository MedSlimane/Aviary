import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import prettyBytes from "pretty-bytes";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Progress } from "@/components/ui/progress";
import { notify } from "@/lib/notify";
import {
  checkForUpdate,
  checkForUpdateOnce,
  installAvailableUpdate,
  requestRelaunch,
  type UpdateCheckResult,
  type UpdateMetadata,
  type UpdateProgress,
} from "@/lib/updater";

type UpdatePhase =
  | "idle"
  | "checking"
  | "current"
  | "available"
  | "installing"
  | "relaunching"
  | "relaunch-required"
  | "disabled"
  | "error";

type UpdateContextValue = {
  phase: UpdatePhase;
  currentVersion: string | null;
  available: UpdateMetadata | null;
  error: string | null;
  checkNow: () => Promise<void>;
  showAvailable: () => void;
};

const UpdateContext = createContext<UpdateContextValue | null>(null);

function stateFromCheck(result: UpdateCheckResult): {
  phase: UpdatePhase;
  currentVersion: string | null;
  available: UpdateMetadata | null;
  error: string | null;
} {
  switch (result.status) {
    case "available":
      return {
        phase: "available",
        currentVersion: result.currentVersion,
        available: result,
        error: null,
      };
    case "disabled-in-development":
      return {
        phase: "disabled",
        currentVersion: result.currentVersion,
        available: null,
        error: null,
      };
    case "error":
      return {
        phase: "error",
        currentVersion: result.currentVersion,
        available: null,
        error: result.message,
      };
    case "current":
      return {
        phase: "current",
        currentVersion: result.currentVersion,
        available: null,
        error: null,
      };
  }
}

export function UpdateProvider({ children }: { children: ReactNode }) {
  const [phase, setPhase] = useState<UpdatePhase>("idle");
  const [currentVersion, setCurrentVersion] = useState<string | null>(null);
  const [available, setAvailable] = useState<UpdateMetadata | null>(null);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [promptOpen, setPromptOpen] = useState(false);
  const checkGeneration = useRef(0);

  const applyCheck = useCallback((result: UpdateCheckResult, manual: boolean) => {
    const next = stateFromCheck(result);
    setPhase(next.phase);
    setCurrentVersion(next.currentVersion);
    setAvailable(next.available);
    setError(next.error);
    setProgress(null);

    if (result.status === "available") {
      setPromptOpen(true);
    } else if (manual && result.status === "current") {
      notify("Aviary is up to date", {
        description: `Version ${result.currentVersion} is the newest alpha.`,
      });
    } else if (manual && result.status === "disabled-in-development") {
      notify("Update checks are disabled", {
        description: "Development builds do not contact the release feed.",
      });
    } else if (manual && result.status === "error") {
      notify("Could not check for updates", { description: result.message });
    }
  }, []);

  useEffect(() => {
    let active = true;
    const generation = ++checkGeneration.current;
    void checkForUpdateOnce().then((result) => {
      if (active && generation === checkGeneration.current) {
        applyCheck(result, false);
      }
    });
    return () => {
      active = false;
    };
  }, [applyCheck]);

  const checkNow = useCallback(async () => {
    const generation = ++checkGeneration.current;
    setPhase("checking");
    setError(null);
    const result = await checkForUpdate();
    if (generation === checkGeneration.current) applyCheck(result, true);
  }, [applyCheck]);

  const install = useCallback(async () => {
    if (!available) return;
    ++checkGeneration.current;
    setPhase("installing");
    setError(null);
    setProgress(null);

    const result = await installAvailableUpdate({
      expectedVersion: available.version,
      onProgress: setProgress,
    });

    switch (result.status) {
      case "changed":
        setPhase("available");
        setAvailable(result);
        setProgress(null);
        notify("A newer update is available", {
          description: `Review version ${result.version} before installing it.`,
        });
        break;
      case "current":
        setPhase("current");
        setCurrentVersion(result.currentVersion);
        setAvailable(null);
        setProgress(null);
        setPromptOpen(false);
        notify("Aviary is already up to date");
        break;
      case "disabled-in-development":
        setPhase("disabled");
        setCurrentVersion(result.currentVersion);
        setAvailable(null);
        setProgress(null);
        setPromptOpen(false);
        break;
      case "error":
        setPhase("error");
        setError(result.message);
        setProgress(null);
        break;
      case "installed-relaunch-required":
        setPhase("relaunch-required");
        setAvailable(result);
        setError(`The update was installed, but Aviary could not relaunch: ${result.message}`);
        setProgress(null);
        break;
      case "installed":
        // A successful relaunch normally replaces this process before the
        // promise settles. Keep the prompt honest if the platform returns.
        setPhase("current");
        setCurrentVersion(result.version);
        setAvailable(null);
        setProgress(null);
        setPromptOpen(false);
        break;
    }
  }, [available]);

  const retryRelaunch = useCallback(async () => {
    setPhase("relaunching");
    setError(null);
    const relaunchError = await requestRelaunch();
    if (relaunchError) {
      setPhase("relaunch-required");
      setError(`Aviary could not relaunch: ${relaunchError}`);
      return;
    }
    setPromptOpen(false);
  }, []);

  const showAvailable = useCallback(() => {
    if (available) setPromptOpen(true);
  }, [available]);

  const downloaded = progress && "downloadedBytes" in progress
    ? progress.downloadedBytes
    : 0;
  const total = progress?.contentLength;
  const percentage = total && total > 0
    ? Math.min(100, (downloaded / total) * 100)
    : null;

  return (
    <UpdateContext.Provider
      value={{ phase, currentVersion, available, error, checkNow, showAvailable }}
    >
      {children}
      <AlertDialog
        open={promptOpen}
        onOpenChange={(open) => {
          if (phase !== "installing" && phase !== "relaunching") {
            setPromptOpen(open);
          }
        }}
      >
        <AlertDialogContent className="max-w-[430px] sm:max-w-[430px]">
          <AlertDialogHeader>
            <AlertDialogTitle>
              {available ? `Aviary ${available.version} is available` : "Aviary update"}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {phase === "installing"
                ? "Downloading the signed update. Aviary will relaunch when installation finishes."
                : phase === "relaunch-required" || phase === "relaunching"
                  ? "The signed update is installed. Relaunch Aviary to start using the new version."
                : `You are running ${currentVersion ?? "an earlier version"}. Updates are signed and delivered from GitHub Releases.`}
            </AlertDialogDescription>
          </AlertDialogHeader>

          {available?.notes && phase !== "installing" && phase !== "relaunching" && (
            <pre className="max-h-[150px] overflow-auto whitespace-pre-wrap rounded-lg border border-border bg-card p-3 font-sans text-[12px] leading-relaxed text-muted-foreground">
              {available.notes}
            </pre>
          )}

          {phase === "installing" && (
            <div className="space-y-2">
              {percentage === null ? (
                <div className="h-1 overflow-hidden rounded-full bg-muted">
                  <div className="av-gradient-fill-live h-full w-full animate-pulse rounded-full" />
                </div>
              ) : (
                <Progress value={percentage} />
              )}
              <p className="font-mono text-[10px] text-tertiary">
                {downloaded > 0 ? prettyBytes(downloaded) : "Starting download"}
                {total ? ` of ${prettyBytes(total)}` : ""}
              </p>
            </div>
          )}

          {error && (
            <p className="rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-[12px] text-destructive">
              {error}
            </p>
          )}

          <AlertDialogFooter>
            <AlertDialogCancel
              disabled={phase === "installing" || phase === "relaunching"}
            >
              {phase === "error" || phase === "relaunch-required" ? "Close" : "Later"}
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={!available || phase === "installing" || phase === "relaunching"}
              onClick={(event) => {
                event.preventDefault();
                if (phase === "relaunch-required") void retryRelaunch();
                else void install();
              }}
            >
              {phase === "installing"
                ? "Installing…"
                : phase === "relaunching"
                  ? "Relaunching…"
                  : phase === "relaunch-required"
                    ? "Relaunch Aviary"
                : phase === "error"
                  ? "Try again"
                  : "Install and relaunch"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </UpdateContext.Provider>
  );
}

export function useUpdater(): UpdateContextValue {
  const value = useContext(UpdateContext);
  if (!value) throw new Error("useUpdater must be used inside UpdateProvider");
  return value;
}
