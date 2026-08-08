import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  FolderOpenIcon,
  RefreshIcon,
  ServerStack01Icon,
} from "@hugeicons/core-free-icons";
import { Button } from "@/components/ui/button";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { PageHeader, Segmented, StatusDot } from "@/components/screen-parts";
import { notify } from "@/lib/notify";
import {
  canonicalContextDirectory,
  checkMcpHealth,
  listProjects,
  scanMcp,
  setMcpEnabled,
  RUNNER_LABEL,
  type McpHealth,
  type McpHealthResult,
  type McpHealthState,
  type McpServer,
  type McpSnapshot,
  type McpTransportSummary,
  type Runner,
} from "@/lib/api";
import { cn } from "@/lib/utils";

const FILTERS = ["All", "Enabled", "Needs attention", "Disabled"] as const;
type Filter = (typeof FILTERS)[number];
const RUNNERS = Object.keys(RUNNER_LABEL) as Runner[];

function tilde(path: string) {
  return path.replace(/^\/Users\/[^/]+/, "~");
}

function sameDirectory(left: string | null, right: string) {
  if (left === null) return right === "~";
  return left === right || tilde(left) === tilde(right);
}

function transportLabel(transport: McpTransportSummary) {
  switch (transport.kind) {
    case "stdio": {
      const environment = transport.envKeys.length + transport.inheritedEnvKeys.length;
      return `${transport.launcher} launcher, ${transport.argumentCount} arguments, ${environment} environment names`;
    }
    case "remote": {
      const endpoint = transport.host
        ? `${transport.scheme ?? transport.transport}://${transport.host}${transport.port ? `:${transport.port}` : ""}`
        : `${transport.transport} endpoint`;
      const shape = `${transport.pathSegments} path segments${transport.hasQuery ? ", query present" : ""}`;
      return `${endpoint}, ${shape}, ${transport.headerKeys.length} header names`;
    }
    case "runnerProvided":
      return "Provided by the runner";
    case "invalid":
      return `Invalid declaration: ${transport.reason.split("-").join(" ")}`;
  }
}

function healthTone(state: McpHealthState): "ok" | "warn" | "error" {
  switch (state) {
    case "ready":
    case "reachable":
      return "ok";
    case "starting":
    case "checking":
    case "degraded":
    case "pending-approval":
    case "auth-required":
    case "needs-authentication":
      return "warn";
    case "failed":
    case "timed-out":
    case "not-configured":
    case "blocked-by-policy":
      return "error";
    default:
      return "warn";
  }
}

function healthLabel(health: McpHealth) {
  return health.state.split("-").join(" ");
}

function healthNeedsAttention(health: McpHealth) {
  return ![
    "unchecked",
    "ready",
    "reachable",
    "disabled",
    "shadowed",
  ].includes(health.state);
}

