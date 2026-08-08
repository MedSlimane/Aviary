import { useCallback, useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  RefreshIcon,
  PlusSignIcon,
  Cancel01Icon,
  Folder01Icon,
} from "@hugeicons/core-free-icons";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { PageHeader, SectionLabel, Segmented } from "@/components/screen-parts";
import { useLibrary } from "@/lib/use-library";
import { notify } from "@/lib/notify";
import {
  addProject,
  discoverProjects,
  removeProject,
  type Candidate,
} from "@/lib/api";
import { cn } from "@/lib/utils";

const RUNNER_DOT: Record<string, string> = {
  "Claude Code": "bg-claude",
  Codex: "bg-codex",
  Cursor: "bg-blue",
  Copilot: "bg-teal",
  Gemini: "bg-gold",
  Windsurf: "bg-peach",
};

const SCOPES = ["All", "Tracked", "Discovered"] as const;
type Scope = (typeof SCOPES)[number];

function tilde(p: string) {
  return p.replace(/^\/Users\/[^/]+/, "~");
}

function ProjectRow({
  c,
  entries,
  onToggle,
  busy,
}: {
  c: Candidate;
  entries: number;
  onToggle: (c: Candidate) => void;
  busy: boolean;
}) {
  return (
    <div
      className={cn(
        "av-hover-grad flex items-center gap-3.5 rounded-[10px] border bg-card px-3.5 py-3 transition-colors",
        c.registered
          ? "border-border-strong"
          : "border-border hover:border-border-strong",
      )}
    >
      <span className="flex size-8 shrink-0 items-center justify-center rounded-[9px] bg-hover">
        <HugeiconsIcon
          icon={Folder01Icon}
          size={15}
          strokeWidth={1.5}
          className={c.registered ? "text-violet" : "text-tertiary"}
        />
      </span>

      <span className="min-w-0 flex-1 space-y-[5px]">
        <span className="flex flex-wrap items-center gap-2">
          <span className="text-[13px] font-medium">{c.name}</span>
          {c.runners.map((r) => (
            <span
              key={r}
              className="flex shrink-0 items-center gap-1.5 rounded-full bg-hover px-2 py-0.5 text-[10px] font-medium text-muted-foreground"
            >
              <span
                className={cn("size-1 rounded-full", RUNNER_DOT[r] ?? "bg-tertiary")}
              />
              {r}
            </span>
          ))}
        </span>
        <span className="flex flex-wrap items-center gap-2.5">
          <span className="truncate font-mono text-[11px] text-tertiary">
            {tilde(c.path)}
          </span>
          {c.markers.map((m) => (
            <span
              key={m}
              className="shrink-0 rounded border border-border px-1.5 py-px font-mono text-[9px] text-tertiary"
            >
              {m}
            </span>
          ))}
        </span>
      </span>

      {c.registered && entries > 0 && (
        <span className="shrink-0 text-right">
          <span className="block font-mono text-[14px] font-semibold tabular-nums">
            {entries}
          </span>
          <span className="block text-[10px] text-tertiary">entries</span>
        </span>
      )}

      <button
        type="button"
        disabled={busy}
        onClick={() => onToggle(c)}
        className={cn(
          "flex shrink-0 items-center gap-1.5 rounded-lg px-3 py-1.5 text-[11px] font-medium transition-colors disabled:opacity-50",
          c.registered
            ? "text-muted-foreground hover:bg-hover hover:text-foreground"
            : "border border-border-strong bg-hover text-foreground hover:border-violet",
        )}
      >
        <HugeiconsIcon
          icon={c.registered ? Cancel01Icon : PlusSignIcon}
          size={12}
          strokeWidth={2}
        />
        {c.registered ? "Remove" : "Add"}
      </button>
    </div>
  );
}

