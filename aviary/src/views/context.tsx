import { useCallback, useEffect, useMemo, useState } from "react";
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
import { PageHeader, SectionLabel } from "@/components/screen-parts";
import { notify } from "@/lib/notify";
import {
  checkMcpHealth,
  listProjects,
  resolveContext,
  RUNNER_LABEL,
  type ContextLayer,
  type ContextScope,
  type ResolvedContext,
  type Runner,
  type TokenBasis,
} from "@/lib/api";
import { cn } from "@/lib/utils";

const SCOPE_COLOR: Record<ContextScope, string> = {
  system: "var(--av-text-tertiary)",
  user: "var(--av-accent-blue)",
  project: "var(--av-accent-violet)",
  local: "var(--av-accent-teal)",
  skills: "var(--av-accent-peach)",
  mcp: "var(--av-accent-coral)",
  memory: "var(--av-accent-gold)",
};

const RUNNERS = Object.keys(RUNNER_LABEL) as Runner[];

function tilde(path: string) {
  return path.replace(/^\/Users\/[^/]+/, "~");
}

function basisLabel(basis: TokenBasis) {
  switch (basis) {
    case "runner-exact":
      return "Runner reported";
    case "o200k-file-estimate":
      return "o200k file estimate";
    case "o200k-schema-estimate":
      return "o200k schema estimate";
    case "unavailable":
      return "Unavailable";
  }
}

export function ContextView() {
  const [runner, setRunner] = useState<Runner>("claude-code");
  const [directories, setDirectories] = useState<string[]>(["~"]);
  const [directory, setDirectory] = useState("~");
  const [resolved, setResolved] = useState<ResolvedContext | null>(null);
  const [loading, setLoading] = useState(true);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    void listProjects()
      .then((projects) => {
        if (!live) return;
        const next = ["~", ...projects.map((project) => project.path)];
        setDirectories([...new Set(next)]);
      })
      .catch((reason) => {
        if (live) {
          notify("Could not list projects", { description: String(reason) });
        }
      });
    return () => {
      live = false;
    };
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const value = await resolveContext(runner, directory);
      setResolved(value);
      if (value.cwd !== directory && value.cwd !== tilde(directory)) {
        setDirectories((current) =>
          current.includes(value.cwd) ? current : [...current, value.cwd],
        );
      }
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      setResolved(null);
      setError(message);
      notify("Could not resolve context", { description: message });
    } finally {
      setLoading(false);
    }
  }, [runner, directory]);

  useEffect(() => {
    void load();
  }, [load]);

  const chooseDirectory = async () => {
    const chosen = await openDialog({ directory: true, multiple: false });
    if (typeof chosen !== "string") return;
    setDirectories((current) =>
      current.includes(chosen) ? current : [...current, chosen],
    );
    setDirectory(chosen);
  };

  const checkDefinitions = async () => {
    const approved = window.confirm(
      "Check MCP servers now? This can start local server processes and contact configured network endpoints.",
    );
    if (!approved) return;
    setChecking(true);
    try {
      const health = await checkMcpHealth(
        runner,
        resolved?.cwd ?? directory,
      );
      notify(
        health.complete ? "MCP check complete" : "MCP check returned partial results",
        { description: `${health.results.length} server results` },
      );
      await load();
    } catch (reason) {
      notify("Could not check MCP servers", { description: String(reason) });
    } finally {
      setChecking(false);
    }
  };

  const included = useMemo(
    () =>
      (resolved?.layers ?? []).filter(
        (layer): layer is ContextLayer & { tokens: number } =>
          layer.tokens !== null && layer.includedInTotal,
      ),
    [resolved],
  );
  const maxTokens = useMemo(() => {
    let max = 1;
    for (const layer of included) max = Math.max(max, layer.tokens);
    return max;
  }, [included]);

  return (
    <div className="flex flex-col gap-[18px] p-[26px]">
      <PageHeader
        title="Context"
        subtitle="The instruction and tool-definition layers this runner can load"
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
              className="max-w-[250px]"
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
              aria-label="Choose working directory"
              title="Choose working directory"
              onClick={() => void chooseDirectory()}
            >
              <HugeiconsIcon icon={FolderOpenIcon} size={14} strokeWidth={1.8} />
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={checking || loading}
              onClick={() => void checkDefinitions()}
            >
              <HugeiconsIcon icon={ServerStack01Icon} size={14} strokeWidth={1.8} />
              {checking ? "Checking…" : "Check MCP"}
            </Button>
            <Button
              size="icon-sm"
              variant="outline"
              aria-label="Resolve again"
              title="Resolve again"
              disabled={checking || loading}
              onClick={() => void load()}
            >
              <HugeiconsIcon icon={RefreshIcon} size={14} strokeWidth={1.8} />
            </Button>
          </div>
        }
      />

      {loading ? (
        <ContextSkeleton />
      ) : error ? (
        <div className="rounded-[12px] border border-destructive/30 bg-destructive/10 p-5">
          <p className="text-[13px] font-medium">Context could not be resolved</p>
          <p className="mt-1 break-words text-xs text-muted-foreground">{error}</p>
          <Button className="mt-4" size="sm" variant="outline" onClick={() => void load()}>
            Try again
          </Button>
        </div>
      ) : resolved ? (
        <>
          <ContextSummary resolved={resolved} included={included} />

          <SectionLabel>RESOLUTION ORDER</SectionLabel>
          <div className="space-y-1.5">
            {resolved.layers.map((layer, index) => (
              <LayerRow
                key={`${layer.scope}:${layer.path}:${index}`}
                layer={layer}
                index={index}
                maxTokens={maxTokens}
              />
            ))}
          </div>

          <p className="text-[11px] leading-relaxed text-tertiary">
            Resolved in {resolved.scannedMs}ms. File and schema estimates use the
            o200k encoder. Unknown values have no numeric fallback.
          </p>
        </>
      ) : null}
    </div>
  );
}

