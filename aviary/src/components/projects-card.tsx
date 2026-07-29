import { useCallback, useEffect, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  RefreshIcon,
  PlusSignIcon,
  Cancel01Icon,
  Folder01Icon,
} from "@hugeicons/core-free-icons";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { SectionLabel } from "@/components/screen-parts";
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

function tilde(p: string) {
  return p.replace(/^\/Users\/[^/]+/, "~");
}

function Row({
  c,
  onToggle,
  busy,
}: {
  c: Candidate;
  onToggle: (c: Candidate) => void;
  busy: boolean;
}) {
  return (
    <div className="av-hover-grad flex items-center gap-3 rounded-[10px] border border-border bg-card px-3 py-2.5 transition-colors hover:border-border-strong">
      <span className="flex size-[28px] shrink-0 items-center justify-center rounded-lg bg-hover">
        <HugeiconsIcon
          icon={Folder01Icon}
          size={14}
          strokeWidth={1.5}
          className="text-tertiary"
        />
      </span>

      <span className="min-w-0 flex-1 space-y-0.5">
        <span className="flex items-center gap-2">
          <span className="truncate text-[13px] font-medium">{c.name}</span>
          {c.runners.map((r) => (
            <span
              key={r}
              className="flex shrink-0 items-center gap-1 rounded-full border border-border bg-elevated px-1.5 py-px text-[10px] text-muted-foreground"
            >
              <span
                className={cn(
                  "size-1 rounded-full",
                  RUNNER_DOT[r] ?? "bg-tertiary",
                )}
              />
              {r}
            </span>
          ))}
        </span>
        <span className="block truncate font-mono text-[11px] text-tertiary">
          {tilde(c.path)}
        </span>
      </span>

      <Button
        size="sm"
        variant={c.registered ? "ghost" : "outline"}
        disabled={busy}
        onClick={() => onToggle(c)}
        className="shrink-0"
      >
        <HugeiconsIcon
          icon={c.registered ? Cancel01Icon : PlusSignIcon}
          size={13}
          strokeWidth={2}
        />
        {c.registered ? "Remove" : "Add"}
      </Button>
    </div>
  );
}

export function ProjectsCard() {
  const [items, setItems] = useState<Candidate[] | null>(null);
  const [scannedMs, setScannedMs] = useState(0);
  const [busy, setBusy] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);

  const scan = useCallback(async () => {
    try {
      const d = await discoverProjects();
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

  const toggle = async (c: Candidate) => {
    setBusy(c.path);
    try {
      if (c.registered) {
        await removeProject(c.path);
        notify(`Removed ${c.name}`, { description: "No longer indexed." });
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

  const registered = items?.filter((c) => c.registered) ?? [];
  const found = items?.filter((c) => !c.registered) ?? [];
  const visible = showAll ? found : found.slice(0, 6);

  return (
    <section className="space-y-3.5 rounded-[14px] border border-border bg-card p-5">
      <div className="flex items-center gap-3">
        <div className="flex-1">
          <SectionLabel>PROJECTS</SectionLabel>
          <p className="mt-1 text-[11px] text-muted-foreground">
            {items
              ? `${registered.length} tracked · ${found.length} found nearby · ${scannedMs}ms`
              : "Looking for projects with agent config…"}
          </p>
        </div>
        <Button size="sm" variant="outline" onClick={() => void scan()}>
          <HugeiconsIcon icon={RefreshIcon} size={13} strokeWidth={1.8} />
          Rescan
        </Button>
      </div>

      {!items ? (
        <div className="space-y-1.5">
          {Array.from({ length: 3 }).map((_, i) => (
            <Skeleton key={i} className="h-[52px] w-full rounded-[10px]" />
          ))}
        </div>
      ) : (
        <div className="space-y-4">
          {registered.length > 0 && (
            <div className="space-y-1.5">
              {registered.map((c) => (
                <Row
                  key={c.path}
                  c={c}
                  onToggle={toggle}
                  busy={busy === c.path}
                />
              ))}
            </div>
          )}

          {found.length > 0 && (
            <div className="space-y-2">
              <p className="text-[10px] font-semibold tracking-[0.08em] text-tertiary">
                DISCOVERED
              </p>
              <div className="space-y-1.5">
                {visible.map((c) => (
                  <Row
                    key={c.path}
                    c={c}
                    onToggle={toggle}
                    busy={busy === c.path}
                  />
                ))}
              </div>
              {found.length > 6 && (
                <button
                  type="button"
                  onClick={() => setShowAll((v) => !v)}
                  className="text-[11px] text-muted-foreground transition-colors hover:text-foreground"
                >
                  {showAll
                    ? "Show fewer"
                    : `Show ${found.length - 6} more`}
                </button>
              )}
            </div>
          )}

          {items.length === 0 && (
            <p className="rounded-[10px] border border-dashed border-border px-4 py-6 text-center text-[13px] text-muted-foreground">
              No projects with agent config found nearby.
            </p>
          )}
        </div>
      )}

      <p className="text-[11px] text-tertiary">
        Scans a few likely folders for <code className="font-mono">.claude</code>,{" "}
        <code className="font-mono">.codex</code>,{" "}
        <code className="font-mono">CLAUDE.md</code>,{" "}
        <code className="font-mono">AGENTS.md</code> and similar. Nothing is
        indexed until you add it.
      </p>
    </section>
  );
}
