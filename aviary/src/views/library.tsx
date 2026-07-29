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
  ArrowDown01Icon,
  ArrowRight01Icon,
} from "@hugeicons/core-free-icons";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuGroup,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuCheckboxItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useLibrary } from "@/lib/use-library";
import { RUNNER_LABEL, type Entry, type Kind, type Runner } from "@/lib/api";
import { PageHeader, SectionLabel, Segmented } from "@/components/screen-parts";
import { notify } from "@/lib/notify";
import { EntryDetail } from "@/components/entry-detail";
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

const GROUP_BY = ["None", "Pack", "Kind", "Runner", "Source"] as const;
type GroupBy = (typeof GROUP_BY)[number];

const DENSITY = ["Comfortable", "Compact"] as const;
type Density = (typeof DENSITY)[number];

const SORT_BY = ["Name", "Recently changed", "Size"] as const;
type SortBy = (typeof SORT_BY)[number];

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

function tilde(p: string) {
  return p.replace(/^\/Users\/[^/]+/, "~");
}

function EntryRow({
  entry,
  density,
  selected,
  onSelect,
}: {
  entry: Entry;
  density: Density;
  selected: boolean;
  onSelect: () => void;
}) {
  const meta = KIND_META[entry.kind];
  const compact = density === "Compact";

  return (
    <button
      type="button"
      onClick={onSelect}
      aria-current={selected ? "true" : undefined}
      className={cn(
        "av-hover-grad flex w-full items-center gap-3 rounded-[10px] border text-left transition-colors",
        selected
          ? "av-selected-wash border-violet/40 bg-selected"
          : "border-border bg-card hover:border-border-strong",
        compact ? "px-3 py-1.5" : "px-3 py-2.5",
      )}
    >
      <span
        className={cn(
          "flex shrink-0 items-center justify-center rounded-lg bg-hover",
          compact ? "size-[22px]" : "size-[30px]",
        )}
      >
        <HugeiconsIcon
          icon={meta.icon}
          size={compact ? 13 : 15}
          strokeWidth={1.5}
          className={meta.color}
        />
      </span>

      <span className="min-w-0 flex-1 space-y-0.5">
        <span className="flex items-center gap-2">
          <span className="truncate text-[13px] font-medium">{entry.name}</span>
          {entry.source === "plugin" && entry.group && (
            <span className="shrink-0 rounded border border-border px-1 py-px text-[9px] font-medium uppercase tracking-wide text-tertiary">
              {entry.group}
            </span>
          )}
          {entry.project && (
            <span className="shrink-0 rounded border border-border px-1 py-px text-[9px] font-medium uppercase tracking-wide text-tertiary">
              {entry.project}
            </span>
          )}
        </span>
        {!compact && (
          <span className="block truncate text-xs text-muted-foreground">
            {entry.description || tilde(entry.path)}
          </span>
        )}
      </span>

      {!compact && (
        <span className="shrink-0 rounded-full border border-border bg-elevated px-2.5 py-1 text-[11px] font-medium text-muted-foreground">
          {entry.kind}
        </span>
      )}
      {entry.runners.map((r) => (
        <RunnerChip key={r} runner={r} />
      ))}
    </button>
  );
}

function groupKey(entry: Entry, by: GroupBy): string {
  switch (by) {
    case "Pack":
      return entry.group ?? "Personal";
    case "Kind":
      return entry.kind;
    case "Runner":
      return entry.runners.length === 2
        ? "Both runners"
        : RUNNER_LABEL[entry.runners[0]];
    case "Source":
      return entry.source;
    default:
      return "";
  }
}