function ContextSummary({
  resolved,
  included,
}: {
  resolved: ResolvedContext;
  included: Array<ContextLayer & { tokens: number }>;
}) {
  const totalLabel = resolved.totalComplete
    ? `${resolved.total.toLocaleString()} total tokens`
    : `${resolved.total.toLocaleString()}+ known tokens`;

  return (
    <div className="rounded-[14px] border border-border bg-card p-5">
      <div className="flex items-start justify-between gap-4">
        <div>
          <p className="text-[15px] font-semibold">{totalLabel}</p>
          <p className="mt-1 text-xs text-muted-foreground">
            {included.length} included {included.length === 1 ? "layer" : "layers"}
            {resolved.unmeasured > 0
              ? `, ${resolved.unmeasured} unmeasured or incomplete`
              : ", every listed layer measured"}
          </p>
        </div>
        <span
          className={cn(
            "rounded-md border px-2 py-1 font-mono text-[10px] font-medium",
            resolved.totalComplete
              ? "border-teal/25 bg-teal/10 text-teal"
              : "border-border bg-hover text-muted-foreground",
          )}
        >
          {resolved.totalComplete ? "complete" : "known subtotal"}
        </span>
      </div>

      {included.length > 0 ? (
        <div className="mt-4 flex h-2 gap-0.5 overflow-hidden rounded-sm">
          {included.map((layer, index) => (
            <span
              key={`${layer.scope}:${layer.path}:${index}`}
              className="min-w-px rounded-[2px]"
              style={{
                backgroundColor: SCOPE_COLOR[layer.scope],
                flexGrow: layer.tokens,
                flexBasis: 0,
              }}
            />
          ))}
        </div>
      ) : (
        <p className="mt-4 text-xs text-muted-foreground">
          No measurable layer is currently included in the total.
        </p>
      )}
    </div>
  );
}

function LayerRow({
  layer,
  index,
  maxTokens,
}: {
  layer: ContextLayer;
  index: number;
  maxTokens: number;
}) {
  const color = SCOPE_COLOR[layer.scope];
  const width = layer.tokens === null ? 0 : (layer.tokens / maxTokens) * 100;
  const state =
    layer.loaded === false
      ? "not loaded"
      : layer.loaded === null
        ? "load state unknown"
        : layer.includedInTotal
          ? "included"
          : "excluded";

  return (
    <div className="av-hover-grad grid grid-cols-[28px_84px_minmax(0,1fr)_150px_92px] items-start gap-3 rounded-[10px] border border-border bg-card px-3 py-2.5 transition-colors hover:border-border-strong">
      <span className="flex size-[22px] items-center justify-center rounded-md bg-hover font-mono text-[10px] text-muted-foreground">
        {index + 1}
      </span>

      <span className="mt-1 flex items-center gap-2">
        <span
          className="size-[7px] rounded-full"
          style={
            layer.tokens === null
              ? { border: `1px dashed ${color}` }
              : { backgroundColor: color }
          }
        />
        <span className="text-[11px] font-medium text-muted-foreground">
          {layer.scope}
        </span>
      </span>

      <span className="min-w-0">
        <span className="block truncate text-xs font-medium">{layer.label}</span>
        <span className="mt-0.5 block truncate font-mono text-[10px] text-tertiary">
          {tilde(layer.path)}
        </span>
        {layer.note ? (
          <span className="mt-1 block text-[11px] leading-relaxed text-muted-foreground">
            {layer.note}
          </span>
        ) : null}
      </span>

      <span className="mt-1.5">
        <span className="block h-1.5 overflow-hidden rounded-full bg-hover">
          {layer.tokens !== null ? (
            <span
              className="block h-full rounded-full"
              style={{ backgroundColor: color, width: `${width}%` }}
            />
          ) : null}
        </span>
        <span className="mt-1.5 block text-[9px] text-tertiary">
          {basisLabel(layer.basis)} · {state}
        </span>
      </span>

      <span className="mt-0.5 text-right font-mono text-xs tabular-nums text-muted-foreground">
        {layer.tokens === null ? "Not measured" : layer.tokens.toLocaleString()}
      </span>
    </div>
  );
}

function ContextSkeleton() {
  return (
    <>
      <Skeleton className="h-[104px] rounded-[14px]" />
      <Skeleton className="h-[18px] w-36" />
      <div className="space-y-1.5">
        {Array.from({ length: 5 }, (_, index) => (
          <Skeleton key={index} className="h-[66px] rounded-[10px]" />
        ))}
      </div>
    </>
  );
}
