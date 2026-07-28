import { useEffect, useRef, useState } from "react";
import * as motionReact from "motion/react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  PlusSignIcon,
  Layers01Icon,
  SparklesIcon,
  Mic01Icon,
  ArrowUp01Icon,
  ArrowDown01Icon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";

const { motion, AnimatePresence } = motionReact;

type Phase = "empty" | "thinking" | "answered" | "recording";

const PILLS = [
  { label: "Thinking…", from: "#fb7cc7", to: "#a78bfa" },
  { label: "Understanding your inbox…", from: "#fcca6b", to: "#fc8c73" },
  { label: "Performing actions…", from: "#fc9e73", to: "#fa6b9e" },
];

const SUGGESTIONS = [
  "Review my inbox",
  "Summarise yesterday's PRs",
  "Audit my MCP servers",
];

export function ChatView() {
  const [phase, setPhase] = useState<Phase>("empty");
  const [value, setValue] = useState("");
  const [visiblePills, setVisiblePills] = useState(0);
  const [elapsed, setElapsed] = useState(0);
  const timers = useRef<number[]>([]);

  useEffect(
    () => () => timers.current.forEach((t) => window.clearTimeout(t)),
    [],
  );

  // Recording timer
  useEffect(() => {
    if (phase !== "recording") return;
    setElapsed(0);
    const id = window.setInterval(() => setElapsed((e) => e + 1), 1000);
    return () => window.clearInterval(id);
  }, [phase]);

  const send = () => {
    if (!value.trim()) return;
    setPhase("thinking");
    setVisiblePills(0);
    timers.current.forEach((t) => window.clearTimeout(t));
    timers.current = [
      window.setTimeout(() => setVisiblePills(1), 250),
      window.setTimeout(() => setVisiblePills(2), 1100),
      window.setTimeout(() => setVisiblePills(3), 2000),
      window.setTimeout(() => setPhase("answered"), 3000),
    ];
  };

  const reset = () => {
    timers.current.forEach((t) => window.clearTimeout(t));
    setPhase("empty");
    setValue("");
    setVisiblePills(0);
  };

  const centered = phase === "empty" || phase === "recording";

  return (
    <div className="relative flex h-full flex-col overflow-hidden">
      {/* Ambient gradient glows */}
      <div
        aria-hidden
        className="pointer-events-none absolute -left-40 -top-52 size-[680px] rounded-full opacity-40 blur-[150px]"
        style={{ background: "radial-gradient(circle, #43156b, transparent 70%)" }}
      />
      <div
        aria-hidden
        className="pointer-events-none absolute -bottom-40 right-0 size-[560px] rounded-full opacity-25 blur-[160px]"
        style={{ background: "radial-gradient(circle, #2e6e66, transparent 70%)" }}
      />

      <div
        className={cn(
          "relative flex min-h-0 flex-1 flex-col px-8",
          centered ? "justify-center" : "justify-end",
        )}
      >
        {/* Conversation */}
        <AnimatePresence mode="popLayout" initial={false}>
          {phase !== "empty" && phase !== "recording" && (
            <motion.div
              key="stream"
              layout
              initial={{ opacity: 0, y: 12 }}
              animate={{ opacity: 1, y: 0 }}
              className="mx-auto w-full max-w-[760px] flex-1 space-y-6 overflow-auto py-8"
            >
              <div className="flex justify-end">
                <div className="av-glass max-w-[420px] rounded-[20px] px-[18px] py-3 text-sm">
                  {value}
                </div>
              </div>

              <div className="space-y-2.5">
                <AnimatePresence initial={false}>
                  {PILLS.slice(0, visiblePills).map((p) => (
                    <motion.div
                      key={p.label}
                      initial={{ opacity: 0, x: -12, filter: "blur(4px)" }}
                      animate={{ opacity: 1, x: 0, filter: "blur(0px)" }}
                      exit={{ opacity: 0 }}
                      transition={{ type: "spring", stiffness: 380, damping: 30 }}
                      className="av-glass flex w-fit items-center gap-2.5 rounded-full py-2 pl-2.5 pr-5"
                    >
                      <motion.span
                        className="size-[15px] rounded-[5px]"
                        style={{
                          backgroundImage: `linear-gradient(135deg, ${p.from}, ${p.to})`,
                        }}
                        animate={{ scale: [1, 1.14, 1] }}
                        transition={{ duration: 1.6, repeat: Infinity, ease: "easeInOut" }}
                      />
                      <span className="text-sm font-medium">{p.label}</span>
                    </motion.div>
                  ))}
                </AnimatePresence>

                {phase === "answered" && (
                  <motion.div
                    initial={{ opacity: 0, y: 8 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ duration: 0.28 }}
                    className="max-w-[620px] space-y-2.5 pt-2"
                  >
                    <p className="text-sm leading-relaxed text-muted-foreground">
                      Three things need you today. The Vercel invoice failed to
                      charge and retries stop Friday. Maya is blocked on the auth
                      migration decision — she asked twice. And the design review
                      you moved twice is now double-booked against board prep.
                    </p>
                    <p className="text-[11px] text-tertiary">
                      Used 3 tools · gmail, calendar, memory
                    </p>
                  </motion.div>
                )}
              </div>
            </motion.div>
          )}
        </AnimatePresence>

        {/* Greeting for the empty + recording states */}
        <AnimatePresence initial={false}>
          {centered && (
            <motion.div
              key="greeting"
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -10 }}
              transition={{ duration: 0.26 }}
              className="mx-auto mb-7 flex w-full max-w-[720px] flex-col items-center gap-3"
            >
              <AviaryMark recording={phase === "recording"} />
              <h2 className="text-[30px] font-semibold tracking-tight">
                {phase === "recording" ? "Listening…" : "What should we work on?"}
              </h2>
              <p className="text-[13px] text-muted-foreground">
                {phase === "recording"
                  ? "Speak naturally — ⌘. to stop, esc to cancel"
                  : "Claude Code · ~/work/dashboard · Frontend Review bundle"}
              </p>
            </motion.div>
          )}
        </AnimatePresence>

        {/* Composer */}
        <motion.div
          layout
          transition={{ type: "spring", stiffness: 340, damping: 34 }}
          className="mx-auto mb-6 w-full max-w-[760px]"
        >
          <div className="av-glass rounded-[22px] p-[18px] pb-3 shadow-[0_16px_40px_-12px_rgba(0,0,0,0.35)]">
            {phase === "recording" ? (
              <RecordingBody elapsed={elapsed} onStop={() => setPhase("empty")} />
            ) : (
              <>
                <textarea
                  value={value}
                  onChange={(e) => setValue(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && !e.shiftKey) {
                      e.preventDefault();
                      send();
                    }
                  }}
                  rows={1}
                  placeholder="Ask anything, or start with a bundle…"
                  className="w-full resize-none bg-transparent text-[15px] outline-none placeholder:text-white/40"
                />
                <div className="mt-4 flex items-center gap-2">
                  <GlassButton icon={PlusSignIcon} label="Attach" round />
                  <GlassChip icon={Layers01Icon} label="Context" />
                  <GlassChip icon={SparklesIcon} label="Sonnet 5" dropdown />
                  <div className="flex-1" />
                  <motion.button
                    type="button"
                    whileTap={{ scale: 0.9 }}
                    onClick={() => setPhase("recording")}
                    aria-label="Voice input"
                    className="rounded-full p-1.5 text-white/70 transition-colors hover:text-white"
                  >
                    <HugeiconsIcon icon={Mic01Icon} size={18} strokeWidth={1.5} />
                  </motion.button>
                  <motion.button
                    type="button"
                    onClick={send}
                    disabled={!value.trim()}
                    whileHover={{ scale: value.trim() ? 1.06 : 1 }}
                    whileTap={{ scale: value.trim() ? 0.92 : 1 }}
                    transition={{ type: "spring", stiffness: 600, damping: 26 }}
                    aria-label="Send"
                    className="flex size-8 items-center justify-center rounded-full bg-white text-black disabled:opacity-35"
                  >
                    <HugeiconsIcon icon={ArrowUp01Icon} size={17} strokeWidth={2} />
                  </motion.button>
                </div>
              </>
            )}
          </div>

          {/* Suggestions */}
          <AnimatePresence initial={false}>
            {phase === "empty" && (
              <motion.div
                initial={{ opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: 8 }}
                transition={{ duration: 0.22, delay: 0.05 }}
                className="mt-5 flex justify-center gap-2.5"
              >
                {SUGGESTIONS.map((s) => (
                  <motion.button
                    key={s}
                    type="button"
                    whileHover={{ y: -2 }}
                    whileTap={{ scale: 0.97 }}
                    transition={{ type: "spring", stiffness: 520, damping: 28 }}
                    onClick={() => setValue(s)}
                    className="rounded-full border border-white/10 bg-white/[0.07] px-3.5 py-2 text-xs font-medium text-white/75 transition-colors hover:text-white"
                  >
                    {s}
                  </motion.button>
                ))}
              </motion.div>
            )}
          </AnimatePresence>

          {phase === "answered" && (
            <div className="mt-3 flex justify-center">
              <button
                type="button"
                onClick={reset}
                className="text-[11px] text-white/45 transition-colors hover:text-white/80"
              >
                New chat
              </button>
            </div>
          )}
        </motion.div>
      </div>
    </div>
  );
}

