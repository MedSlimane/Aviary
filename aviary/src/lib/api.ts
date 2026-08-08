import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { error as writeErrorLog } from "@tauri-apps/plugin-log";

const LOG_FIELD_CHARS = 16_000;

export type FrontendFailure = {
  source: "react" | "window" | "unhandled-rejection" | "ipc" | "handled";
  context?: string;
  message: string;
  stack?: string;
  componentStack?: string;
};

function clippedForLog(value: string | undefined): string | undefined {
  if (value === undefined) return undefined;
  const redacted = value.replace(/\/Users\/[^/\s]+/g, "~");
  return redacted.length > LOG_FIELD_CHARS
    ? `${redacted.slice(0, LOG_FIELD_CHARS)}\n…[truncated]`
    : redacted;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Best-effort only: a failed logger must never become another app failure. */
export function reportFrontendFailure(failure: FrontendFailure): void {
  const safe = {
    source: clippedForLog(failure.source),
    context: clippedForLog(failure.context),
    message: clippedForLog(failure.message),
    stack: clippedForLog(failure.stack),
    componentStack: clippedForLog(failure.componentStack),
  };
  try {
    void writeErrorLog(JSON.stringify(safe)).catch(() => {});
  } catch {
    // The diagnostics path must never recursively create a global error.
  }
}

/**
 * The one IPC path also records rejected commands. Arguments are intentionally
 * absent from the log: they can contain prompts, config, paths, or media data.
 */
async function invokeLogged<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    reportFrontendFailure({
      source: "ipc",
      context: command,
      message: errorMessage(error),
      stack: error instanceof Error ? error.stack : undefined,
    });
    throw error;
  }
}

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

function normalizeLibrarySnapshot(raw: RawSnapshot): LibrarySnapshot {
  return {
    ...raw,
    scannedMs: raw.scanned_ms,
    entries: raw.entries.map(({ real_path, ...entry }) => ({
      ...entry,
      realPath: real_path,
    })),
  };
}

type RawLibraryUpdate = {
  revision: number;
  changed_paths: string[];
  scopes: string[];
  snapshot: RawSnapshot;
};

export type LibraryUpdate = {
  revision: number;
  changedPaths: string[];
  scopes: string[];
  snapshot: LibrarySnapshot;
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
  const raw = await invokeLogged<RawSnapshot>("scan_library", { fresh });
  return normalizeLibrarySnapshot(raw);
}

