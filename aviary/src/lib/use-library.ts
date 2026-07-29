import { useCallback, useEffect, useState } from "react";
import { scanLibrary, type LibrarySnapshot } from "@/lib/api";

type State = {
  data: LibrarySnapshot | null;
  error: string | null;
  loading: boolean;
};

/**
 * Loads the real library from disk. Kept deliberately simple — the indexer
 * (with file watching and incremental updates) replaces this later; for now a
 * full scan is well under the frame budget.
 */
export function useLibrary() {
  const [state, setState] = useState<State>({
    data: null,
    error: null,
    loading: true,
  });

  const refresh = useCallback(async () => {
    setState((s) => ({ ...s, loading: true, error: null }));
    try {
      const data = await scanLibrary();
      setState({ data, error: null, loading: false });
    } catch (e) {
      setState({
        data: null,
        error: e instanceof Error ? e.message : String(e),
        loading: false,
      });
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { ...state, refresh };
}
