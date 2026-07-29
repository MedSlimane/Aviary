import { invoke } from "@tauri-apps/api/core";

export type Runner = "claude-code" | "codex";
export type Kind = "skill" | "agent" | "command" | "prompt" | "memory";
export type Source = "user" | "plugin" | "project";

export type Entry = {
  id: string;
  name: string;
  description: string;
  kind: Kind;
  source: Source;
  runners: Runner[];
  path: string;
  realPath: string;
  project: string | null;
  meta: Record<string, string>;
  bytes: number;
  modified: number;
};

export type Project = { name: string; path: string };

export type RunnerStatus = {
  runner: Runner;
  label: string;
  root: string;
  detected: boolean;
};

export type LibrarySnapshot = {
  entries: Entry[];
  projects: Project[];
  runners: RunnerStatus[];
  scannedMs: number;
};

/** Rust serialises snake_case; normalise the few fields that differ. */
type RawEntry = Omit<Entry, "realPath"> & { real_path: string };
type RawSnapshot = Omit<LibrarySnapshot, "entries" | "scannedMs"> & {
  entries: RawEntry[];
  scanned_ms: number;
};

export const RUNNER_LABEL: Record<Runner, string> = {
  "claude-code": "Claude Code",
  codex: "Codex",
};

export async function scanLibrary(): Promise<LibrarySnapshot> {
  const raw = await invoke<RawSnapshot>("scan_library");
  return {
    ...raw,
    scannedMs: raw.scanned_ms,
    entries: raw.entries.map(({ real_path, ...e }) => ({
      ...e,
      realPath: real_path,
    })),
  };
}

export function readEntry(path: string): Promise<string> {
  return invoke<string>("read_entry", { path });
}

export function listProjects(): Promise<Project[]> {
  return invoke<Project[]>("list_projects");
}

export function addProject(name: string, path: string): Promise<Project[]> {
  return invoke<Project[]>("add_project", { name, path });
}

export function removeProject(path: string): Promise<Project[]> {
  return invoke<Project[]>("remove_project", { path });
}
