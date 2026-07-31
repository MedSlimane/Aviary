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

/**
 * Scans are cached in SQLite. Called without `fresh` they return the last
 * snapshot immediately — a cold launch paints real content instead of a
 * spinner. Pass `fresh` to force a walk of the filesystem.
 */
export async function scanLibrary(fresh = false): Promise<LibrarySnapshot> {
  const raw = await invoke<RawSnapshot>("scan_library", { fresh });
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

export async function discoverProjects(fresh = false): Promise<Discovery> {
  const raw = await invoke<RawDiscovery>("discover_projects", { fresh });
  return { candidates: raw.candidates, scannedMs: raw.scanned_ms, roots: raw.roots };
}

export type Transport = "stdio" | "http" | "sse";
export type McpSource = "user" | "plugin" | "project";

export type McpServer = {
  name: string;
  transport: Transport;
  command: string | null;
  args: string[];
  url: string | null;
  /** Names only — values are never read, so secrets cannot surface here. */
  envKeys: string[];
  source: McpSource;
  origin: string | null;
  runners: Runner[];
  enabled: boolean;
  configPath: string;
};

type RawServer = Omit<McpServer, "envKeys" | "configPath"> & {
  env_keys: string[];
  config_path: string;
};

export type McpSnapshot = { servers: McpServer[]; scannedMs: number };

// ----------------------------------------------------------------- media ---

export type MediaItem = {
  /** sha256 of the bytes — the identity, and stable across renames. */
  hash: string;
  kind: "image" | "video" | "file";
  ext: string;
  bytes: number;
  width: number | null;
  height: number | null;
  orientation: "landscape" | "portrait" | "square" | null;
  /** `#rrggbb`, doubles as the tile placeholder while the thumb decodes. */
  dominant: string | null;
  /** Where it was imported from. Provenance only — never read back. */
  origin: string | null;
  title: string | null;
  note: string | null;
  addedAt: number;
  tags: string[];
  /** Absolute path to the stored original. */
  path: string;
  /** Absolute path to the cached thumbnail, when one was generated. */
  thumb: string | null;
};

export type MediaCollection = { id: number; name: string; count: number };

type RawMedia = Omit<MediaItem, "addedAt"> & { added_at: number };

const toMedia = ({ added_at, ...m }: RawMedia): MediaItem => ({
  ...m,
  addedAt: added_at,
});

export async function importMedia(paths: string[]): Promise<MediaItem[]> {
  const raw = await invoke<RawMedia[]>("import_media", { paths });
  return raw.map(toMedia);
}

export async function listMedia(collection?: number): Promise<MediaItem[]> {
  const raw = await invoke<RawMedia[]>("list_media", {
    collection: collection ?? null,
  });
  return raw.map(toMedia);
}

export async function searchMedia(
  query: string,
  limit?: number,
): Promise<MediaItem[]> {
  const raw = await invoke<RawMedia[]>("search_media", {
    query,
    limit: limit ?? null,
  });
  return raw.map(toMedia);
}

export function removeMedia(hash: string): Promise<void> {
  return invoke("remove_media", { hash });
}

export function setMediaTags(hash: string, tags: string[]): Promise<void> {
  return invoke("set_media_tags", { hash, tags });
}

export function listCollections(): Promise<MediaCollection[]> {
  return invoke<MediaCollection[]>("list_collections");
}

export function createCollection(name: string): Promise<number> {
  return invoke<number>("create_collection", { name });
}

export function setCollectionMembership(
  collectionId: number,
  hash: string,
  member: boolean,
): Promise<void> {
  return invoke("set_collection_membership", { collectionId, hash, member });
}

// ----------------------------------------------------------- preferences ---

export function getPreference(key: string): Promise<string | null> {
  return invoke<string | null>("get_preference", { key });
}

export function setPreference(key: string, value: string): Promise<void> {
  return invoke("set_preference", { key, value });
}

/** Which part of the instruction stack a layer belongs to. */
export type ContextScope =
  | "system"
  | "user"
  | "project"
  | "local"
  | "skills"
  | "mcp"
  | "memory";

export type ContextLayer = {
  scope: ContextScope;
  label: string;
  /** Path on disk, or a summary when the layer aggregates several files. */
  path: string;
  tokens: number;
  /**
   * False when the cost cannot be read from disk — a built-in system prompt, or
   * MCP tool schemas that only arrive after a handshake. Such layers contribute
   * nothing to `total` and must not be drawn as a size.
   */
  measured: boolean;
  note: string | null;
  bytes: number;
};

export type ResolvedContext = {
  runner: Runner;
  cwd: string;
  /** In load order: shallowest first, so the last entry wins. */
  layers: ContextLayer[];
  /** Sum over measured layers only. */
  total: number;
  unmeasured: number;
  scannedMs: number;
};

export async function resolveContext(
  runner: Runner,
  cwd: string,
): Promise<ResolvedContext> {
  const raw = await invoke<Omit<ResolvedContext, "scannedMs"> & { scanned_ms: number }>(
    "resolve_context",
    { runner, cwd },
  );
  const { scanned_ms, ...rest } = raw;
  return { ...rest, scannedMs: scanned_ms };
}

export async function scanMcp(fresh = false): Promise<McpSnapshot> {
  const raw = await invoke<{ servers: RawServer[]; scanned_ms: number }>("scan_mcp", { fresh });
  return {
    scannedMs: raw.scanned_ms,
    servers: raw.servers.map(({ env_keys, config_path, ...s }) => ({
      ...s,
      envKeys: env_keys,
      configPath: config_path,
    })),
  };
}

import { Channel } from "@tauri-apps/api/core";

export type PermissionMode =
  | "Plan"
  | "Manual"
  | "AcceptEdits"
  | "Auto"
  | "DontAsk"
  | "BypassPermissions";

export const PERMISSION_MODES: {
  id: PermissionMode;
  label: string;
  desc: string;
  tone: "safe" | "caution" | "risky";
  tag?: string;
}[] = [
  { id: "Plan", label: "plan", desc: "Read-only. Explores and proposes, never edits or runs commands.", tone: "safe", tag: "Recommended" },
  { id: "Manual", label: "manual", desc: "Asks in the terminal before each tool call.", tone: "safe" },
  { id: "AcceptEdits", label: "acceptEdits", desc: "Auto-approves file edits. Still asks for commands.", tone: "caution" },
  { id: "Auto", label: "auto", desc: "Auto-approves most tool calls.", tone: "caution" },
  { id: "DontAsk", label: "dontAsk", desc: "Never prompts. Runs whatever it decides to run.", tone: "risky", tag: "Risky" },
  { id: "BypassPermissions", label: "bypassPermissions", desc: "Every guard off, including dangerous commands.", tone: "risky", tag: "Dangerous" },
];

export type TurnEvent =
  | { kind: "started"; sessionId: string; model: string; cwd: string; tools: number; mcpServers: number; permissionMode: string }
  | { kind: "text"; text: string }
  | { kind: "tool-call"; name: string; summary: string }
  | { kind: "raw"; lineType: string; json: string }
  | { kind: "finished"; isError: boolean; durationMs: number }
  | { kind: "failed"; message: string };

/** Rust serialises snake_case; normalise at the boundary. */
type RawEvent = Record<string, unknown> & { kind: string };

function normaliseEvent(e: RawEvent): TurnEvent {
  const g = (k: string) => e[k];
  switch (e.kind) {
    case "started":
      return {
        kind: "started",
        sessionId: String(g("session_id") ?? ""),
        model: String(g("model") ?? ""),
        cwd: String(g("cwd") ?? ""),
        tools: Number(g("tools") ?? 0),
        mcpServers: Number(g("mcp_servers") ?? 0),
        permissionMode: String(g("permission_mode") ?? ""),
      };
    case "tool-call":
      return { kind: "tool-call", name: String(g("name") ?? ""), summary: String(g("summary") ?? "") };
    case "finished":
      return { kind: "finished", isError: Boolean(g("is_error")), durationMs: Number(g("duration_ms") ?? 0) };
    case "raw":
      return { kind: "raw", lineType: String(g("line_type") ?? ""), json: String(g("json") ?? "") };
    case "failed":
      return { kind: "failed", message: String(g("message") ?? "") };
    default:
      return { kind: "text", text: String(g("text") ?? "") };
  }
}

export async function runTurn(
  runner: Runner,
  prompt: string,
  mode: PermissionMode,
  cwd: string | null,
  model: string | null,
  effort: string | null,
  onEvent: (e: TurnEvent) => void,
): Promise<void> {
  const channel = new Channel<RawEvent>();
  channel.onmessage = (e) => onEvent(normaliseEvent(e));
  await invoke("run_turn", {
    runner: runner === "claude-code" ? "claude-code" : "codex",
    prompt,
    cwd,
    mode,
    model,
    effort,
    channel,
  });
}

export type ReasoningLevel = { effort: string; description: string };

export type ModelOption = {
  id: string | null;
  label: string;
  note: string;
  isAlias: boolean;
  /** Effort levels this model accepts, lowest first. */
  reasoningLevels: ReasoningLevel[];
  defaultEffort: string | null;
};

export type ModelCatalogue = {
  models: ModelOption[];
  configuredDefault: string | null;
  source: string;
};

type RawModel = Omit<ModelOption, "isAlias" | "reasoningLevels" | "defaultEffort"> & {
  is_alias: boolean;
  reasoning_levels: ReasoningLevel[];
  default_effort: string | null;
};

export async function listModels(runner: Runner): Promise<ModelCatalogue> {
  const raw = await invoke<{
    models: RawModel[];
    configured_default: string | null;
    source: string;
  }>("list_models", { runner });
  return {
    configuredDefault: raw.configured_default,
    source: raw.source,
    models: raw.models.map(({ is_alias, reasoning_levels, default_effort, ...m }) => ({
      ...m,
      isAlias: is_alias,
      reasoningLevels: reasoning_levels,
      defaultEffort: default_effort,
    })),
  };
}
