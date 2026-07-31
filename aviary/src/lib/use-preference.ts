import { useCallback, useEffect, useState } from "react";
import { getPreference, setPreference } from "@/lib/api";

/**
 * A boolean preference backed by SQLite (`~/.aviary/data.db`).
 *
 * Reads once on mount and writes through on every change, so a setting
 * survives relaunch — the previous implementation held these in `useState`
 * alone, which silently reset every toggle when the window closed.
 */
export function useBoolPreference(key: string, fallback = false) {
  const [value, setValue] = useState(fallback);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    let alive = true;
    getPreference(key)
      .then((raw) => {
        if (!alive) return;
        if (raw !== null) setValue(raw === "true");
        setLoaded(true);
      })
      .catch(() => setLoaded(true));
    return () => {
      alive = false;
    };
  }, [key]);

  const update = useCallback(
    (next: boolean) => {
      // Optimistic: the UI must not wait on a disk write to toggle.
      setValue(next);
      void setPreference(key, String(next));
    },
    [key],
  );

  return [value, update, loaded] as const;
}