export function LibraryView() {
  const { data, error, loading, refresh } = useLibrary();
  const [filter, setFilter] = useState<Filter>("All");
  const [query, setQuery] = useState("");
  const [groupBy, setGroupBy] = useState<GroupBy>("Pack");
  const [density, setDensity] = useState<Density>("Comfortable");
  const [sortBy, setSortBy] = useState<SortBy>("Name");
  const [showPlugins, setShowPlugins] = useState(false);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<Entry | null>(null);

  const visible = useMemo(() => {
    if (!data) return [];
    const q = query.trim().toLowerCase();
    const rows = data.entries.filter((e) => {
      if (!showPlugins && e.source === "plugin") return false;
      if (filter !== "All" && e.kind !== FILTER_KIND[filter]) return false;
      if (!q) return true;
      return (
        e.name.toLowerCase().includes(q) ||
        e.description.toLowerCase().includes(q) ||
        e.path.toLowerCase().includes(q) ||
        (e.group ?? "").toLowerCase().includes(q)
      );
    });

    return rows.sort((a, b) => {
      if (sortBy === "Recently changed") return b.modified - a.modified;
      if (sortBy === "Size") return b.bytes - a.bytes;
      return a.name.toLowerCase().localeCompare(b.name.toLowerCase());
    });
  }, [data, filter, query, showPlugins, sortBy]);

  const groups = useMemo(() => {
    if (groupBy === "None") return [["", visible]] as [string, Entry[]][];
    const map = new Map<string, Entry[]>();
    for (const e of visible) {
      const k = groupKey(e, groupBy);
      if (!map.has(k)) map.set(k, []);
      map.get(k)!.push(e);
    }
    // Largest packs first — they are the ones worth collapsing.
    return [...map.entries()].sort((a, b) => b[1].length - a[1].length);
  }, [visible, groupBy]);

  const toggleGroup = (key: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      next.has(key) ? next.delete(key) : next.add(key);
      return next;
    });

  const pluginCount = data?.entries.filter((e) => e.source === "plugin").length ?? 0;

  const subtitle = data
    ? `${visible.length} of ${data.entries.length} entries · ${data.runners
        .filter((r) => r.detected)
        .map((r) => r.label)
        .join(" and ")} · ${data.scannedMs}ms`
    : loading
      ? "Scanning your machine…"
      : "Could not scan";

  return (
    <div className="flex h-full min-h-0 gap-4 p-[26px]">
      <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-[18px] overflow-y-auto pr-1">
      <PageHeader
        title="Library"
        subtitle={subtitle}
        action={
          <div className="flex items-center gap-2">
            <ViewMenu
              groupBy={groupBy}
              setGroupBy={setGroupBy}
              density={density}
              setDensity={setDensity}
              sortBy={sortBy}
              setSortBy={setSortBy}
              showPlugins={showPlugins}
              setShowPlugins={setShowPlugins}
              pluginCount={pluginCount}
            />
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
          </div>
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
          placeholder="Filter by name, description, pack or path…"
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
        <div className="space-y-4">
          {groups.map(([key, rows]) => {
            const isCollapsed = collapsed.has(key);
            return (
              <div key={key || "all"} className="space-y-2">
                {key ? (
                  <button
                    type="button"
                    onClick={() => toggleGroup(key)}
                    className="flex w-full items-center gap-2 rounded-md py-0.5 text-left transition-opacity hover:opacity-80"
                  >
                    <HugeiconsIcon
                      icon={isCollapsed ? ArrowRight01Icon : ArrowDown01Icon}
                      size={13}
                      strokeWidth={2}
                      className="text-tertiary"
                    />
                    <SectionLabel>{key.toUpperCase()}</SectionLabel>
                    <span className="rounded-full bg-hover px-1.5 py-px font-mono text-[10px] text-tertiary">
                      {rows.length}
                    </span>
                  </button>
                ) : (
                  <SectionLabel>
                    {rows.length} {rows.length === 1 ? "ENTRY" : "ENTRIES"}
                  </SectionLabel>
                )}

                {!isCollapsed && (
                  <div className={cn(density === "Compact" ? "space-y-1" : "space-y-1.5")}>
                    {rows.map((e) => (
                      <EntryRow
                        key={e.id}
                        entry={e}
                        density={density}
                        selected={selected?.id === e.id}
                        onSelect={() =>
                          setSelected((cur) => (cur?.id === e.id ? null : e))
                        }
                      />
                    ))}
                  </div>
                )}
              </div>
            );
          })}

          {visible.length === 0 && (
            <p className="rounded-[10px] border border-dashed border-border px-4 py-8 text-center text-[13px] text-muted-foreground">
              Nothing matches that filter.
            </p>
          )}
        </div>
      )}
      </div>

      {selected && (
        <EntryDetail entry={selected} onClose={() => setSelected(null)} />
      )}
    </div>
  );
}

function ViewMenu(props: {
  groupBy: GroupBy;
  setGroupBy: (v: GroupBy) => void;
  density: Density;
  setDensity: (v: Density) => void;
  sortBy: SortBy;
  setSortBy: (v: SortBy) => void;
  showPlugins: boolean;
  setShowPlugins: (v: boolean) => void;
  pluginCount: number;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger className="flex items-center gap-2 rounded-full border border-border bg-elevated px-3 py-1.5 text-xs font-medium transition-colors hover:border-border-strong">
        View
        <HugeiconsIcon icon={ArrowDown01Icon} size={12} strokeWidth={2} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-52">
        <DropdownMenuRadioGroup
          value={props.groupBy}
          onValueChange={(v) => props.setGroupBy(v as GroupBy)}
        >
          <DropdownMenuLabel>Group by</DropdownMenuLabel>
          {GROUP_BY.map((g) => (
            <DropdownMenuRadioItem key={g} value={g}>
              {g}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>

        <DropdownMenuSeparator />
        <DropdownMenuRadioGroup
          value={props.sortBy}
          onValueChange={(v) => props.setSortBy(v as SortBy)}
        >
          <DropdownMenuLabel>Sort by</DropdownMenuLabel>
          {SORT_BY.map((s) => (
            <DropdownMenuRadioItem key={s} value={s}>
              {s}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>

        <DropdownMenuSeparator />
        <DropdownMenuRadioGroup
          value={props.density}
          onValueChange={(v) => props.setDensity(v as Density)}
        >
          <DropdownMenuLabel>Density</DropdownMenuLabel>
          {DENSITY.map((d) => (
            <DropdownMenuRadioItem key={d} value={d}>
              {d}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>

        <DropdownMenuSeparator />
        <DropdownMenuGroup>
          <DropdownMenuCheckboxItem
            checked={props.showPlugins}
            onCheckedChange={props.setShowPlugins}
          >
            Show plugin skills ({props.pluginCount})
          </DropdownMenuCheckboxItem>
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