function AviaryMark({ recording }: { recording: boolean }) {
  const petal =
    "M64 59C58.9 54.3 57.4 46.5 60.2 34.3L64 14C67.8 29.2 70.2 43.2 68.7 49.5C67.8 53.7 66.1 57 64 59Z";
  return (
    <motion.svg
      viewBox="0 0 128 128"
      className="size-[42px] text-violet"
      animate={recording ? { scale: [1, 1.12, 1], opacity: [0.85, 1, 0.85] } : {}}
      transition={{ duration: 1.6, repeat: Infinity, ease: "easeInOut" }}
    >
      {[0, 60, 120, 180, 240, 300].map((a) => (
        <path key={a} d={petal} fill="currentColor" transform={`rotate(${a} 64 64)`} />
      ))}
      <circle cx="64" cy="64" r="7.2" fill="currentColor" />
    </motion.svg>
  );
}

function RecordingBody({
  elapsed,
  onStop,
}: {
  elapsed: number;
  onStop: () => void;
}) {
  const mmss = `${Math.floor(elapsed / 60)}:${String(elapsed % 60).padStart(2, "0")}`;
  return (
    <>
      <p className="text-[15px] text-white/90">
        Draft a migration plan for the auth service, then open a PR against
      </p>

      {/* Live waveform */}
      <div className="mt-4 flex h-[34px] items-center gap-[3px]">
        {Array.from({ length: 64 }).map((_, i) => (
          <motion.span
            key={i}
            className="w-[3px] rounded-full bg-white/60"
            animate={{ height: [4, 6 + ((i * 37) % 24), 4] }}
            transition={{
              duration: 0.7 + ((i % 7) * 0.09),
              repeat: Infinity,
              ease: "easeInOut",
              delay: (i % 11) * 0.05,
            }}
          />
        ))}
      </div>

      <div className="mt-4 flex items-center gap-2.5">
        <span className="flex items-center gap-2 rounded-full bg-coral/20 px-3 py-1.5">
          <motion.span
            className="size-2 rounded-full bg-coral"
            animate={{ opacity: [1, 0.35, 1] }}
            transition={{ duration: 1.2, repeat: Infinity }}
          />
          <span className="text-xs font-medium">Recording</span>
        </span>
        <span className="font-mono text-xs text-white/55 tabular-nums">{mmss}</span>
        <div className="flex-1" />
        <button
          type="button"
          onClick={onStop}
          className="rounded-full bg-white/10 px-3 py-1.5 text-xs font-medium text-white/75 transition-colors hover:text-white"
        >
          Cancel
        </button>
        <motion.button
          type="button"
          onClick={onStop}
          whileTap={{ scale: 0.9 }}
          aria-label="Stop recording"
          className="flex size-[34px] items-center justify-center rounded-full bg-white"
        >
          <span className="size-[11px] rounded-[3px] bg-black" />
        </motion.button>
      </div>
    </>
  );
}

