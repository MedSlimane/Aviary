import * as motionReact from "motion/react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  SparklesIcon,
  ServerStack01Icon,
  Brain02Icon,
  BubbleChatIcon,
} from "@hugeicons/core-free-icons";
import type { RouteId } from "@/components/app-rail";
import {
  SectionLabel,
  StaggerList,
  StaggerRow,
  StatusDot,
} from "@/components/screen-parts";

const { motion } = motionReact;

const ART: Record<string, string> = {
  aurora: "linear-gradient(105deg, #2b2140, #5b4b9e 35%, #7c8fe0 70%, #bfd9f2)",
  tidal: "linear-gradient(95deg, #10312f, #2e6e66 35%, #7fc9c0 70%, #eaf7ef)",
  ember: "linear-gradient(115deg, #3a1d22, #8e4a48 35%, #d98a6b 70%, #fce3c8)",
  dusk: "linear-gradient(120deg, #160b2e, #43156b 35%, #9b2b84 70%, #f0a0b4)",
};

const BUNDLES = [
  { title: "Frontend Review", meta: "2 skills · 2 agents · figma", art: "aurora" },
  { title: "Deep Research", meta: "1 prompt · 3 skills · web", art: "tidal" },
  { title: "Design Exploration", meta: "4 skills · figma", art: "ember" },
  { title: "Repo Triage", meta: "2 agents · github", art: "dusk" },
] as const;

const STATS = [
  { label: "Skills", value: "48", icon: SparklesIcon, route: "library" as RouteId },
  { label: "MCP servers", value: "6", icon: ServerStack01Icon, route: "mcp" as RouteId },
  { label: "Context loaded", value: "16.9K", icon: Brain02Icon, route: "context" as RouteId },
  { label: "Sessions today", value: "12", icon: BubbleChatIcon, route: "chat" as RouteId },
];

export function HomeView({ onNavigate }: { onNavigate: (r: RouteId) => void }) {
  return (
    <div className="flex flex-col gap-[22px] p-[26px]">
      <div className="space-y-1">
        <motion.h1
          initial={{ opacity: 0, y: 6 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.24 }}
          className="text-[26px] font-semibold tracking-tight"
        >
          Good evening
        </motion.h1>
        <p className="text-xs text-muted-foreground">
          Claude Code and Codex are both healthy · last indexed 4 minutes ago
        </p>
      </div>

      {/* Stat tiles */}
      <StaggerList className="grid grid-cols-4 gap-3.5">
        {STATS.map((s) => (
          <StaggerRow
            key={s.label}
            onClick={() => onNavigate(s.route)}
            className="cursor-pointer rounded-[14px] border border-border bg-card p-4 transition-colors hover:border-border-strong hover:bg-elevated"
          >
            <HugeiconsIcon
              icon={s.icon}
              size={16}
              strokeWidth={1.5}
              className="text-tertiary"
            />
            <p className="mt-3 font-mono text-[24px] font-semibold tabular-nums">
              {s.value}
            </p>
            <p className="mt-0.5 text-[11px] text-muted-foreground">{s.label}</p>
          </StaggerRow>
        ))}
      </StaggerList>

      <div className="space-y-3">
        <SectionLabel>QUICK LAUNCH</SectionLabel>
        <StaggerList className="grid grid-cols-4 gap-3.5">
          {BUNDLES.map((b) => (
            <StaggerRow
              key={b.title}
              onClick={() => onNavigate("chat")}
              className="cursor-pointer overflow-hidden rounded-[14px] border border-border bg-card transition-colors hover:border-border-strong"
            >
              <div className="h-[78px]" style={{ backgroundImage: ART[b.art] }} />
              <div className="space-y-[3px] p-3.5">
                <h3 className="truncate text-[13px] font-semibold">{b.title}</h3>
                <p className="truncate text-[11px] text-muted-foreground">
                  {b.meta}
                </p>
              </div>
            </StaggerRow>
          ))}
        </StaggerList>
      </div>

      <div className="space-y-3">
        <SectionLabel>HEALTH</SectionLabel>
        <StaggerList className="space-y-1.5">
          {[
            { status: "warn" as const, text: "notion — auth expired", meta: "MCP" },
            { status: "ok" as const, text: "All 48 skills parsed cleanly", meta: "Library" },
            { status: "ok" as const, text: "Index up to date · 1,284 entries", meta: "Indexer" },
          ].map((h) => (
            <StaggerRow
              key={h.text}
              interactive={false}
              className="flex items-center gap-3 rounded-[10px] border border-border bg-card px-3.5 py-2.5"
            >
              <StatusDot status={h.status} />
              <span className="min-w-0 flex-1 truncate text-[13px]">{h.text}</span>
              <span className="text-[11px] text-tertiary">{h.meta}</span>
            </StaggerRow>
          ))}
        </StaggerList>
      </div>
    </div>
  );
}
