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
  /** The pack an entry ships as part of — plugin name, or project name. */
  group: string | null;
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

export type EntryContent = {
  raw: string;
  body: string;
  frontmatter: string | null;
  /** Real count via tiktoken (o200k_base), not a byte heuristic. */
  tokens: number;
  /** Content hash at read time; sent back on save to detect outside edits. */
  hash: string;
};

export type WriteOutcome =
  | { status: "written"; hash: string; snapshot: string }
  | { status: "conflict"; diskHash: string; diskContent: string };

type RawWrite =
  | { status: "written"; hash: string; snapshot: string }
  | { status: "conflict"; disk_hash: string; disk_content: string };

export async function writeEntry(
  path: string,
  content: string,
  expectedHash: string,
  force = false,
): Promise<WriteOutcome> {
  const r = await invoke<RawWrite>("write_entry", {
    path,
    content,
    expectedHash,
    force,
  });
  return r.status === "written"
    ? r
    : { status: "conflict", diskHash: r.disk_hash, diskContent: r.disk_content };
}

export function readEntry(path: string): Promise<EntryContent> {
  return invoke<EntryContent>("read_entry", { path });
}

export function countTokens(path: string): Promise<number> {
  return invoke<number>("count_tokens", { path });
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

export type Candidate = {
  name: string;
  path: string;
  runners: string[];
  markers: string[];
  registered: boolean;
};

type RawDiscovery = { candidates: Candidate[]; scanned_ms: number; roots: string[] };
export type Discovery = { candidates: Candidate[]; scannedMs: number; roots: string[] };

export async function discoverProjects(): Promise<Discovery> {
  const raw = await invoke<RawDiscovery>("discover_projects");
  return { candidates: raw.candidates, scannedMs: raw.scanned_ms, roots: raw.roots };
}
