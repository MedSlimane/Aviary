import * as motionReact from "motion/react";
import { cn } from "@/lib/utils";
import { listContainer, listItem, pressable } from "@/lib/motion";

const { motion } = motionReact;

export function PageHeader({
  title,
  subtitle,
  action,
}: {
  title: string;
  subtitle: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex items-center gap-4">
      <div className="flex-1 space-y-[3px]">
        <h1 className="text-[22px] font-semibold tracking-tight">{title}</h1>
        <p className="text-xs text-muted-foreground">{subtitle}</p>
      </div>
      {action}
    </div>
  );
}

export function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <p className="text-[10px] font-semibold tracking-[0.08em] text-tertiary">
      {children}
    </p>
  );
}

export function RunnerChip({
  runner,
}: {
  runner: "Claude Code" | "Codex";
}) {
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

export function StatusDot({
  status,
}: {
  status: "ok" | "warn" | "error";
}) {
  return (
    <span className="relative flex size-[7px] shrink-0">
      {status === "ok" && (
        <motion.span
          className="absolute inset-0 rounded-full bg-ok"
          animate={{ opacity: [0.5, 0, 0.5], scale: [1, 2.1, 1] }}
          transition={{ duration: 2.4, repeat: Infinity, ease: "easeInOut" }}
        />
      )}
      <span
        className={cn(
          "relative size-[7px] rounded-full",
          status === "ok" && "bg-ok",
          status === "warn" && "bg-warn",
          status === "error" && "bg-destructive",
        )}
      />
    </span>
  );
}

/** Segmented control with a shared layout-animated selection pill. */
export function Segmented<T extends string>({
  options,
  value,
  onChange,
  layoutId,
}: {
  options: readonly T[];
  value: T;
  onChange: (v: T) => void;
  layoutId: string;
}) {
  return (
    <div className="flex items-center gap-0.5 rounded-[10px] border border-border bg-elevated p-[3px]">
      {options.map((opt) => {
        const active = opt === value;
        return (
          <button
            key={opt}
            type="button"
            onClick={() => onChange(opt)}
            className={cn(
              "relative rounded-[7px] px-3 py-[5px] text-xs font-medium transition-colors",
              active ? "text-foreground" : "text-muted-foreground hover:text-foreground",
            )}
          >
            {active && (
              <motion.span
                layoutId={layoutId}
                className="av-selected-wash absolute inset-0 rounded-[7px] bg-selected ring-1 ring-inset ring-white/[0.07]"
                transition={{ type: "spring", stiffness: 520, damping: 38 }}
              />
            )}
            <span className="relative z-10">{opt}</span>
          </button>
        );
      })}
    </div>
  );
}

/** Staggered list wrapper — children animate in on mount. */
export function StaggerList({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <motion.div
      className={className}
      variants={listContainer}
      initial="hidden"
      animate="show"
    >
      {children}
    </motion.div>
  );
}

export function StaggerRow({
  children,
  className,
  onClick,
  interactive = true,
}: {
  children: React.ReactNode;
  className?: string;
  onClick?: () => void;
  interactive?: boolean;
}) {
  return (
    <motion.div
      variants={listItem}
      onClick={onClick}
      {...(interactive ? pressable : {})}
      className={className}
    >
      {children}
    </motion.div>
  );
}
