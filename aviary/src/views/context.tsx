import { useCallback, useEffect, useMemo, useState } from "react";
import * as motionReact from "motion/react";
import { HugeiconsIcon } from "@hugeicons/react";
import { ArrowDown01Icon } from "@hugeicons/core-free-icons";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import {
  PageHeader,
  SectionLabel,
  StaggerList,
  StaggerRow,
} from "@/components/screen-parts";
import { notify } from "@/lib/notify";
import {
  listProjects,
  resolveContext,
  RUNNER_LABEL,
  type ContextLayer,
  type ContextScope,
  type ResolvedContext,
  type Runner,
} from "@/lib/api";

const { motion } = motionReact;

/**
 * Nominal window, used only for the secondary "share of a window" figure.
 * The headline number is deliberately "tokens from your configuration" — the
 * built-in system prompt and MCP tool schemas are not readable from disk, so
 * claiming a true share of the window would be a fabrication.
 */
const WINDOW = 200_000;

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

function tilde(p: string) {
  return p.replace(/^\/Users\/[^/]+/, "~");
}

export function ContextView() {
  const [runner, setRunner] = useState<Runner>("claude-code");
  const [dirs, setDirs] = useState<string[]>(["~"]);
  const [dir, setDir] = useState("~");
  const [resolved, setResolved] = useState<ResolvedContext | null>(null);
  const [loading, setLoading] = useState(true);

  // Directory choices are the registered projects, plus home so the picker is
  // never empty on a fresh install.
  useEffect(() => {
    listProjects()
      .then((projects) => {
        const paths = ["~", ...projects.map((p) => p.path)];
        setDirs(paths);
        setDir((current) => (paths.includes(current) ? current : paths[0]));
      })
      .catch((e) =>
        notify("Could not list projects", { description: String(e) }),
      );
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setResolved(await resolveContext(runner, dir));
    } catch (e) {
      setResolved(null);
      notify("Could not resolve context", { description: String(e) });
    } finally {
      setLoading(false);
    }
  }, [runner, dir]);

  useEffect(() => {
    void load();
  }, [load]);

  const layers = resolved?.layers ?? [];
  // Unmeasured layers carry no size, so they stay out of the meter and out of
  // the bar scale — a 0-token row would otherwise flatten every real bar.
  const measured = useMemo(() => layers.filter((l) => l.measured), [layers]);
  const max = useMemo(
    () => Math.max(1, ...measured.map((l) => l.tokens)),
    [measured],
  );

  const total = resolved?.total ?? 0;
  const pct = ((total / WINDOW) * 100).toFixed(1);
  const nothingLoads = !loading && resolved !== null && measured.length === 0;

  return (
    <div className="flex flex-col gap-[18px] p-[26px]">
      <PageHeader
        title="Context"
        subtitle="Exactly what gets loaded, in order, before your first message"
        action={
          <div className="flex items-center gap-2">
            <Selector
              label="Runner"
              value={RUNNER_LABEL[runner]}
              options={RUNNERS.map((r) => RUNNER_LABEL[r])}
              onChange={(label) =>
                setRunner(
                  RUNNERS.find((r) => RUNNER_LABEL[r] === label) ?? runner,
                )
              }
            />
            <Selector
              label="Directory"
              value={tilde(dir)}
              options={dirs.map(tilde)}
              onChange={(shown) =>
                setDir(dirs.find((d) => tilde(d) === shown) ?? shown)
              }
            />
          </div>
        }
      />

      {loading ? (
        <>
          <Skeleton className="h-[128px] rounded-[14px]" />
          <Skeleton className="h-[220px] rounded-[14px]" />
        </>
      ) : nothingLoads ? (
        <EmptyState runner={runner} dir={tilde(dir)} />
      ) : (
        <>
          <div className="space-y-4 rounded-[14px] border border-border bg-card p-5">
            <div className="flex items-center gap-4">
              <div className="flex-1 space-y-1">
                <p className="text-[15px] font-semibold">
                  {total.toLocaleString()} tokens from your configuration
                </p>
                <p className="text-xs text-muted-foreground">
                  {measured.length} measured{" "}
                  {measured.length === 1 ? "layer" : "layers"} · {pct}% of a 200K
                  window
                  {resolved && resolved.unmeasured > 0 && (
                    <> · {resolved.unmeasured} not measurable from disk</>
                  )}
                </p>
              </div>
              <span className="font-mono text-[26px] font-semibold tabular-nums">
                {pct}%
              </span>
            </div>

            {/* Stacked meter — measured layers only */}
            <div className="flex h-2.5 gap-0.5 overflow-hidden">
              {measured.map((l) => (
                <motion.div
                  key={l.scope + l.path}
                  layout
                  animate={{ flexGrow: l.tokens }}
                  transition={{ type: "spring", stiffness: 260, damping: 34 }}
                  className="rounded-[3px]"
                  style={{ backgroundColor: SCOPE_COLOR[l.scope], flexBasis: 0 }}
                />
              ))}
            </div>

            <div className="flex flex-wrap gap-4">
              {measured.map((l) => (
                <span
                  key={l.scope + l.path}
                  className="flex items-center gap-1.5 text-[11px] text-muted-foreground"
                >
                  <span
                    className="size-1.5 rounded-full"
                    style={{ backgroundColor: SCOPE_COLOR[l.scope] }}
                  />
                  {l.scope}
                </span>
              ))}
            </div>
          </div>

          <SectionLabel>RESOLUTION ORDER</SectionLabel>

          <StaggerList className="space-y-1.5" key={`${runner}:${dir}`}>
            {layers.map((l, i) => (
              <LayerRow
                key={`${l.scope}:${l.path}:${i}`}
                layer={l}
                index={i}
                max={max}
              />
            ))}
          </StaggerList>

          {resolved && (
            <p className="text-[11px] text-tertiary">
              Resolved in {resolved.scannedMs}ms · counts are estimates from the
              o200k encoder
            </p>
          )}
        </>
      )}
    </div>
  );
}

