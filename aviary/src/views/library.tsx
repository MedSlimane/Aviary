import { useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  SparklesIcon,
  BotIcon,
  TextAlignLeftIcon,
  CommandIcon,
  Note01Icon,
  RefreshIcon,
  Alert02Icon,
} from "@hugeicons/core-free-icons";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { useLibrary } from "@/lib/use-library";
import { RUNNER_LABEL, type Entry, type Kind, type Runner } from "@/lib/api";
import { PageHeader, SectionLabel, Segmented } from "@/components/screen-parts";
import { notify } from "@/lib/notify";
import { cn } from "@/lib/utils";

const KIND_META: Record<Kind, { icon: typeof SparklesIcon; color: string }> = {
  skill: { icon: SparklesIcon, color: "text-violet" },
  agent: { icon: BotIcon, color: "text-teal" },
  command: { icon: CommandIcon, color: "text-blue" },
  prompt: { icon: TextAlignLeftIcon, color: "text-peach" },
  memory: { icon: Note01Icon, color: "text-gold" },
};

const FILTERS = ["All", "Skills", "Agents", "Commands", "Prompts", "Memory"] as const;
type Filter = (typeof FILTERS)[number];

const FILTER_KIND: Record<Exclude<Filter, "All">, Kind> = {
  Skills: "skill",
  Agents: "agent",
  Commands: "command",
  Prompts: "prompt",
  Memory: "memory",
};

function RunnerChip({ runner }: { runner: Runner }) {
  return (
    <span className="flex shrink-0 items-center gap-1.5 rounded-full border border-border bg-elevated px-2.5 py-1 text-[11px] font-medium text-muted-foreground">
      <span
        className={cn(
          "size-1.5 rounded-full",
          runner === "claude-code" ? "bg-claude" : "bg-codex",
        )}
      />
      {RUNNER_LABEL[runner]}
    </span>
  );
}

function EntryRow({ entry }: { entry: Entry }) {
  const meta = KIND_META[entry.kind];
  return (
    <button
      type="button"
      onClick={() =>
        notify(entry.name, { description: entry.path.replace(/^\/Users\/[^/]+/, "~") })
      }
      className="av-hover-grad flex w-full items-center gap-3 rounded-[10px] border border-border bg-card px-3 py-2.5 text-left transition-colors hover:border-border-strong"
    >
      <span className="flex size-[30px] shrink-0 items-center justify-center rounded-lg bg-hover">
        <HugeiconsIcon
          icon={meta.icon}
          size={15}
          strokeWidth={1.5}
          className={meta.color}
        />
      </span>

      <span className="min-w-0 flex-1 space-y-0.5">
        <span className="flex items-center gap-2">
          <span className="truncate text-[13px] font-medium">{entry.name}</span>
          {entry.source === "plugin" && (
            <span className="shrink-0 rounded border border-border px-1 py-px text-[9px] font-medium uppercase tracking-wide text-tertiary">
              plugin
            </span>
          )}
          {entry.project && (
            <span className="shrink-0 rounded border border-border px-1 py-px text-[9px] font-medium uppercase tracking-wide text-tertiary">
              {entry.project}
            </span>
          )}
        </span>
        <span className="block truncate text-xs text-muted-foreground">
          {entry.description || entry.path.replace(/^\/Users\/[^/]+/, "~")}
        </span>
      </span>

      <span className="shrink-0 rounded-full border border-border bg-elevated px-2.5 py-1 text-[11px] font-medium text-muted-foreground">
        {entry.kind}
      </span>
      {entry.runners.map((r) => (
        <RunnerChip key={r} runner={r} />
      ))}
    </button>
  );
}

export function LibraryView() {
  const { data, error, loading, refresh } = useLibrary();
  const [filter, setFilter] = useState<Filter>("All");
  const [query, setQuery] = useState("");

  const visible = useMemo(() => {
    if (!data) return [];
    const q = query.trim().toLowerCase();
    return data.entries.filter((e) => {
      if (filter !== "All" && e.kind !== FILTER_KIND[filter]) return false;
      if (!q) return true;
      return (
        e.name.toLowerCase().includes(q) ||
        e.description.toLowerCase().includes(q) ||
        e.path.toLowerCase().includes(q)
      );
    });
  }, [data, filter, query]);

  const subtitle = data
    ? `${data.entries.length} entries · ${data.runners
        .filter((r) => r.detected)
        .map((r) => r.label)
        .join(" and ")} · scanned in ${data.scannedMs}ms`
    : loading
      ? "Scanning your machine…"
      : "Could not scan";

  return (
    <div className="flex flex-col gap-[22px] p-[26px]">
      <PageHeader
        title="Library"
        subtitle={subtitle}
        action={
          <Button
            size="sm"
            variant="outline"
            className="rounded-full"
            onClick={() => {
              void refresh();
              notify("Rescanning…");
            }}
          >
            <HugeiconsIcon icon={RefreshIcon} size={14} strokeWidth={1.8} />
            Rescan
          </Button>
        }
      />

      {error && (
        <div className="flex items-center gap-3 rounded-[10px] border border-destructive/30 bg-destructive/10 px-3.5 py-3">
          <HugeiconsIcon
            icon={Alert02Icon}
            size={16}
            strokeWidth={1.8}
            className="text-destructive"
          />
          <p className="flex-1 text-[13px]">{error}</p>
        </div>
      )}

      <div className="flex items-center gap-2">
        <Segmented
          options={FILTERS}
          value={filter}
          onChange={setFilter}
          layoutId="library-filter"
        />
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Filter by name, description or path…"
          className="h-[34px] max-w-[320px] flex-1"
        />
      </div>

      {loading && !data ? (
        <div className="space-y-1.5">
          {Array.from({ length: 8 }).map((_, i) => (
            <Skeleton key={i} className="h-[54px] w-full rounded-[10px]" />
          ))}
        </div>
      ) : (
        <div className="space-y-3">
          <SectionLabel>
            {visible.length} {visible.length === 1 ? "ENTRY" : "ENTRIES"}
          </SectionLabel>
          <div className="space-y-1.5">
            {visible.map((e) => (
              <EntryRow key={e.id} entry={e} />
            ))}
            {visible.length === 0 && (
              <p className="rounded-[10px] border border-dashed border-border px-4 py-8 text-center text-[13px] text-muted-foreground">
                Nothing matches that filter.
              </p>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
