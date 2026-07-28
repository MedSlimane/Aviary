import { HugeiconsIcon } from "@hugeicons/react";
import {
  SparklesIcon,
  BotIcon,
  TextAlignLeftIcon,
  CommandIcon,
} from "@hugeicons/core-free-icons";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

type Kind = "skill" | "agent" | "prompt" | "command";

const KIND_META: Record<Kind, { icon: typeof SparklesIcon; color: string }> = {
  skill: { icon: SparklesIcon, color: "text-violet" },
  agent: { icon: BotIcon, color: "text-teal" },
  prompt: { icon: TextAlignLeftIcon, color: "text-peach" },
  command: { icon: CommandIcon, color: "text-violet" },
};

const BUNDLES = [
  { title: "Frontend Review", meta: "2 skills · 2 agents · figma, playwright", art: "aurora" },
  { title: "Deep Research", meta: "1 prompt · 3 skills · web, memory", art: "tidal" },
  { title: "Design Exploration", meta: "4 skills · media collection · figma", art: "ember" },
  { title: "Repo Triage", meta: "2 agents · 1 skill · github, sqlite", art: "dusk" },
] as const;

const ART: Record<string, string> = {
  aurora: "linear-gradient(105deg, #2b2140, #5b4b9e 35%, #7c8fe0 70%, #bfd9f2)",
  tidal: "linear-gradient(95deg, #10312f, #2e6e66 35%, #7fc9c0 70%, #eaf7ef)",
  ember: "linear-gradient(115deg, #3a1d22, #8e4a48 35%, #d98a6b 70%, #fce3c8)",
  dusk: "linear-gradient(120deg, #160b2e, #43156b 35%, #9b2b84 70%, #f0a0b4)",
};

const ENTRIES: {
  name: string;
  desc: string;
  kind: Kind;
  runner: "Claude Code" | "Codex";
}[] = [
  { name: "design-taste-frontend", desc: "Visual language rules for dense product UI", kind: "skill", runner: "Claude Code" },
  { name: "systematic-debugging", desc: "Root-cause first — never patch the symptom", kind: "skill", runner: "Claude Code" },
  { name: "Explore", desc: "Read-only fan-out search across the repo", kind: "agent", runner: "Claude Code" },
  { name: "review-checklist", desc: "Correctness, reuse and altitude pass", kind: "prompt", runner: "Codex" },
  { name: "brandkit", desc: "Generate a brand system from a single seed", kind: "skill", runner: "Codex" },
  { name: "/verify", desc: "Drive the change end-to-end before commit", kind: "command", runner: "Claude Code" },
];

const FILTERS = ["All", "Prompts", "Skills", "Agents", "Commands"] as const;

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <p className="text-[10px] font-semibold tracking-[0.08em] text-tertiary">
      {children}
    </p>
  );
}

function RunnerChip({ runner }: { runner: "Claude Code" | "Codex" }) {
  return (
    <span className="flex shrink-0 items-center gap-1.5 rounded-full border border-border bg-elevated px-2.5 py-1 text-[11px] font-medium text-muted-foreground">
      <span
        className={cn(
          "size-1.5 rounded-full",
          runner === "Claude Code" ? "bg-claude" : "bg-codex",
        )}
      />
      {runner}
    </span>
  );
}

export function LibraryView() {
  return (
    <div className="flex flex-col gap-[22px] p-[26px]">
      {/* Header */}
      <div className="flex items-center gap-4">
        <div className="flex-1 space-y-[3px]">
          <h1 className="text-[22px] font-semibold tracking-tight">Library</h1>
          <p className="text-xs text-muted-foreground">
            1,284 entries across Claude Code and Codex
          </p>
        </div>
        <Button size="sm" className="rounded-full">
          New skill
        </Button>
      </div>

      {/* Filters */}
      <div className="flex items-center gap-1.5">
        <div className="flex items-center gap-0.5 rounded-[10px] border border-border bg-elevated p-[3px]">
          {FILTERS.map((f, i) => (
            <button
              key={f}
              type="button"
              className={cn(
                "rounded-[7px] px-3 py-[5px] text-xs font-medium transition-colors",
                i === 0
                  ? "av-selected-wash bg-selected text-foreground ring-1 ring-inset ring-white/[0.07]"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              {f}
            </button>
          ))}
        </div>
        <div className="flex-1" />
        <RunnerChip runner="Claude Code" />
        <RunnerChip runner="Codex" />
      </div>

      {/* Bundles */}
      <div className="space-y-3">
        <SectionLabel>CONTEXT BUNDLES</SectionLabel>
        <div className="grid grid-cols-4 gap-3.5">
          {BUNDLES.map((b) => (
            <article
              key={b.title}
              className="overflow-hidden rounded-[14px] border border-border bg-card transition-colors hover:border-border-strong"
            >
              <div
                className="h-[78px]"
                style={{ backgroundImage: ART[b.art] }}
              />
              <div className="space-y-[3px] p-3.5">
                <h3 className="truncate text-[13px] font-semibold">{b.title}</h3>
                <p className="truncate text-[11px] text-muted-foreground">
                  {b.meta}
                </p>
              </div>
            </article>
          ))}
        </div>
      </div>

      {/* Entries */}
      <div className="space-y-3">
        <SectionLabel>ALL ENTRIES</SectionLabel>
        <div className="space-y-1.5">
          {ENTRIES.map((e) => {
            const meta = KIND_META[e.kind];
            return (
              <button
                key={e.name}
                type="button"
                className="flex w-full items-center gap-3 rounded-[10px] border border-border bg-card px-3 py-2.5 text-left transition-colors hover:border-border-strong hover:bg-elevated"
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
                  <span className="block truncate text-[13px] font-medium">
                    {e.name}
                  </span>
                  <span className="block truncate text-xs text-muted-foreground">
                    {e.desc}
                  </span>
                </span>
                <span className="shrink-0 rounded-full border border-border bg-elevated px-2.5 py-1 text-[11px] font-medium text-muted-foreground">
                  {e.kind}
                </span>
                <RunnerChip runner={e.runner} />
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}