function LayerRow({
  layer,
  index,
  max,
}: {
  layer: ContextLayer;
  index: number;
  max: number;
}) {
  const color = SCOPE_COLOR[layer.scope];

  return (
    <StaggerRow
      interactive={false}
      className="av-hover-grad flex items-start gap-3.5 rounded-[10px] border border-border bg-card px-3 py-2.5 transition-colors hover:border-border-strong"
    >
      <span className="mt-[2px] flex size-[22px] shrink-0 items-center justify-center rounded-md bg-hover font-mono text-[10px] text-muted-foreground">
        {index + 1}
      </span>

      <span className="mt-[4px] flex w-[78px] shrink-0 items-center gap-2">
        <span
          className="size-[7px] rounded-full"
          style={
            layer.measured
              ? { backgroundColor: color }
              : { border: `1px dashed ${color}` }
          }
        />
        <span className="text-[11px] font-medium text-muted-foreground">
          {layer.scope}
        </span>
      </span>

      <span className="min-w-0 flex-1 space-y-0.5">
        <span className="block truncate text-xs font-medium">{layer.label}</span>
        <span className="block truncate font-mono text-[11px] text-muted-foreground">
          {tilde(layer.path)}
        </span>
        {layer.note && (
          <span className="block text-[11px] text-tertiary">{layer.note}</span>
        )}
      </span>

      <span className="mt-[7px] h-1.5 w-[150px] shrink-0 overflow-hidden rounded-full bg-hover">
        {layer.measured && (
          <motion.span
            className="block h-full rounded-full"
            style={{ backgroundColor: color }}
            animate={{ width: `${(layer.tokens / max) * 100}%` }}
            transition={{ duration: 0.28, ease: [0.22, 1, 0.36, 1] }}
          />
        )}
      </span>

      <span className="mt-[5px] w-14 shrink-0 text-right font-mono text-xs tabular-nums text-muted-foreground">
        {layer.measured ? layer.tokens.toLocaleString() : "—"}
      </span>
    </StaggerRow>
  );
}

/** A directory that loads nothing is a real answer, not an error. */
function EmptyState({ runner, dir }: { runner: Runner; dir: string }) {
  const file = runner === "claude-code" ? "CLAUDE.md" : "AGENTS.md";
  return (
    <div className="rounded-[14px] border border-border bg-card p-8 text-center">
      <p className="text-[15px] font-semibold">Nothing loads here</p>
      <p className="mx-auto mt-1.5 max-w-[430px] text-xs text-muted-foreground">
        {RUNNER_LABEL[runner]} finds no {file}, skills, or memory for{" "}
        <span className="font-mono">{dir}</span>. It would start from its
        built-in system prompt alone.
      </p>
    </div>
  );
}

function Selector({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: string[];
  onChange: (v: string) => void;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger className="flex items-center gap-2 rounded-[9px] border border-border bg-elevated px-3 py-[7px] transition-colors hover:border-border-strong">
        <span className="text-[11px] text-tertiary">{label}</span>
        <span className="max-w-[220px] truncate text-xs font-medium">
          {value}
        </span>
        <HugeiconsIcon icon={ArrowDown01Icon} size={11} strokeWidth={2} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuRadioGroup value={value} onValueChange={onChange}>
          {options.map((o) => (
            <DropdownMenuRadioItem key={o} value={o}>
              {o}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