export function listenLibraryUpdates(
  handler: (update: LibraryUpdate) => void,
): Promise<UnlistenFn> {
  return listen<RawLibraryUpdate>("aviary://library-updated", ({ payload }) => {
    handler({
      revision: payload.revision,
      changedPaths: payload.changed_paths,
      scopes: payload.scopes,
      snapshot: normalizeLibrarySnapshot(payload.snapshot),
    });
  });
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
  const r = await invokeLogged<RawWrite>("write_entry", {
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
  return invokeLogged<EntryContent>("read_entry", { path });
}

export function countTokens(path: string): Promise<number> {
  return invokeLogged<number>("count_tokens", { path });
}

export function listProjects(): Promise<Project[]> {
  return invokeLogged<Project[]>("list_projects");
}

export function addProject(name: string, path: string): Promise<Project[]> {
  return invokeLogged<Project[]>("add_project", { name, path });
}

export function removeProject(path: string): Promise<Project[]> {
  return invokeLogged<Project[]>("remove_project", { path });
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
  const raw = await invokeLogged<RawDiscovery>("discover_projects", { fresh });
  return { candidates: raw.candidates, scannedMs: raw.scanned_ms, roots: raw.roots };
}

export type McpSource = "managed" | "local" | "project" | "user" | "plugin";
export type McpTransport =
  | "stdio"
  | "http"
  | "sse"
  | "websocket"
  | "runner-provided"
  | "invalid";
export type McpLauncherKind =
  | "node"
  | "npx"
  | "bun"
  | "python"
  | "uvx"
  | "docker"
  | "other"
  | "missing";
export type McpInvalidConfigReason =
  | "missing-command"
  | "missing-transport-type"
  | "missing-url"
  | "invalid-url"
  | "conflicting-transport"
  | "unsupported-transport";

export type McpTransportSummary =
  | {
      kind: "stdio";
      launcher: McpLauncherKind;
      argumentCount: number;
      envKeys: string[];
      inheritedEnvKeys: string[];
      hasWorkingDirectory: boolean;
    }
  | {
      kind: "remote";
      transport: McpTransport;
      scheme: string | null;
      host: string | null;
      port: number | null;
      pathSegments: number;
      hasQuery: boolean;
      headerKeys: string[];
      bearerEnvKey: string | null;
    }
  | { kind: "runnerProvided" }
  | { kind: "invalid"; reason: McpInvalidConfigReason };

export type McpDeclarationState =
  | "enabled"
  | "disabled"
  | "pending-approval"
  | "invalid"
  | "blocked-by-policy"
  | "unknown";

export type McpHealthState =
  | "unchecked"
  | "checking"
  | "reachable"
  | "starting"
  | "ready"
  | "degraded"
  | "disabled"
  | "pending-approval"
  | "auth-required"
  | "needs-authentication"
  | "not-configured"
  | "failed"
  | "timed-out"
  | "cancelled"
  | "unsupported"
  | "shadowed"
  | "blocked-by-policy";

export type TokenBasis =
  | "runner-exact"
  | "o200k-file-estimate"
  | "o200k-schema-estimate"
  | "unavailable";

export type TokenMeasurement = {
  tokens: number | null;
  basis: TokenBasis;
  complete: boolean;
  loaded: boolean | null;
  includedInTotal: boolean;
};

export type McpToolInventory = {
  count: number | null;
  definitions: TokenMeasurement;
  checkedAtMs: number | null;
};

export type McpHealth = {
  state: McpHealthState;
  tools: McpToolInventory;
  checkedAtMs: number | null;
  expiresAtMs: number | null;
  stale: boolean;
};

export type McpToggleUnavailableReason =
  | "managed-by-policy"
  | "pending-approval"
  | "invalid-configuration"
  | "project-required"
  | "runner-provided-only"
  | "unsupported-source";

export type McpToggleCapability = {
  writable: boolean;
  revision: string | null;
  sharedProjectFile: boolean;
  unavailableReason: McpToggleUnavailableReason | null;
};

export type McpDeclaration = {
  id: string;
  runner: Runner;
  name: string;
  effectiveName: string;
  source: McpSource;
  origin: string | null;
  projectKey: string | null;
  configPath: string;
  pointer: string;
  transport: McpTransportSummary;
  state: McpDeclarationState;
};

export type McpServer = {
  id: string;
  runner: Runner;
  cwd: string | null;
  declarationId: string;
  name: string;
  source: McpSource;
  transport: McpTransportSummary;
  state: McpDeclarationState;
  shadowedDeclarationIds: string[];
  toggle: McpToggleCapability;
  healthRevision: string;
  health: McpHealth;
};

export type McpHealthResult = {
  id: string;
  declarationId: string | null;
  revision: string | null;
  runner: Runner;
  cwd: string;
  name: string;
  runnerProvided: boolean;
  health: McpHealth;
};

export type McpHealthSnapshot = {
  runner: Runner;
  cwd: string;
  results: McpHealthResult[];
  checkedAtMs: number;
  expiresAtMs: number;
  complete: boolean;
};

export type McpSnapshot = {
  declarations: McpDeclaration[];
  servers: McpServer[];
  healthResults: McpHealthResult[];
  scannedMs: number;
};

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
  const raw = await invokeLogged<RawMedia[]>("import_media", { paths });
  return raw.map(toMedia);
}

export async function listMedia(collection?: number): Promise<MediaItem[]> {
  const raw = await invokeLogged<RawMedia[]>("list_media", {
    collection: collection ?? null,
  });
  return raw.map(toMedia);
}

export async function searchMedia(
  query: string,
  limit?: number,
): Promise<MediaItem[]> {
  const raw = await invokeLogged<RawMedia[]>("search_media", {
    query,
    limit: limit ?? null,
  });
  return raw.map(toMedia);
}

export function removeMedia(hash: string): Promise<void> {
  return invokeLogged("remove_media", { hash });
}

export function setMediaTags(hash: string, tags: string[]): Promise<void> {
  return invokeLogged("set_media_tags", { hash, tags });
}

export function listCollections(): Promise<MediaCollection[]> {
  return invokeLogged<MediaCollection[]>("list_collections");
}

export function createCollection(name: string): Promise<number> {
  return invokeLogged<number>("create_collection", { name });
}

export function setCollectionMembership(
  collectionId: number,
  hash: string,
  member: boolean,
): Promise<void> {
  return invokeLogged("set_collection_membership", { collectionId, hash, member });
}

// ----------------------------------------------------------- preferences ---

export function getPreference(key: string): Promise<string | null> {
  return invokeLogged<string | null>("get_preference", { key });
}

export function setPreference(key: string, value: string): Promise<void> {
  return invokeLogged("set_preference", { key, value });
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
  tokens: number | null;
  basis: TokenBasis;
  complete: boolean;
  loaded: boolean | null;
  includedInTotal: boolean;
  note: string | null;
  bytes: number | null;
};

export type ResolvedContext = {
  runner: Runner;
  cwd: string;
  /** In load order: shallowest first, so the last entry wins. */
  layers: ContextLayer[];
  /** Sum over measured layers only. */
  total: number;
  totalComplete: boolean;
  unmeasured: number;
  scannedMs: number;
};

type RawContextLayer = Omit<ContextLayer, "includedInTotal"> & {
  included_in_total: boolean;
};

type RawResolvedContext = Omit<
  ResolvedContext,
  "layers" | "totalComplete" | "scannedMs"
> & {
  layers: RawContextLayer[];
  total_complete: boolean;
  scanned_ms: number;
};

export async function resolveContext(
  runner: Runner,
  cwd: string,
): Promise<ResolvedContext> {
  const raw = await invokeLogged<RawResolvedContext>("resolve_context", {
    runner,
    cwd,
  });
  return {
    runner: raw.runner,
    cwd: raw.cwd,
    total: raw.total,
    totalComplete: raw.total_complete,
    unmeasured: raw.unmeasured,
    scannedMs: raw.scanned_ms,
    layers: raw.layers.map(({ included_in_total, ...layer }) => ({
      ...layer,
      includedInTotal: included_in_total,
    })),
  };
}

type RawTransportSummary =
  | {
      kind: "stdio";
      launcher: McpLauncherKind;
      argument_count: number;
      env_keys: string[];
      inherited_env_keys: string[];
      has_working_directory: boolean;
    }
  | {
      kind: "remote";
      transport: McpTransport;
      scheme: string | null;
      host: string | null;
      port: number | null;
      path_segments: number;
      has_query: boolean;
      header_keys: string[];
      bearer_env_key: string | null;
    }
  | { kind: "runnerProvided" }
  | { kind: "invalid"; reason: McpInvalidConfigReason };

type RawTokenMeasurement = Omit<TokenMeasurement, "includedInTotal"> & {
  included_in_total: boolean;
};

type RawMcpToolInventory = Omit<McpToolInventory, "definitions" | "checkedAtMs"> & {
  definitions: RawTokenMeasurement;
  checked_at_ms: number | null;
};

type RawMcpHealth = Omit<
  McpHealth,
  "tools" | "checkedAtMs" | "expiresAtMs"
> & {
  tools: RawMcpToolInventory;
  checked_at_ms: number | null;
  expires_at_ms: number | null;
};

type RawMcpToggleCapability = Omit<
  McpToggleCapability,
  "sharedProjectFile" | "unavailableReason"
> & {
  shared_project_file: boolean;
  unavailable_reason: McpToggleUnavailableReason | null;
};

type RawMcpDeclaration = Omit<
  McpDeclaration,
  "effectiveName" | "projectKey" | "configPath" | "transport"
> & {
  effective_name: string;
  project_key: string | null;
  config_path: string;
  transport: RawTransportSummary;
};

type RawMcpServer = Omit<
  McpServer,
  | "declarationId"
  | "transport"
  | "shadowedDeclarationIds"
  | "toggle"
  | "healthRevision"
  | "health"
> & {
  declaration_id: string;
  transport: RawTransportSummary;
  shadowed_declaration_ids: string[];
  toggle: RawMcpToggleCapability;
  health_revision: string;
  health: RawMcpHealth;
};

type RawMcpHealthResult = Omit<
  McpHealthResult,
  "declarationId" | "runnerProvided" | "health"
> & {
  declaration_id: string | null;
  runner_provided: boolean;
  health: RawMcpHealth;
};

type RawMcpSnapshot = {
  declarations: RawMcpDeclaration[];
  servers: RawMcpServer[];
  health_results: RawMcpHealthResult[];
  scanned_ms: number;
};

function normalizeTransport(raw: RawTransportSummary): McpTransportSummary {
  switch (raw.kind) {
    case "stdio":
      return {
        kind: raw.kind,
        launcher: raw.launcher,
        argumentCount: raw.argument_count,
        envKeys: raw.env_keys,
        inheritedEnvKeys: raw.inherited_env_keys,
        hasWorkingDirectory: raw.has_working_directory,
      };
    case "remote":
      return {
        kind: raw.kind,
        transport: raw.transport,
        scheme: raw.scheme,
        host: raw.host,
        port: raw.port,
        pathSegments: raw.path_segments,
        hasQuery: raw.has_query,
        headerKeys: raw.header_keys,
        bearerEnvKey: raw.bearer_env_key,
      };
    default:
      return raw;
  }
}

function normalizeTokenMeasurement(raw: RawTokenMeasurement): TokenMeasurement {
  const { included_in_total, ...measurement } = raw;
  return { ...measurement, includedInTotal: included_in_total };
}

function normalizeMcpHealth(raw: RawMcpHealth): McpHealth {
  return {
    state: raw.state,
    stale: raw.stale,
    checkedAtMs: raw.checked_at_ms,
    expiresAtMs: raw.expires_at_ms,
    tools: {
      count: raw.tools.count,
      checkedAtMs: raw.tools.checked_at_ms,
      definitions: normalizeTokenMeasurement(raw.tools.definitions),
    },
  };
}

function normalizeHealthResult(raw: RawMcpHealthResult): McpHealthResult {
  const { declaration_id, runner_provided, health, ...result } = raw;
  return {
    ...result,
    declarationId: declaration_id,
    runnerProvided: runner_provided,
    health: normalizeMcpHealth(health),
  };
}

function normalizeMcpSnapshot(raw: RawMcpSnapshot): McpSnapshot {
  return {
    scannedMs: raw.scanned_ms,
    declarations: raw.declarations.map(
      ({ effective_name, project_key, config_path, transport, ...declaration }) => ({
        ...declaration,
        effectiveName: effective_name,
        projectKey: project_key,
        configPath: config_path,
        transport: normalizeTransport(transport),
      }),
    ),
    servers: raw.servers.map(
      ({
        declaration_id,
        transport,
        shadowed_declaration_ids,
        toggle,
        health_revision,
        health,
        ...server
      }) => ({
        ...server,
        declarationId: declaration_id,
        transport: normalizeTransport(transport),
        shadowedDeclarationIds: shadowed_declaration_ids,
        toggle: {
          writable: toggle.writable,
          revision: toggle.revision,
          sharedProjectFile: toggle.shared_project_file,
          unavailableReason: toggle.unavailable_reason,
        },
        healthRevision: health_revision,
        health: normalizeMcpHealth(health),
      }),
    ),
    healthResults: raw.health_results.map(normalizeHealthResult),
  };
}

export async function scanMcp(
  fresh = false,
  cwd?: string,
): Promise<McpSnapshot> {
  const raw = await invokeLogged<RawMcpSnapshot>("scan_mcp", {
    fresh,
    cwd: cwd ?? null,
  });
  return normalizeMcpSnapshot(raw);
}

export function canonicalContextDirectory(cwd: string): Promise<string> {
  return invokeLogged("canonical_context_directory", { cwd });
}

export async function checkMcpHealth(
  runner: Runner,
  cwd: string,
  effectiveIds?: string[],
): Promise<McpHealthSnapshot> {
  const raw = await invokeLogged<{
    runner: Runner;
    cwd: string;
    results: RawMcpHealthResult[];
    checked_at_ms: number;
    expires_at_ms: number;
    complete: boolean;
  }>("check_mcp_health", {
    runner,
    cwd,
    effectiveIds: effectiveIds ?? null,
  });
  return {
    runner: raw.runner,
    cwd: raw.cwd,
    results: raw.results.map(normalizeHealthResult),
    checkedAtMs: raw.checked_at_ms,
    expiresAtMs: raw.expires_at_ms,
    complete: raw.complete,
  };
}

export type McpToggleOutcome =
  | { status: "written"; revision: string; snapshotId: string }
  | { status: "unchanged"; revision: string }
  | { status: "conflict" }
  | { status: "unavailable"; reason: McpToggleUnavailableReason }
  | { status: "not-found" };

type RawMcpToggleOutcome =
  | { status: "written"; revision: string; snapshot_id: string }
  | { status: "unchanged"; revision: string }
  | { status: "conflict" }
  | { status: "unavailable"; reason: McpToggleUnavailableReason }
  | { status: "not-found" };

export async function setMcpEnabled(
  effectiveId: string,
  cwd: string | null,
  enabled: boolean,
  expectedRevision: string,
): Promise<McpToggleOutcome> {
  const raw = await invokeLogged<RawMcpToggleOutcome>("set_mcp_enabled", {
    effectiveId,
    cwd,
    enabled,
    expectedRevision,
  });
  if (raw.status !== "written") return raw;
  return {
    status: raw.status,
    revision: raw.revision,
    snapshotId: raw.snapshot_id,
  };
}

export type TurnStatus =
  | "queued"
  | "running"
  | "completed"
  | "failed"
  | "interrupted";

export type FailureKind =
  | "spawn"
  | "protocol"
  | "runner-exit"
  | "input"
  | "internal";

export type ToolResultStatus = "succeeded" | "failed";

export type PermissionQuestionOption = {
  label: string;
  description: string;
};

export type PermissionQuestion = {
  id: string;
  header: string;
  question: string;
  isOther: boolean;
  isSecret: boolean;
  options: PermissionQuestionOption[];
};

export type PermissionPromptData =
  | { kind: "questions"; questions: PermissionQuestion[] }
  | { kind: "unsupported"; message: string };

export type SessionEvent =
  | {
      kind: "started";
      model: string | null;
      cwd: string | null;
      tools: number | null;
      mcpServers: number | null;
      permissionMode: string | null;
    }
  | { kind: "thinking"; text: string }
  | { kind: "text"; text: string }
  | {
      kind: "tool-call";
      callId: string | null;
      name: string;
      summary: string;
      detail: string | null;
    }
  | {
      kind: "tool-result";
      callId: string | null;
      status: ToolResultStatus;
      summary: string;
      detail: string | null;
    }
  | {
      kind: "tool-started" | "tool-updated";
      callId: string;
      name: string;
      summary: string;
      detail: string | null;
    }
  | {
      kind: "tool-finished";
      callId: string;
      name: string;
      status: ToolResultStatus;
      summary: string;
      detail: string | null;
    }
  | {
      kind: "permission-request";
      requestId: string;
      toolName: string;
      summary: string;
      options: PermissionDecision[];
      prompt: PermissionPromptData | null;
      expiresWithTurn: boolean;
    }
  | { kind: "permission-resolved"; requestId: string; decision: string }
  | {
      kind: "token-usage";
      inputTokens: number | null;
      cachedInputTokens: number | null;
      outputTokens: number | null;
      reasoningOutputTokens: number | null;
      totalTokens: number | null;
    }
  | { kind: "finished" | "interrupted"; durationMs: number | null }
  | {
      kind: "failed";
      failure: FailureKind;
      displayMessage: string;
      durationMs: number | null;
    };

export type StoredEvent = {
  id: number;
  turnId: string;
  sequence: number;
  schemaVersion: number;
  createdAt: number;
  event: SessionEvent;
};

export type ChatSession = {
  id: string;
  runner: Runner;
  runnerSessionId: string | null;
  cwd: string;
  title: string;
  createdAt: number;
  updatedAt: number;
};

export type ChatTurn = {
  id: string;
  sessionId: string;
  ordinal: number;
  prompt: string;
  requestedModel: string | null;
  requestedEffort: string | null;
  permissionMode: string;
  status: TurnStatus;
  failureKind: FailureKind | null;
  createdAt: number;
  startedAt: number | null;
  finishedAt: number | null;
  durationMs: number | null;
};

export type TurnDetail = { turn: ChatTurn; events: StoredEvent[] };
export type SessionDetail = { session: ChatSession; turns: TurnDetail[] };
export type SessionSummary = {
  session: ChatSession;
  turnCount: number;
  lastTurnStatus: TurnStatus | null;
};
export type RunReceipt = { session: ChatSession; turn: ChatTurn };

export type SafetyOption = {
  id: string;
  label: string;
  description: string;
  interactiveApprovals: boolean;
  dangerous: boolean;
  sandbox: string | null;
  approvalPolicy: string | null;
};

export type SafetyCapabilities = {
  runner: Runner;
  available: boolean;
  protocol: string;
  defaultOptionId: string | null;
  options: SafetyOption[];
  warning: string | null;
};

export type PermissionDecision =
  | "allow-once"
  | "allow-session"
  | "deny"
  | "cancel"
  | "submit";

export type PermissionReply = {
  decision: PermissionDecision;
  updatedInput?: unknown;
  message?: string;
  answers?: Record<string, { answers: string[] }>;
  content?: unknown;
};

export type EngineEvent = { sessionId: string; stored: StoredEvent };

type RawPermissionQuestion = Omit<PermissionQuestion, "isOther" | "isSecret"> & {
  is_other: boolean;
  is_secret: boolean;
};

type RawPermissionPromptData =
  | { kind: "questions"; questions: RawPermissionQuestion[] }
  | { kind: "unsupported"; message: string };

type RawSessionEvent =
  | {
      kind: "started";
      model: string | null;
      cwd: string | null;
      tools: number | null;
      mcp_servers: number | null;
      permission_mode: string | null;
    }
  | { kind: "thinking"; text: string }
  | { kind: "text"; text: string }
  | {
      kind: "tool-call";
      call_id: string | null;
      name: string;
      summary: string;
      detail: string | null;
    }
  | {
      kind: "tool-result";
      call_id: string | null;
      status: ToolResultStatus;
      summary: string;
      detail: string | null;
    }
  | {
      kind: "tool-started" | "tool-updated";
      call_id: string;
      name: string;
      summary: string;
      detail: string | null;
    }
  | {
      kind: "tool-finished";
      call_id: string;
      name: string;
      status: ToolResultStatus;
      summary: string;
      detail: string | null;
    }
  | {
      kind: "permission-request";
      request_id: string;
      tool_name: string;
      summary: string;
      options: PermissionDecision[];
      prompt?: RawPermissionPromptData | null;
      expires_with_turn?: boolean;
    }
  | { kind: "permission-resolved"; request_id: string; decision: string }
  | {
      kind: "token-usage";
      input_tokens: number | null;
      cached_input_tokens: number | null;
      output_tokens: number | null;
      reasoning_output_tokens: number | null;
      total_tokens: number | null;
    }
  | { kind: "finished" | "interrupted"; duration_ms: number | null }
  | {
      kind: "failed";
      failure: FailureKind;
      display_message: string;
      duration_ms: number | null;
    };

type RawStoredEvent = Omit<StoredEvent, "turnId" | "schemaVersion" | "createdAt" | "event"> & {
  turn_id: string;
  schema_version: number;
  created_at: number;
  event: RawSessionEvent;
};

type RawChatSession = Omit<
  ChatSession,
  "runnerSessionId" | "createdAt" | "updatedAt"
> & {
  runner_session_id: string | null;
  created_at: number;
  updated_at: number;
};

type RawChatTurn = Omit<
  ChatTurn,
  | "sessionId"
  | "requestedModel"
  | "requestedEffort"
  | "permissionMode"
  | "failureKind"
  | "createdAt"
  | "startedAt"
  | "finishedAt"
  | "durationMs"
> & {
  session_id: string;
  requested_model: string | null;
  requested_effort: string | null;
  permission_mode: string;
  failure_kind: FailureKind | null;
  created_at: number;
  started_at: number | null;
  finished_at: number | null;
  duration_ms: number | null;
};

type RawTurnDetail = { turn: RawChatTurn; events: RawStoredEvent[] };
type RawSessionDetail = { session: RawChatSession; turns: RawTurnDetail[] };
type RawSessionSummary = {
  session: RawChatSession;
  turn_count: number;
  last_turn_status: TurnStatus | null;
};
type RawRunReceipt = { session: RawChatSession; turn: RawChatTurn };
type RawEngineEvent = { sessionId: string; stored: RawStoredEvent };

function normalizePermissionPrompt(
  prompt: RawPermissionPromptData | null | undefined,
): PermissionPromptData | null {
  if (!prompt) return null;
  if (prompt.kind === "unsupported") return prompt;
  return {
    kind: "questions",
    questions: prompt.questions.map(({ is_other, is_secret, ...question }) => ({
      ...question,
      isOther: is_other,
      isSecret: is_secret,
    })),
  };
}

function normalizeSessionEvent(event: RawSessionEvent): SessionEvent {
  switch (event.kind) {
    case "started":
      return {
        kind: event.kind,
        model: event.model,
        cwd: event.cwd,
        tools: event.tools,
        mcpServers: event.mcp_servers,
        permissionMode: event.permission_mode,
      };
    case "thinking":
    case "text":
      return event;
    case "tool-call":
    case "tool-result": {
      const { call_id, ...rest } = event;
      return { ...rest, callId: call_id };
    }
    case "tool-started":
    case "tool-updated":
    case "tool-finished": {
      const { call_id, ...rest } = event;
      return { ...rest, callId: call_id };
    }
    case "permission-request":
      return {
        kind: event.kind,
        requestId: event.request_id,
        toolName: event.tool_name,
        summary: event.summary,
        options: event.options,
        prompt: normalizePermissionPrompt(event.prompt),
        expiresWithTurn: event.expires_with_turn ?? false,
      };
    case "permission-resolved":
      return {
        kind: event.kind,
        requestId: event.request_id,
        decision: event.decision,
      };
    case "token-usage":
      return {
        kind: event.kind,
        inputTokens: event.input_tokens,
        cachedInputTokens: event.cached_input_tokens,
        outputTokens: event.output_tokens,
        reasoningOutputTokens: event.reasoning_output_tokens,
        totalTokens: event.total_tokens,
      };
    case "finished":
    case "interrupted":
      return { kind: event.kind, durationMs: event.duration_ms };
    case "failed":
      return {
        kind: event.kind,
        failure: event.failure,
        displayMessage: event.display_message,
        durationMs: event.duration_ms,
      };
  }
}

function normalizeStoredEvent(raw: RawStoredEvent): StoredEvent {
  return {
    id: raw.id,
    turnId: raw.turn_id,
    sequence: raw.sequence,
    schemaVersion: raw.schema_version,
    createdAt: raw.created_at,
    event: normalizeSessionEvent(raw.event),
  };
}

function normalizeChatSession(raw: RawChatSession): ChatSession {
  const { runner_session_id, created_at, updated_at, ...session } = raw;
  return {
    ...session,
    runnerSessionId: runner_session_id,
    createdAt: created_at,
    updatedAt: updated_at,
  };
}

function normalizeChatTurn(raw: RawChatTurn): ChatTurn {
  const {
    session_id,
    requested_model,
    requested_effort,
    permission_mode,
    failure_kind,
    created_at,
    started_at,
    finished_at,
    duration_ms,
    ...turn
  } = raw;
  return {
    ...turn,
    sessionId: session_id,
    requestedModel: requested_model,
    requestedEffort: requested_effort,
    permissionMode: permission_mode,
    failureKind: failure_kind,
    createdAt: created_at,
    startedAt: started_at,
    finishedAt: finished_at,
    durationMs: duration_ms,
  };
}

function normalizeSessionDetail(raw: RawSessionDetail): SessionDetail {
  return {
    session: normalizeChatSession(raw.session),
    turns: raw.turns.map(({ turn, events }) => ({
      turn: normalizeChatTurn(turn),
      events: events.map(normalizeStoredEvent),
    })),
  };
}

function normalizeRunReceipt(raw: RawRunReceipt): RunReceipt {
  return {
    session: normalizeChatSession(raw.session),
    turn: normalizeChatTurn(raw.turn),
  };
}

export function discoverRunnerSafety(runner: Runner): Promise<SafetyCapabilities> {
  return invokeLogged("discover_runner_safety", { runner });
}

export async function listChatSessions(limit = 100): Promise<SessionSummary[]> {
  const raw = await invokeLogged<RawSessionSummary[]>("list_chat_sessions", { limit });
  return raw.map(({ session, turn_count, last_turn_status }) => ({
    session: normalizeChatSession(session),
    turnCount: turn_count,
    lastTurnStatus: last_turn_status,
  }));
}

export async function loadChatSession(
  sessionId: string,
): Promise<SessionDetail | null> {
  const raw = await invokeLogged<RawSessionDetail | null>("load_chat_session", {
    sessionId,
  });
  return raw ? normalizeSessionDetail(raw) : null;
}

export type ChatTurnOptions = {
  prompt: string;
  safetyOptionId: string | null;
  model: string | null;
  effort: string | null;
};

function chatEventChannel(onEvent: (event: EngineEvent) => void) {
  const channel = new Channel<RawEngineEvent>();
  channel.onmessage = ({ sessionId, stored }) => {
    onEvent({ sessionId, stored: normalizeStoredEvent(stored) });
  };
  return channel;
}

/** Retained by the view until a terminal event or reconciliation finishes. */
export type ChatRunHandle = {
  receipt: RunReceipt;
  channel: Channel<RawEngineEvent>;
};

export async function createChatSession(
  runner: Runner,
  cwd: string,
  title: string | null,
  options: ChatTurnOptions,
  onEvent: (event: EngineEvent) => void,
): Promise<ChatRunHandle> {
  const channel = chatEventChannel(onEvent);
  const raw = await invokeLogged<RawRunReceipt>("create_chat_session", {
    runner,
    cwd,
    title,
    prompt: options.prompt,
    safetyOptionId: options.safetyOptionId,
    model: options.model,
    effort: options.effort,
    channel,
  });
  return { receipt: normalizeRunReceipt(raw), channel };
}

export async function createChatSessionWithBundle(
  bundleId: string,
  expectedRevision: number,
  title: string | null,
  options: ChatTurnOptions,
  onEvent: (event: EngineEvent) => void,
): Promise<ChatRunHandle> {
  const channel = chatEventChannel(onEvent);
  const raw = await invokeLogged<RawRunReceipt>(
    "create_chat_session_with_bundle",
    {
      bundleId,
      expectedRevision,
      title,
      prompt: options.prompt,
      safetyOptionId: options.safetyOptionId,
      model: options.model,
      effort: options.effort,
      channel,
    },
  );
  return { receipt: normalizeRunReceipt(raw), channel };
}

export async function resumeChatSession(
  sessionId: string,
  options: ChatTurnOptions,
  onEvent: (event: EngineEvent) => void,
): Promise<ChatRunHandle> {
  const channel = chatEventChannel(onEvent);
  const raw = await invokeLogged<RawRunReceipt>("resume_chat_session", {
    sessionId,
    prompt: options.prompt,
    safetyOptionId: options.safetyOptionId,
    model: options.model,
    effort: options.effort,
    channel,
  });
  return { receipt: normalizeRunReceipt(raw), channel };
}

export function respondPermission(
  requestId: string,
  reply: PermissionReply,
): Promise<void> {
  return invokeLogged("respond_permission", { requestId, reply });
}

export function interruptTurn(turnId: string): Promise<void> {
  return invokeLogged("interrupt_turn", { turnId });
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
  const raw = await invokeLogged<{
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

// --------------------------------------------------------------- bundles ---

export type BundleMemoryMode = "inherit" | "supplement";
export type BundleMemberKind =
  | "project"
  | "skill"
  | "prompt"
  | "agent"
  | "memory"
  | "mcp"
  | "media-collection";
export type BundleMemberRole =
  | "working-directory"
  | "available"
  | "invoke-first-turn"
  | "prefill"
  | "primary"
  | "supplement"
  | "enabled"
  | "retrieval";

export type BundleMemberTarget =
  | { type: "project"; path: string }
  | { type: "entry"; id: string }
  | { type: "mcp-declaration"; id: string }
  | { type: "media-collection"; id: number };

export type BundleMemberDraft = {
  /** Present only when preserving a member returned by Aviary during update. */
  id: string | null;
  ordinal: number;
  kind: BundleMemberKind;
  role: BundleMemberRole;
  target: BundleMemberTarget;
};

export type BundleDraft = {
  name: string;
  description: string;
  runner: Runner;
  modelId: string | null;
  memoryMode: BundleMemoryMode;
  members: BundleMemberDraft[];
};

export type BundleMember = Omit<BundleMemberDraft, "id"> & {
  id: string;
  snapshotLabel: string;
  createdAt: number;
};

export type Bundle = {
  id: string;
  name: string;
  description: string;
  runner: Runner;
  modelId: string | null;
  memoryMode: BundleMemoryMode;
  revision: number;
  createdAt: number;
  updatedAt: number;
  members: BundleMember[];
};

export type BundleResolutionStatus = "ready" | "missing" | "incompatible";

export type BundleTargetResolution = {
  status: BundleResolutionStatus;
  currentLabel: string | null;
  reason: string | null;
};

export type ResolvedBundleMember = {
  member: BundleMember;
  resolution: BundleTargetResolution;
};

export type ResolvedBundle = {
  bundle: Bundle;
  members: ResolvedBundleMember[];
};

export type BundleChatPlan = {
  runner: Runner;
  cwd: string;
  modelId: string | null;
};

export type BundleSnapshotMember = {
  memberId: string;
  ordinal: number;
  kind: BundleMemberKind;
  role: BundleMemberRole;
  target: BundleMemberTarget;
  snapshotLabel: string;
  disposition: "apply" | "available" | "inherited";
  note: string | null;
};

export type BundleAttachmentSnapshot = {
  schemaVersion: number;
  bundleId: string;
  bundleRevision: number;
  bundleName: string;
  runner: Runner;
  modelId: string | null;
  cwd: string;
  members: BundleSnapshotMember[];
};

export type SessionBundleAttachment = {
  sessionId: string;
  attachedAt: number;
  snapshot: BundleAttachmentSnapshot;
};

type RawBundleMemberDraft = Omit<BundleMemberDraft, "id"> & {
  id?: string | null;
};

type RawBundleDraft = Omit<BundleDraft, "modelId" | "memoryMode" | "members"> & {
  model_id: string | null;
  memory_mode: BundleMemoryMode;
  members: RawBundleMemberDraft[];
};

type RawBundleMember = Omit<BundleMember, "snapshotLabel" | "createdAt"> & {
  snapshot_label: string;
  created_at: number;
};

type RawBundle = Omit<Bundle, "modelId" | "memoryMode" | "createdAt" | "updatedAt" | "members"> & {
  model_id: string | null;
  memory_mode: BundleMemoryMode;
  created_at: number;
  updated_at: number;
  members: RawBundleMember[];
};

type RawBundleTargetResolution = Omit<BundleTargetResolution, "currentLabel"> & {
  current_label: string | null;
};

type RawResolvedBundle = {
  bundle: RawBundle;
  members: Array<{
    member: RawBundleMember;
    resolution: RawBundleTargetResolution;
  }>;
};

type RawBundleChatPlan = Omit<BundleChatPlan, "modelId"> & {
  model_id: string | null;
};

type RawBundleSnapshotMember = Omit<
  BundleSnapshotMember,
  "memberId" | "snapshotLabel"
> & {
  member_id: string;
  snapshot_label: string;
};

type RawBundleAttachmentSnapshot = Omit<
  BundleAttachmentSnapshot,
  | "schemaVersion"
  | "bundleId"
  | "bundleRevision"
  | "bundleName"
  | "modelId"
  | "members"
> & {
  schema_version: number;
  bundle_id: string;
  bundle_revision: number;
  bundle_name: string;
  model_id: string | null;
  members: RawBundleSnapshotMember[];
};

type RawSessionBundleAttachment = Omit<
  SessionBundleAttachment,
  "sessionId" | "attachedAt" | "snapshot"
> & {
  session_id: string;
  attached_at: number;
  snapshot: RawBundleAttachmentSnapshot;
};

function toRawBundleDraft(draft: BundleDraft): RawBundleDraft {
  return {
    name: draft.name,
    description: draft.description,
    runner: draft.runner,
    model_id: draft.modelId,
    memory_mode: draft.memoryMode,
    members: draft.members.map((member, ordinal) => ({
      ...member,
      id: member.id ?? undefined,
      ordinal,
    })),
  };
}

function normalizeBundleMember(raw: RawBundleMember): BundleMember {
  const { snapshot_label, created_at, ...member } = raw;
  return {
    ...member,
    snapshotLabel: snapshot_label,
    createdAt: created_at,
  };
}

function normalizeBundle(raw: RawBundle): Bundle {
  const { model_id, memory_mode, created_at, updated_at, members, ...bundle } = raw;
  return {
    ...bundle,
    modelId: model_id,
    memoryMode: memory_mode,
    createdAt: created_at,
    updatedAt: updated_at,
    members: members.map(normalizeBundleMember),
  };
}

function normalizeResolvedBundle(raw: RawResolvedBundle): ResolvedBundle {
  return {
    bundle: normalizeBundle(raw.bundle),
    members: raw.members.map(({ member, resolution }) => ({
      member: normalizeBundleMember(member),
      resolution: {
        status: resolution.status,
        currentLabel: resolution.current_label,
        reason: resolution.reason,
      },
    })),
  };
}

function normalizeSessionBundle(
  raw: RawSessionBundleAttachment,
): SessionBundleAttachment {
  const { session_id, attached_at, snapshot } = raw;
  return {
    sessionId: session_id,
    attachedAt: attached_at,
    snapshot: {
      schemaVersion: snapshot.schema_version,
      bundleId: snapshot.bundle_id,
      bundleRevision: snapshot.bundle_revision,
      bundleName: snapshot.bundle_name,
      runner: snapshot.runner,
      modelId: snapshot.model_id,
      cwd: snapshot.cwd,
      members: snapshot.members.map(
        ({ member_id, snapshot_label, ...member }) => ({
          ...member,
          memberId: member_id,
          snapshotLabel: snapshot_label,
        }),
      ),
    },
  };
}

export async function listBundles(): Promise<ResolvedBundle[]> {
  const rows = await invokeLogged<RawResolvedBundle[]>("list_bundles");
  return rows.map(normalizeResolvedBundle);
}

export async function createBundle(draft: BundleDraft): Promise<ResolvedBundle> {
  const raw = await invokeLogged<RawResolvedBundle>("create_bundle", {
    draft: toRawBundleDraft(draft),
  });
  return normalizeResolvedBundle(raw);
}

export async function updateBundle(
  id: string,
  expectedRevision: number,
  draft: BundleDraft,
): Promise<ResolvedBundle> {
  const raw = await invokeLogged<RawResolvedBundle>("update_bundle", {
    id,
    expectedRevision,
    draft: toRawBundleDraft(draft),
  });
  return normalizeResolvedBundle(raw);
}

export function deleteBundle(id: string, expectedRevision: number): Promise<void> {
  return invokeLogged("delete_bundle", { id, expectedRevision });
}

export async function prepareBundleChat(
  bundleId: string,
  expectedRevision: number,
): Promise<BundleChatPlan> {
  const raw = await invokeLogged<RawBundleChatPlan>("prepare_bundle_chat", {
    bundleId,
    expectedRevision,
  });
  return { runner: raw.runner, cwd: raw.cwd, modelId: raw.model_id };
}

export async function loadSessionBundle(
  sessionId: string,
): Promise<SessionBundleAttachment | null> {
  const raw = await invokeLogged<RawSessionBundleAttachment | null>(
    "load_session_bundle",
    { sessionId },
  );
  return raw ? normalizeSessionBundle(raw) : null;
}

export type PreparedLaunch = {
  launchId: string;
  commandFile: string;
  descriptorFile: string;
  statusFile: string;
  expiresAt: number;
};

export function launchBundleTerminal(
  bundleId: string,
  expectedRevision: number,
): Promise<PreparedLaunch> {
  return invokeLogged("launch_bundle_terminal", {
    bundleId,
    expectedRevision,
  });
}

export type McpRegistration = {
  name: string;
  command: string;
  args: string[];
};

export function mediaMcpRegistration(
  collectionId?: number,
): Promise<McpRegistration> {
  return invokeLogged("media_mcp_registration", {
    collectionId: collectionId ?? null,
  });
}

export function libraryMcpRegistration(): Promise<McpRegistration> {
  return invokeLogged("library_mcp_registration");
}

export type DiagnosticsBundle = { text: string; logsDir?: string };

export async function collectDiagnostics(
  failure?: FrontendFailure,
): Promise<DiagnosticsBundle> {
  const raw = await invokeLogged<{ text: string; logs_dir: string | null }>(
    "collect_diagnostics",
    { failure: failure ?? null },
  );
  return { text: raw.text, logsDir: raw.logs_dir ?? undefined };
}