export function ProjectsView() {
  const [items, setItems] = useState<Candidate[] | null>(null);
  const [scannedMs, setScannedMs] = useState(0);
  const [busy, setBusy] = useState<string | null>(null);
  const [scope, setScope] = useState<Scope>("All");
  const [query, setQuery] = useState("");
  const { data: library } = useLibrary();

  const scan = useCallback(async (fresh = false) => {
    try {
      const d = await discoverProjects(fresh);
      setItems(d.candidates);
      setScannedMs(d.scannedMs);
    } catch (e) {
      notify("Discovery failed", {
        description: e instanceof Error ? e.message : String(e),
      });
    }
  }, []);

  useEffect(() => {
    void scan();
  }, [scan]);

  /** Entries the library actually picked up per tracked project. */
  const entryCounts = useMemo(() => {
    const m = new Map<string, number>();
    for (const e of library?.entries ?? []) {
      if (e.project) m.set(e.project, (m.get(e.project) ?? 0) + 1);
    }
    return m;
  }, [library]);

  const toggle = async (c: Candidate) => {
    setBusy(c.path);
    try {
      if (c.registered) {
        await removeProject(c.path);
        notify(`Removed ${c.name}`, {
          description: "Its entries have left the library.",
        });
      } else {
        await addProject(c.name, c.path);
        notify(`Added ${c.name}`, {
          description: "Its skills and instructions are now in the library.",
        });
      }
      await scan();
    } catch (e) {
      notify("Could not update", {
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(null);
    }
  };

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (items ?? []).filter((c) => {
      if (scope === "Tracked" && !c.registered) return false;
      if (scope === "Discovered" && c.registered) return false;
      if (!q) return true;
      return (
        c.name.toLowerCase().includes(q) || c.path.toLowerCase().includes(q)
      );
    });
  }, [items, scope, query]);

  const tracked = filtered.filter((c) => c.registered);
  const found = filtered.filter((c) => !c.registered);

  return (
    <div className="flex h-full min-h-0 flex-col gap-[18px] overflow-y-auto p-[26px]">
      <PageHeader
        title="Projects"
        subtitle={
          items
            ? `${items.filter((c) => c.registered).length} tracked · ${items.filter((c) => !c.registered).length} discovered nearby · scanned in ${scannedMs}ms`
            : "Scanning for projects with agent config…"
        }
        action={
          <button
            type="button"
            onClick={() => void scan(true)}
            className="flex shrink-0 items-center gap-1.5 rounded-full border border-border bg-elevated px-3.5 py-1.5 text-xs font-medium transition-colors hover:border-border-strong"
          >
            <HugeiconsIcon icon={RefreshIcon} size={13} strokeWidth={1.8} />
            Rescan
          </button>
        }
      />

      <div className="flex items-center gap-2">
        <Segmented
          options={SCOPES}
          value={scope}
          onChange={setScope}
          layoutId="projects-scope"
        />
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Filter by name or path…"
          className="h-[34px] max-w-[280px] flex-1"
        />
      </div>

      {!items ? (
        <div className="space-y-1.5">
          {Array.from({ length: 5 }).map((_, i) => (
            <Skeleton key={i} className="h-[60px] w-full rounded-[10px]" />
          ))}
        </div>
      ) : (
        <div className="space-y-5">
          {tracked.length > 0 && (
            <div className="space-y-2">
              <SectionLabel>TRACKED</SectionLabel>
              <div className="space-y-1.5">
                {tracked.map((c) => (
                  <ProjectRow
                    key={c.path}
                    c={c}
                    entries={entryCounts.get(c.name) ?? 0}
                    onToggle={toggle}
                    busy={busy === c.path}
                  />
                ))}
              </div>
            </div>
          )}

          {found.length > 0 && (
            <div className="space-y-2">
              <SectionLabel>DISCOVERED</SectionLabel>
              <div className="space-y-1.5">
                {found.map((c) => (
                  <ProjectRow
                    key={c.path}
                    c={c}
                    entries={0}
                    onToggle={toggle}
                    busy={busy === c.path}
                  />
                ))}
              </div>
            </div>
          )}

          {filtered.length === 0 && (
            <p className="rounded-[10px] border border-dashed border-border px-4 py-8 text-center text-[13px] text-muted-foreground">
              {query ? "Nothing matches that filter." : "No projects found nearby."}
            </p>
          )}

          <p className="text-[11px] leading-relaxed text-tertiary">
            Scans your home folder and the usual code directories, three levels
            deep, for <code className="font-mono">.claude</code>,{" "}
            <code className="font-mono">.codex</code>,{" "}
            <code className="font-mono">CLAUDE.md</code>,{" "}
            <code className="font-mono">AGENTS.md</code> and similar. Nothing is
            indexed until you add it.
          </p>
        </div>
      )}
    </div>
  );
}
