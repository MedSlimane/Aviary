import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  listenLibraryUpdates,
  scanLibrary,
  type LibrarySnapshot,
} from "@/lib/api";

type State = {
  data: LibrarySnapshot | null;
  error: string | null;
  loading: boolean;
};

type LibraryContextValue = State & {
  /** Forces a real filesystem walk; the ordinary initial load uses cache. */
  refresh: () => Promise<LibrarySnapshot>;
};

const LibraryContext = createContext<LibraryContextValue | null>(null);

/**
 * One app-wide library subscription.
 *
 * The listener is installed before the cached invoke. If startup revalidation
 * commits before registration, the invoke sees it; if it commits afterward,
 * the event sees it. A revision guard prevents a slower invoke response from
 * replacing a newer event snapshot.
 */
export function LibraryProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<State>({
    data: null,
    error: null,
    loading: true,
  });
  const revision = useRef(0);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    const start = async () => {
      try {
        const stop = await listenLibraryUpdates((update) => {
          if (disposed || update.revision <= revision.current) return;
          revision.current = update.revision;
          setState({ data: update.snapshot, error: null, loading: false });
        });
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
      } catch {
        // A native listener failure must not hide the cached library. Manual
        // refresh remains available even when live updates are not.
      }

      const beforeLoad = revision.current;
      try {
        const data = await scanLibrary();
        if (!disposed && revision.current === beforeLoad) {
          setState({ data, error: null, loading: false });
        }
      } catch (error) {
        if (disposed || revision.current !== beforeLoad) return;
        setState({
          data: null,
          error: error instanceof Error ? error.message : String(error),
          loading: false,
        });
      }
    };

    void start();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const refresh = useCallback(async () => {
    const beforeLoad = revision.current;
    setState((current) => ({ ...current, loading: true, error: null }));
    try {
      const data = await scanLibrary(true);
      // The backend normally emits this same snapshot before the invoke
      // resolves. This fallback covers an absent webview listener without
      // allowing an older response to overwrite a later watcher revision.
      if (revision.current === beforeLoad) {
        setState({ data, error: null, loading: false });
      }
      return data;
    } catch (error) {
      if (revision.current === beforeLoad) {
        setState((current) => ({
          ...current,
          error: error instanceof Error ? error.message : String(error),
          loading: false,
        }));
      }
      throw error;
    }
  }, []);

  return (
    <LibraryContext.Provider value={{ ...state, refresh }}>
      {children}
    </LibraryContext.Provider>
  );
}

export function useLibrary(): LibraryContextValue {
  const value = useContext(LibraryContext);
  if (!value) {
    throw new Error("useLibrary must be used inside LibraryProvider");
  }
  return value;
}