export function McpView() {
  const [runner, setRunner] = useState<Runner>("claude-code");
  const [directories, setDirectories] = useState<string[]>(["~"]);
  const [directory, setDirectory] = useState("~");
  const [canonicalDirectory, setCanonicalDirectory] = useState<string | null>(null);
  const [snapshot, setSnapshot] = useState<McpSnapshot | null>(null);
  const [filter, setFilter] = useState<Filter>("All");
  const [loading, setLoading] = useState(true);
  const [checking, setChecking] = useState(false);
  const [togglingId, setTogglingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [liveHealth, setLiveHealth] = useState<McpHealthResult[]>([]);
  const loadRequestRef = useRef(0);

  useEffect(() => {
    let live = true;
    void listProjects()
      .then((projects) => {
        if (!live) return;
        setDirectories([
          ...new Set(["~", ...projects.map((project) => project.path)]),
        ]);
      })
      .catch((reason) => {
        if (live) notify("Could not list projects", { description: String(reason) });
      });
    return () => {
      live = false;
    };
  }, []);

  const load = useCallback(
    async (fresh = false) => {
      const request = ++loadRequestRef.current;
      setLoading(true);
      setError(null);
      setCanonicalDirectory(null);
      try {
        const canonical = await canonicalContextDirectory(directory);
        const next = await scanMcp(fresh, canonical);
        if (request !== loadRequestRef.current) return;
        setCanonicalDirectory(canonical);
        setSnapshot(next);
        setLiveHealth([]);
      } catch (reason) {
        if (request !== loadRequestRef.current) return;
        const message = reason instanceof Error ? reason.message : String(reason);
        setError(message);
        notify("Could not read MCP configuration", { description: message });
      } finally {
        if (request === loadRequestRef.current) setLoading(false);
      }
    },
    [directory],
  );

  useEffect(() => {
    void load(false);
  }, [load]);

  const contextDirectory = canonicalDirectory ?? directory;
  const effective = useMemo(
    () =>
      (snapshot?.servers ?? []).filter(
        (server) =>
          server.runner === runner && sameDirectory(server.cwd, contextDirectory),
      ),
    [snapshot, runner, contextDirectory],
  );

  const healthById = useMemo(() => {
    const values = new Map<string, McpHealthResult>();
    for (const result of snapshot?.healthResults ?? []) values.set(result.id, result);
    for (const result of liveHealth) values.set(result.id, result);
    return values;
  }, [snapshot, liveHealth]);

  const rows = useMemo(
    () =>
      effective.filter((server) => {
        const health = healthById.get(server.id)?.health ?? server.health;
        if (filter === "Enabled") return server.state === "enabled";
        if (filter === "Disabled") return server.state === "disabled";
        if (filter === "Needs attention") {
          return server.state === "invalid" || healthNeedsAttention(health);
        }
        return true;
      }),
    [effective, filter, healthById],
  );

  const runnerOnlyResults = useMemo(() => {
    const known = new Set(effective.map((server) => server.id));
    return [...healthById.values()].filter(
      (result) =>
        result.runner === runner &&
        sameDirectory(result.cwd, contextDirectory) &&
        result.runnerProvided &&
        !known.has(result.id),
    );
  }, [effective, healthById, runner, contextDirectory]);

  const chooseDirectory = async () => {
    const chosen = await openDialog({ directory: true, multiple: false });
    if (typeof chosen !== "string") return;
    setDirectories((current) =>
      current.includes(chosen) ? current : [...current, chosen],
    );
    setDirectory(chosen);
  };

  const check = async () => {
    const approved = window.confirm(
      "Check MCP servers now? This can start local server processes and contact configured network endpoints.",
    );
    if (!approved) return;
    setChecking(true);
    try {
      const result = await checkMcpHealth(
        runner,
        contextDirectory,
        effective.map((server) => server.id),
      );
      setLiveHealth(result.results);
      notify(result.complete ? "MCP check complete" : "MCP check is incomplete", {
        description: `${result.results.length} server results`,
      });
    } catch (reason) {
      notify("Could not check MCP servers", { description: String(reason) });
    } finally {
      setChecking(false);
    }
  };

  const toggle = async (server: McpServer, enabled: boolean) => {
    const revision = server.toggle.revision;
    if (!server.toggle.writable || revision === null) return;
    if (
      server.toggle.sharedProjectFile &&
      !window.confirm(
        `Change ${server.name} in the shared project configuration? Other tools and people using this project can observe the change.`,
      )
    ) {
      return;
    }
    setTogglingId(server.id);
    try {
      const outcome = await setMcpEnabled(
        server.id,
        server.cwd ?? contextDirectory,
        enabled,
        revision,
      );
      switch (outcome.status) {
        case "written":
        case "unchanged":
          notify(`${server.name} ${enabled ? "enabled" : "disabled"}`);
          await load(true);
          break;
        case "conflict":
          notify("MCP configuration changed outside Aviary", {
            description: "Rescan and try the change again.",
          });
          await load(true);
          break;
        case "unavailable":
          notify("This server cannot be changed here", {
            description: outcome.reason.split("-").join(" "),
          });
          break;
        case "not-found":
          notify("The server declaration moved or was removed", {
            description: "A fresh scan will update the list.",
          });
          await load(true);
          break;
      }
    } catch (reason) {
      notify("Could not update MCP configuration", { description: String(reason) });
    } finally {
      setTogglingId(null);
    }
  };

  const attention = effective.filter((server) => {
    const health = healthById.get(server.id)?.health ?? server.health;
    return server.state === "invalid" || healthNeedsAttention(health);
  }).length;

  return (
    <div className="flex flex-col gap-[18px] p-[26px]">
      <PageHeader
        title="MCP Servers"
        subtitle={
          snapshot
            ? `${effective.length} effective servers, ${attention} need attention, scanned in ${snapshot.scannedMs}ms`
            : "Reading runner configuration"
        }
        action={
          <div className="flex flex-wrap items-center justify-end gap-2">
            <NativeSelect
              size="sm"
              aria-label="Runner"
              value={runner}
              onChange={(event) => setRunner(event.target.value as Runner)}
            >
              {RUNNERS.map((value) => (
                <NativeSelectOption key={value} value={value}>
                  {RUNNER_LABEL[value]}
                </NativeSelectOption>
              ))}
            </NativeSelect>
            <NativeSelect
              size="sm"
              aria-label="Working directory"
              className="max-w-[240px]"
              value={directory}
              onChange={(event) => setDirectory(event.target.value)}
            >
              {directories.map((value) => (
                <NativeSelectOption key={value} value={value}>
                  {tilde(value)}
                </NativeSelectOption>
              ))}
            </NativeSelect>
            <Button
              size="icon-sm"
              variant="outline"
              title="Choose working directory"
              aria-label="Choose working directory"
              onClick={() => void chooseDirectory()}
            >
              <HugeiconsIcon icon={FolderOpenIcon} size={14} strokeWidth={1.8} />
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={checking || loading}
              onClick={() => void check()}
            >
              <HugeiconsIcon icon={ServerStack01Icon} size={14} strokeWidth={1.8} />
              {checking ? "Checking…" : "Check health"}
            </Button>
            <Button
              size="icon-sm"
              variant="outline"
              title="Rescan configuration"
              aria-label="Rescan configuration"
              disabled={checking || loading}
              onClick={() => void load(true)}
            >
              <HugeiconsIcon icon={RefreshIcon} size={14} strokeWidth={1.8} />
            </Button>
          </div>
        }
      />

      <Segmented
        options={FILTERS}
        value={filter}
        onChange={setFilter}
        layoutId="mcp-filter"
      />

      {loading && !snapshot ? (
        <div className="space-y-1.5">
          {Array.from({ length: 5 }, (_, index) => (
            <Skeleton key={index} className="h-[88px] rounded-[10px]" />
          ))}
        </div>
      ) : error && !snapshot ? (
        <div className="rounded-[12px] border border-destructive/30 bg-destructive/10 p-5">
          <p className="text-[13px] font-medium">MCP configuration could not be read</p>
          <p className="mt-1 break-words text-xs text-muted-foreground">{error}</p>
          <Button className="mt-4" size="sm" variant="outline" onClick={() => void load(true)}>
            Try again
          </Button>
        </div>
      ) : (
        <div className="space-y-1.5">
          {rows.map((server) => (
            <ServerRow
              key={server.id}
              server={server}
              health={healthById.get(server.id)?.health ?? server.health}
              toggling={togglingId === server.id}
              onToggle={(enabled) => void toggle(server, enabled)}
            />
          ))}
          {filter === "All"
            ? runnerOnlyResults.map((result) => (
                <RunnerProvidedRow key={result.id} result={result} />
              ))
            : null}
          {rows.length === 0 &&
          (filter !== "All" || runnerOnlyResults.length === 0) ? (
            <div className="rounded-[10px] border border-dashed border-border px-5 py-10 text-center">
              <p className="text-[13px] font-medium">No effective servers</p>
              <p className="mt-1 text-xs text-muted-foreground">
                No server matches this runner, directory, and filter.
              </p>
            </div>
          ) : null}
        </div>
      )}

      <p className="text-[11px] leading-relaxed text-tertiary">
        Inventory scans read local declarations only. Health checks are explicit
        because they can start stdio processes or contact configured endpoints.
        Aviary returns launcher categories, endpoint shape, and key names; it
        never sends arguments, URLs, values, probe errors, or tool schemas to the
        webview.
      </p>
    </div>
  );
}

function ServerRow({
  server,
  health,
  toggling,
  onToggle,
}: {
  server: McpServer;
  health: McpHealth;
  toggling: boolean;
  onToggle: (enabled: boolean) => void;
}) {
  const definitionTokens = health.tools.definitions.tokens;
  const enabled = server.state === "enabled";

  return (
    <div
      className={cn(
        "av-hover-grad grid grid-cols-[18px_minmax(0,1fr)_180px_104px] items-center gap-3 rounded-[10px] border border-border bg-card px-3.5 py-3 transition-colors hover:border-border-strong",
        server.state === "disabled" && "opacity-65",
      )}
    >
      <StatusDot status={healthTone(health.state)} />

      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2">
          <span className="truncate text-[13px] font-medium">{server.name}</span>
          <Badge>{server.source}</Badge>
          <Badge>{server.transport.kind === "remote" ? server.transport.transport : server.transport.kind}</Badge>
          {server.shadowedDeclarationIds.length > 0 ? (
            <Badge tone="warn">
              shadows {server.shadowedDeclarationIds.length}
            </Badge>
          ) : null}
        </div>
        <p className="mt-1 truncate font-mono text-[10px] text-tertiary">
          {transportLabel(server.transport)}
        </p>
        <p className="mt-1 text-[10px] text-tertiary">
          effective {server.id.slice(0, 10)} / declaration {server.declarationId.slice(0, 10)}
        </p>
      </div>

      <div className="text-right">
        <p className="text-[11px] font-medium capitalize">{healthLabel(health)}</p>
        <p className="mt-1 text-[10px] text-tertiary">
          {health.tools.count === null
            ? "Tool count unknown"
            : `${health.tools.count} ${health.tools.count === 1 ? "tool" : "tools"}`}
          {definitionTokens === null
            ? ""
            : `, ${definitionTokens.toLocaleString()} schema tokens`}
        </p>
        {health.stale ? (
          <p className="mt-0.5 text-[9px] text-gold">Cached result is stale</p>
        ) : null}
      </div>

      <div className="flex items-center justify-end gap-2">
        <span className="text-[10px] text-tertiary">
          {server.toggle.writable
            ? enabled
              ? "Enabled"
              : "Disabled"
            : toggleReason(server)}
        </span>
        <Switch
          size="sm"
          checked={enabled}
          disabled={!server.toggle.writable || toggling}
          aria-label={`${enabled ? "Disable" : "Enable"} ${server.name}`}
          onCheckedChange={onToggle}
        />
      </div>
    </div>
  );
}

function RunnerProvidedRow({ result }: { result: McpHealthResult }) {
  return (
    <div className="grid grid-cols-[18px_minmax(0,1fr)_180px_104px] items-center gap-3 rounded-[10px] border border-border bg-card px-3.5 py-3">
      <StatusDot status={healthTone(result.health.state)} />
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <span className="truncate text-[13px] font-medium">{result.name}</span>
          <Badge>runner provided</Badge>
        </div>
        <p className="mt-1 text-[10px] text-tertiary">
          Reported by {RUNNER_LABEL[result.runner]}, with no writable declaration
        </p>
      </div>
      <div className="text-right text-[11px] capitalize">
        {healthLabel(result.health)}
      </div>
      <div className="text-right text-[10px] text-tertiary">Runner managed</div>
    </div>
  );
}

function Badge({
  children,
  tone = "muted",
}: {
  children: React.ReactNode;
  tone?: "muted" | "warn";
}) {
  return (
    <span
      className={cn(
        "rounded-[5px] border px-1.5 py-0.5 font-mono text-[9px] font-medium",
        tone === "warn"
          ? "border-gold/25 bg-gold/10 text-gold"
          : "border-border bg-hover text-tertiary",
      )}
    >
      {children}
    </span>
  );
}

function toggleReason(server: McpServer) {
  if (server.source === "plugin") return "Plugin managed";
  if (server.toggle.unavailableReason) {
    return server.toggle.unavailableReason.split("-").join(" ");
  }
  return "Read only";
}
