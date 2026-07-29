import { useCallback, useRef } from "react";
import * as motionReact from "motion/react";
import type { ReasoningLevel } from "@/lib/api";
import { cn } from "@/lib/utils";

const { motion } = motionReact;

/**
 * Discrete effort slider.
 *
 * The levels come from the runner, so the number of stops varies — Codex
 * exposes six including `ultra`, Claude five. The track is therefore built
 * from the array rather than assuming a fixed count.
 */
export function EffortSlider({
  levels,
  value,
  onChange,
}: {
  levels: ReasoningLevel[];
  value: string;
  onChange: (effort: string) => void;
}) {
  const trackRef = useRef<HTMLDivElement>(null);
  const index = Math.max(
    0,
    levels.findIndex((l) => l.effort === value),
  );
  const last = levels.length - 1;
  const pct = last > 0 ? (index / last) * 100 : 0;

  /** Maps a pointer position to the nearest stop. */
  const pick = useCallback(
    (clientX: number) => {
      const el = trackRef.current;
      if (!el || last <= 0) return;
      const { left, width } = el.getBoundingClientRect();
      const ratio = Math.min(1, Math.max(0, (clientX - left) / width));
      const next = levels[Math.round(ratio * last)];
      if (next && next.effort !== value) onChange(next.effort);
    },
    [levels, last, value, onChange],
  );

  const current = levels[index];

  return (
    <div className="space-y-2.5">
      <div
        ref={trackRef}
        role="slider"
        tabIndex={0}
        aria-label="Reasoning effort"
        aria-valuemin={0}
        aria-valuemax={last}
        aria-valuenow={index}
        aria-valuetext={current?.effort}
        onPointerDown={(e) => {
          e.currentTarget.setPointerCapture(e.pointerId);
          pick(e.clientX);
        }}
        onPointerMove={(e) => {
          if (e.buttons === 1) pick(e.clientX);
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowLeft" && index > 0) {
            e.preventDefault();
            onChange(levels[index - 1].effort);
          }
          if (e.key === "ArrowRight" && index < last) {
            e.preventDefault();
            onChange(levels[index + 1].effort);
          }
        }}
        className="relative h-9 cursor-pointer touch-none select-none rounded-full bg-hover outline-none ring-offset-2 focus-visible:ring-2 focus-visible:ring-ring"
      >
        {/* Filled portion */}
        <motion.div
          className="absolute inset-y-0 left-0 rounded-full bg-violet"
          animate={{ width: `calc(${pct}% + 18px)` }}
          transition={{ type: "spring", stiffness: 520, damping: 38 }}
        />

        {/* One dot per stop, sitting under the knob */}
        <div className="absolute inset-0 flex items-center justify-between px-[15px]">
          {levels.map((l, i) => (
            <span
              key={l.effort}
              className={cn(
                "size-1 rounded-full transition-colors",
                i <= index ? "bg-white/45" : "bg-tertiary/50",
              )}
            />
          ))}
        </div>

        {/* Knob */}
        <motion.div
          className="absolute top-1/2 size-[30px] rounded-full bg-white shadow-[0_1px_4px_rgba(0,0,0,0.3)]"
          animate={{ left: `calc(${pct}% - ${pct * 0.3}px + 2px)` }}
          style={{ translateY: "-50%" }}
          transition={{ type: "spring", stiffness: 520, damping: 38 }}
        />
      </div>

      <div className="flex items-baseline gap-2 px-0.5">
        <span className="text-[12px] font-medium capitalize">
          {current?.effort ?? "—"}
        </span>
        {current?.description && (
          <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">
            {current.description}
          </span>
        )}
      </div>
    </div>
  );
}
