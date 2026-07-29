import { useCallback, useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import { RefreshIcon } from "@hugeicons/core-free-icons";
import { Skeleton } from "@/components/ui/skeleton";
import { PageHeader, Segmented, StatusDot } from "@/components/screen-parts";
import { notify } from "@/lib/notify";
import { scanMcp, RUNNER_LABEL, type McpServer } from "@/lib/api";
import { cn } from "@/lib/utils";

const SCOPES = ["All", "Claude Code", "Codex", "Disabled"] as const;
type Scope = (typeof SCOPES)[number];

function tilde(p: string) {
  return p.replace(/^\/Users\/[^/]+/, "~");
}

/** What the server points at — a URL, or the command that runs it. */
function target(s: McpServer) {
  if (s.url) return s.url;
  const cmd = [s.command ?? "", ...s.args].join(" ").trim();
  return tilde(cmd) || "—";
}

function Badge({
  children,
  tone = "muted",
  outlined,
}: {
  children: React.ReactNode;
  tone?: "muted" | "blue" | "violet";
  outlined?: boolean;
}) {
  return (
    <span
      className={cn(
        "shrink-0 rounded-[5px] px-1.5 py-0.5 font-mono text-[9px] font-medium",
        outlined ? "border border-border" : "bg-hover",
        tone === "blue" && "text-blue",
        tone === "violet" && "text-violet",
        tone === "muted" && "text-tertiary",
      )}
    >
      {children}
    </span>
  );
}

function ServerRow({ s }: { s: McpServer }) {
  return (
    <div
      className={cn(
        "av-hover-grad flex items-center gap-3.5 rounded-[10px] border border-border bg-card px-3.5 py-3 transition-colors hover:border-border-strong",
        !s.enabled && "opacity-55",
      )}
    >
      <StatusDot status={s.enabled ? "ok" : "warn"} />

      <span className="min-w-0 flex-1 space-y-[5px]">
        <span className="flex flex-wrap items-center gap-2">
          <span className="text-[13px] font-medium">{s.name}</span>
          <Badge tone={s.transport === "stdio" ? "muted" : "blue"}>
            {s.transport}
          </Badge>
          <Badge outlined tone={s.origin ? "violet" : "muted"}>
            {s.origin ?? "user config"}
          </Badge>
        </span>
        <span className="block truncate font-mono text-[11px] text-tertiary">
          {target(s)}
        </span>
      </span>

      {s.envKeys.length > 0 && (
        <span
          className="shrink-0 rounded-full bg-hover px-2 py-1 font-mono text-[10px] text-tertiary"
          title={s.envKeys.join("\n")}
        >
          {s.envKeys.length} env
        </span>
      )}

      {s.runners.map((r) => (
        <span
          key={r}
          className="flex shrink-0 items-center gap-1.5 rounded-full border border-border bg-elevated px-2.5 py-1 text-[11px] font-medium text-muted-foreground"
        >
          <span
            className={cn(
              "size-1.5 rounded-full",
              r === "claude-code" ? "bg-claude" : "bg-codex",
            )}
          />
          {RUNNER_LABEL[r]}
        </span>
      ))}

      {/* A switch here would imply control the config cannot express: plugin
          servers arrive and leave with their plugin, and Claude's JSON has no
          enable flag at all. Stating the fact beats a dead toggle. */}
      <span className="w-[68px] shrink-0 text-right text-[10px] text-tertiary">
        {s.source === "plugin" ? "via plugin" : s.enabled ? "enabled" : "disabled"}
      </span>
    </div>
  );
}

export function McpView() {
  const [snap, setSnap] = useState<{
    servers: McpServer[];
    scannedMs: number;
  } | null>(null);
  const [scope, setScope] = useState<Scope>("All");

  const scan = useCallback(async () => {
    try {
      setSnap(await scanMcp());
    } catch (e) {
      notify("Could not read MCP config", {
        description: e instanceof Error ? e.message : String(e),
      });
    }
  }, []);

  useEffect(() => {
    void scan();
  }, [scan]);

  const visible = useMemo(() => {
    const all = snap?.servers ?? [];
    switch (scope) {
      case "Claude Code":
        return all.filter((s) => s.runners.includes("claude-code"));
      case "Codex":
        return all.filter((s) => s.runners.includes("codex"));
      case "Disabled":
        return all.filter((s) => !s.enabled);
      default:
        return all;
    }
  }, [snap, scope]);

  const fromPlugins =
    snap?.servers.filter((s) => s.source === "plugin").length ?? 0;
  const disabled = snap?.servers.filter((s) => !s.enabled).length ?? 0;

  return (
    <div className="flex flex-col gap-[18px] p-[26px]">
      <PageHeader
        title="MCP Servers"
        subtitle={
          snap
            ? `${snap.servers.length} servers · ${fromPlugins} from plugins · ${disabled} disabled · scanned in ${snap.scannedMs}ms`
            : "Reading MCP config…"
        }
        action={
          <button
            type="button"
            onClick={() => void scan()}
            className="flex shrink-0 items-center gap-1.5 rounded-full border border-border bg-elevated px-3.5 py-1.5 text-xs font-medium transition-colors hover:border-border-strong"
          >
            <HugeiconsIcon icon={RefreshIcon} size={13} strokeWidth={1.8} />
            Rescan
          </button>
        }
      />

      <Segmented
        options={SCOPES}
        value={scope}
        onChange={setScope}
        layoutId="mcp-scope"
      />

      {!snap ? (
        <div className="space-y-1.5">
          {Array.from({ length: 5 }).map((_, i) => (
            <Skeleton key={i} className="h-[58px] w-full rounded-[10px]" />
          ))}
        </div>
      ) : (
        <div className="space-y-1.5">
          {visible.map((s) => (
            <ServerRow key={s.name} s={s} />
          ))}
          {visible.length === 0 && (
            <p className="rounded-[10px] border border-dashed border-border px-4 py-8 text-center text-[13px] text-muted-foreground">
              No servers match that filter.
            </p>
          )}
        </div>
      )}

      <p className="text-[11px] leading-relaxed text-tertiary">
        Plugin-supplied servers come and go with their plugin, so they cannot be
        toggled here. Only servers in your own config carry an enable flag — and
        only Codex supports one. Environment variable <em>names</em> are shown;
        values are never read.
      </p>
    </div>
  );
}
