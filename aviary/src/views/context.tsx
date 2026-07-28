import { useMemo, useState } from "react";
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
import {
  PageHeader,
  SectionLabel,
  StaggerList,
  StaggerRow,
} from "@/components/screen-parts";

const { motion } = motionReact;

type Layer = {
  scope: string;
  path: string;
  tokens: number;
  color: string;
};

const LAYERS_BY_RUNNER: Record<string, Layer[]> = {
  "Claude Code": [
    { scope: "system", path: "Claude Code system prompt · built-in", tokens: 2400, color: "var(--av-text-tertiary)" },
    { scope: "user", path: "~/.claude/CLAUDE.md", tokens: 820, color: "var(--av-accent-blue)" },
    { scope: "project", path: "~/work/dashboard/CLAUDE.md", tokens: 1240, color: "var(--av-accent-violet)" },
    { scope: "local", path: "~/work/dashboard/.claude/CLAUDE.local.md", tokens: 310, color: "var(--av-accent-teal)" },
    { scope: "skills", path: "3 active skills · design-taste, debugging, verify", tokens: 4180, color: "var(--av-accent-peach)" },
    { scope: "mcp", path: "MCP tool definitions · figma, playwright", tokens: 6900, color: "var(--av-accent-coral)" },
    { scope: "memory", path: "12 memory files · ~/.claude/projects/…/memory", tokens: 1050, color: "var(--av-accent-gold)" },
  ],
  Codex: [
    { scope: "system", path: "Codex system prompt · built-in", tokens: 1900, color: "var(--av-text-tertiary)" },
    { scope: "user", path: "~/.codex/AGENTS.md", tokens: 640, color: "var(--av-accent-blue)" },
    { scope: "project", path: "~/work/dashboard/AGENTS.md", tokens: 980, color: "var(--av-accent-violet)" },
    { scope: "skills", path: "2 active skills · imagegen, brandkit", tokens: 2350, color: "var(--av-accent-peach)" },
    { scope: "mcp", path: "MCP tool definitions · figma, sqlite", tokens: 3100, color: "var(--av-accent-coral)" },
  ],
};

const DIRECTORIES = ["~/work/dashboard", "~/personalAi", "~/work/api"];

export function ContextView() {
  const [runner, setRunner] = useState("Claude Code");
  const [dir, setDir] = useState(DIRECTORIES[0]);

  const layers = LAYERS_BY_RUNNER[runner];
  const total = useMemo(
    () => layers.reduce((sum, l) => sum + l.tokens, 0),
    [layers],
  );
  const max = useMemo(
    () => Math.max(...layers.map((l) => l.tokens)),
    [layers],
  );
  const pct = ((total / 200_000) * 100).toFixed(1);

  return (
    <div className="flex flex-col gap-[18px] p-[26px]">
      <PageHeader
        title="Context"
        subtitle="Exactly what gets loaded, in order, before your first message"
        action={
          <div className="flex items-center gap-2">
            <Selector
              label="Runner"
              value={runner}
              options={Object.keys(LAYERS_BY_RUNNER)}
              onChange={setRunner}
            />
            <Selector
              label="Directory"
              value={dir}
              options={DIRECTORIES}
              onChange={setDir}
            />
          </div>
        }
      />

      {/* Budget card */}
      <div className="space-y-4 rounded-[14px] border border-border bg-card p-5">
        <div className="flex items-center gap-4">
          <div className="flex-1 space-y-1">
            <motion.p
              key={total}
              initial={{ opacity: 0, y: -4 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.2 }}
              className="text-[15px] font-semibold"
            >
              {total.toLocaleString()} tokens loaded before you type anything
            </motion.p>
            <p className="text-xs text-muted-foreground">
              {pct}% of the 200K context window · {layers.length} layers
            </p>
          </div>
          <motion.span
            key={pct}
            initial={{ opacity: 0, scale: 0.94 }}
            animate={{ opacity: 1, scale: 1 }}
            transition={{ type: "spring", stiffness: 420, damping: 30 }}
            className="font-mono text-[26px] font-semibold tabular-nums"
          >
            {pct}%
          </motion.span>
        </div>

        {/* Stacked meter */}
        <div className="flex h-2.5 gap-0.5 overflow-hidden">
          {layers.map((l) => (
            <motion.div
              key={l.scope}
              layout
              initial={{ flexGrow: 0, opacity: 0 }}
              animate={{ flexGrow: l.tokens, opacity: 1 }}
              transition={{ type: "spring", stiffness: 260, damping: 34 }}
              className="rounded-[3px]"
              style={{ backgroundColor: l.color, flexBasis: 0 }}
            />
          ))}
        </div>

        <div className="flex flex-wrap gap-4">
          {layers.map((l) => (
            <span key={l.scope} className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
              <span
                className="size-1.5 rounded-full"
                style={{ backgroundColor: l.color }}
              />
              {l.scope}
            </span>
          ))}
        </div>
      </div>

      <SectionLabel>RESOLUTION ORDER</SectionLabel>

      <StaggerList className="space-y-1.5" key={runner}>
        {layers.map((l, i) => (
          <StaggerRow
            key={l.scope}
            interactive={false}
            className="flex items-center gap-3.5 rounded-[10px] border border-border bg-card px-3 py-2.5 transition-colors hover:border-border-strong"
          >
            <span className="flex size-[22px] shrink-0 items-center justify-center rounded-md bg-hover font-mono text-[10px] text-muted-foreground">
              {i + 1}
            </span>
            <span className="flex w-[78px] shrink-0 items-center gap-2">
              <span
                className="size-[7px] rounded-full"
                style={{ backgroundColor: l.color }}
              />
              <span className="text-[11px] font-medium text-muted-foreground">
                {l.scope}
              </span>
            </span>
            <span className="min-w-0 flex-1 truncate font-mono text-xs">
              {l.path}
            </span>
            <span className="h-1.5 w-[150px] shrink-0 overflow-hidden rounded-full bg-hover">
              <motion.span
                className="block h-full rounded-full"
                style={{ backgroundColor: l.color }}
                initial={{ width: 0 }}
                animate={{ width: `${(l.tokens / max) * 100}%` }}
                transition={{ duration: 0.5, delay: 0.05 * i, ease: [0.22, 1, 0.36, 1] }}
              />
            </span>
            <span className="w-14 shrink-0 text-right font-mono text-xs tabular-nums text-muted-foreground">
              {l.tokens.toLocaleString()}
            </span>
          </StaggerRow>
        ))}
      </StaggerList>
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
        <span className="text-xs font-medium">{value}</span>
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