function GlassChip({
  icon,
  label,
  dropdown,
}: {
  icon: typeof Layers01Icon;
  label: string;
  dropdown?: boolean;
}) {
  return (
    <motion.button
      type="button"
      whileHover={{ y: -1 }}
      whileTap={{ scale: 0.96 }}
      transition={{ type: "spring", stiffness: 560, damping: 28 }}
      className="flex items-center gap-1.5 rounded-full bg-white/10 py-1.5 pl-2.5 pr-3 text-[13px] font-medium text-white/85 transition-colors hover:bg-white/[0.16]"
    >
      <HugeiconsIcon icon={icon} size={15} strokeWidth={1.5} />
      {label}
      {dropdown && (
        <HugeiconsIcon icon={ArrowDown01Icon} size={12} strokeWidth={2} className="opacity-70" />
      )}
    </motion.button>
  );
}

function GlassButton({
  icon,
  label,
  round,
}: {
  icon: typeof PlusSignIcon;
  label: string;
  round?: boolean;
}) {
  return (
    <motion.button
      type="button"
      aria-label={label}
      whileHover={{ y: -1 }}
      whileTap={{ scale: 0.94 }}
      transition={{ type: "spring", stiffness: 560, damping: 28 }}
      className={cn(
        "flex items-center justify-center bg-white/10 text-white/85 transition-colors hover:bg-white/[0.16]",
        round ? "size-[30px] rounded-full" : "rounded-full px-3 py-1.5",
      )}
    >
      <HugeiconsIcon icon={icon} size={15} strokeWidth={1.5} />
    </motion.button>
  );
}
