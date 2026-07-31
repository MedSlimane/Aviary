import { useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  SparklesIcon,
  ServerStack01Icon,
  FolderLibraryIcon,
  Image01Icon,
} from "@hugeicons/core-free-icons";
import type { RouteId } from "@/components/app-rail";
import {
  SectionLabel,
  StaggerList,
  StaggerRow,
  StatusDot,
} from "@/components/screen-parts";
import { Skeleton } from "@/components/ui/skeleton";
import { notify } from "@/lib/notify";
import {
  listMedia,
  scanLibrary,
  scanMcp,
  type Entry,
  type LibrarySnapshot,
  type McpSnapshot,
} from "@/lib/api";

function greeting(d = new Date()) {
  const h = d.getHours();
  if (h < 5) return "Still up";
  if (h < 12) return "Good morning";
  if (h < 18) return "Good afternoon";
  return "Good evening";
}

function ago(unixSeconds: number) {
  const mins = Math.max(0, Math.round((Date.now() / 1000 - unixSeconds) / 60));
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

export function HomeView({ onNavigate }: { onNavigate: (r: RouteId) => void }) {
  const [library, setLibrary] = useState<LibrarySnapshot | null>(null);
  const [mcp, setMcp] = useState<McpSnapshot | null>(null);
  const [mediaCount, setMediaCount] = useState<number | null>(null);

  useEffect(() => {
    // Cached scans, so this paints immediately on a cold launch.
    Promise.all([scanLibrary(), scanMcp(), listMedia()])
      .then(([lib, servers, media]) => {
        setLibrary(lib);
        setMcp(servers);
        setMediaCount(media.length);
      })
      .catch((e) => notify("Could not load", { description: String(e) }));
  }, []);

  const stats = useMemo(() => {
    const entries = library?.entries ?? [];
    const skills = entries.filter((e) => e.kind === "skill").length;
    const enabled = (mcp?.servers ?? []).filter((s) => s.enabled).length;
    return [
      { label: "Skills", value: skills, icon: SparklesIcon, route: "library" as RouteId },
      { label: "MCP servers", value: enabled, icon: ServerStack01Icon, route: "mcp" as RouteId },
      { label: "Projects", value: library?.projects.length ?? 0, icon: FolderLibraryIcon, route: "projects" as RouteId },
      { label: "References", value: mediaCount ?? 0, icon: Image01Icon, route: "inspiration" as RouteId },
    ];
  }, [library, mcp, mediaCount]);

  // Most recently edited entries — the closest thing to "where you left off"
  // that the index can actually answer today.
  const recent = useMemo(
    () =>
      [...(library?.entries ?? [])]
        .sort((a, b) => b.modified - a.modified)
        .slice(0, 5),
    [library],
  );

  const health = useMemo(() => {
    const rows: { status: "ok" | "warn" | "error"; text: string; meta: string }[] = [];

    for (const r of library?.runners ?? []) {
      rows.push({
        status: r.detected ? "ok" : "warn",
        text: r.detected ? `${r.label} detected` : `${r.label} not found on this machine`,
        meta: "Runner",
      });
    }

    const disabled = (mcp?.servers ?? []).filter((s) => !s.enabled);
    if (disabled.length > 0) {
      rows.push({
        status: "warn",
        text: `${disabled.length} MCP server${disabled.length === 1 ? "" : "s"} disabled`,
        meta: "MCP",
      });
    }

    if (library) {
      rows.push({
        status: "ok",
        text: `${library.entries.length} entries indexed in ${library.scannedMs}ms`,
        meta: "Library",
      });
    }
    return rows;
  }, [library, mcp]);

  const loading = library === null;

  return (
    <div className="flex flex-col gap-[22px] p-[26px]">
      <div className="space-y-1">
        <h1 className="text-[26px] font-semibold tracking-tight">{greeting()}</h1>
        <p className="text-xs text-muted-foreground">
          {loading
            ? "Loading your library…"
            : `${library.entries.length} entries across ${
                library.runners.filter((r) => r.detected).length
              } runner${library.runners.filter((r) => r.detected).length === 1 ? "" : "s"}`}
        </p>
      </div>

      {/* Stat tiles */}
      <StaggerList className="grid grid-cols-4 gap-3.5">
        {stats.map((s) => (
          <StaggerRow
            key={s.label}
            onClick={() => onNavigate(s.route)}
            className="av-hover-grad cursor-pointer rounded-[14px] border border-border bg-card p-4 transition-colors hover:border-border-strong"
          >
            <HugeiconsIcon
              icon={s.icon}
              size={16}
              strokeWidth={1.5}
              className="text-tertiary"
            />
            <p className="mt-3 font-mono text-[24px] font-semibold tabular-nums">
              {loading ? "—" : s.value}
            </p>
            <p className="mt-0.5 text-[11px] text-muted-foreground">{s.label}</p>
          </StaggerRow>
        ))}
      </StaggerList>

      <div className="space-y-3">
        <SectionLabel>RECENTLY EDITED</SectionLabel>
        {loading ? (
          <div className="space-y-1.5">
            <Skeleton className="h-[46px] rounded-[10px]" />
            <Skeleton className="h-[46px] rounded-[10px]" />
          </div>
        ) : recent.length === 0 ? (
          <p className="rounded-[10px] border border-dashed border-border px-3.5 py-3 text-xs text-muted-foreground">
            Nothing indexed yet. Register a project or install a skill to get started.
          </p>
        ) : (
          <StaggerList className="space-y-1.5">
            {recent.map((e) => (
              <RecentRow key={e.id} entry={e} onOpen={() => onNavigate("library")} />
            ))}
          </StaggerList>
        )}
      </div>

      <div className="space-y-3">
        <SectionLabel>HEALTH</SectionLabel>
        {loading ? (
          <Skeleton className="h-[46px] rounded-[10px]" />
        ) : (
          <StaggerList className="space-y-1.5">
            {health.map((h) => (
              <StaggerRow
                key={h.text}
                interactive={false}
                className="av-hover-grad flex items-center gap-3 rounded-[10px] border border-border bg-card px-3.5 py-2.5"
              >
                <StatusDot status={h.status} />
                <span className="min-w-0 flex-1 truncate text-[13px]">{h.text}</span>
                <span className="text-[11px] text-tertiary">{h.meta}</span>
              </StaggerRow>
            ))}
          </StaggerList>
        )}
      </div>
    </div>
  );
}

function RecentRow({ entry, onOpen }: { entry: Entry; onOpen: () => void }) {
  return (
    <StaggerRow
      onClick={onOpen}
      className="av-hover-grad flex cursor-pointer items-center gap-3 rounded-[10px] border border-border bg-card px-3.5 py-2.5 transition-colors hover:border-border-strong"
    >
      <span className="w-[62px] shrink-0 font-mono text-[10px] uppercase text-tertiary">
        {entry.kind}
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13px] font-medium">{entry.name}</span>
        {entry.description && (
          <span className="block truncate text-[11px] text-muted-foreground">
            {entry.description}
          </span>
        )}
      </span>
      <span className="shrink-0 text-[11px] text-tertiary">{ago(entry.modified)}</span>
    </StaggerRow>
  );
}
