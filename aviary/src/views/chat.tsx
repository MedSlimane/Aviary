import { useCallback, useEffect, useRef, useState } from "react";
import * as motionReact from "motion/react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  PlusSignIcon,
  SparklesIcon,
  Mic01Icon,
  ArrowUp01Icon,
  ArrowDown01Icon,
  Alert02Icon,
} from "@hugeicons/core-free-icons";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  runTurn,
  listModels,
  PERMISSION_MODES,
  type PermissionMode,
  type Runner,
  type TurnEvent,
  type ModelCatalogue,
  type ModelOption,
  type ReasoningLevel,
} from "@/lib/api";
import { Input } from "@/components/ui/input";
import { EffortSlider } from "@/components/effort-slider";
import { LabMark, RUNNER_LAB } from "@/lib/lab-marks";
import { notify } from "@/lib/notify";
import { cn } from "@/lib/utils";

const { motion, AnimatePresence } = motionReact;

type Message =
  | { role: "user"; text: string }
  | { role: "assistant"; text: string }
  | { role: "tool"; name: string; summary: string }
  | { role: "error"; text: string };

type Block =
  | { kind: "message"; message: Message; index: number }
  /** Consecutive tool calls collapse into one group — a long run of them
   *  otherwise buries the prose between. */
  | { kind: "tools"; tools: { name: string; summary: string }[]; index: number };

function groupMessages(messages: Message[]): Block[] {
  const blocks: Block[] = [];
  for (const [i, m] of messages.entries()) {
    if (m.role === "tool") {
      const last = blocks[blocks.length - 1];
      if (last?.kind === "tools") {
        last.tools.push({ name: m.name, summary: m.summary });
        continue;
      }
      blocks.push({
        kind: "tools",
        tools: [{ name: m.name, summary: m.summary }],
        index: i,
      });
      continue;
    }
    blocks.push({ kind: "message", message: m, index: i });
  }
  return blocks;
}

const SUGGESTIONS = [
  "Summarise what changed in this repo today",
  "Which of my skills overlap?",
  "Audit my MCP servers",
];

const TOOL_TINT = [
  ["#fb7cc7", "#a78bfa"],
  ["#fcca6b", "#fc8c73"],
  ["#7dd3fc", "#5eead4"],
  ["#fc9e73", "#fa6b9e"],
] as const;

export function ChatView() {
  const [runner, setRunner] = useState<Runner>("claude-code");
  const [mode, setMode] = useState<PermissionMode>("Plan");
  const [catalogue, setCatalogue] = useState<ModelCatalogue | null>(null);
  const [model, setModel] = useState<ModelOption>({
    id: null,
    label: "Default",
    note: "",
    isAlias: false,
    reasoningLevels: [],
    defaultEffort: null,
  });
  const [effort, setEffort] = useState<string | null>(null);
  const [value, setValue] = useState("");
  const [messages, setMessages] = useState<Message[]>([]);
  const [running, setRunning] = useState(false);
  const [session, setSession] = useState<{
    model: string;
    tools: number;
    mcpServers: number;
  } | null>(null);
  const streamRef = useRef<HTMLDivElement>(null);

  const modeMeta = PERMISSION_MODES.find((m) => m.id === mode)!;
  const permissive = modeMeta.tone === "risky";

  // Models come from the runner itself, so a newly released one shows up
  // without a change here.
  useEffect(() => {
    let cancelled = false;
    listModels(runner)
      .then((c) => {
        if (cancelled) return;
        setCatalogue(c);
        setModel(c.models[0]);
        setEffort(c.models[0]?.defaultEffort ?? null);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [runner]);

  useEffect(() => {
    streamRef.current?.scrollTo({
      top: streamRef.current.scrollHeight,
      behavior: "smooth",
    });
  }, [messages, running]);

  const send = useCallback(async () => {
    const prompt = value.trim();
    if (!prompt || running) return;

    setMessages((m) => [...m, { role: "user", text: prompt }]);
    setValue("");
    setRunning(true);

    try {
      await runTurn(runner, prompt, mode, null, model.id, effort, (e: TurnEvent) => {
        switch (e.kind) {
          case "started":
            setSession({ model: e.model, tools: e.tools, mcpServers: e.mcpServers });
            break;
          case "text":
            setMessages((m) => [...m, { role: "assistant", text: e.text }]);
            break;
          case "tool-call":
            setMessages((m) => [
              ...m,
              { role: "tool", name: e.name, summary: e.summary },
            ]);
            break;
          case "failed":
            setMessages((m) => [...m, { role: "error", text: e.message }]);
            break;
          case "finished":
            if (e.isError) {
              setMessages((m) => [
                ...m,
                { role: "error", text: "The turn ended with an error." },
              ]);
            }
            break;
        }
      });
    } catch (err) {
      setMessages((m) => [
        ...m,
        { role: "error", text: err instanceof Error ? err.message : String(err) },
      ]);
    } finally {
      setRunning(false);
    }
  }, [value, running, runner, mode, model, effort]);

  const empty = messages.length === 0;

  return (
    <div className="relative flex h-full flex-col overflow-hidden">
      <div
        aria-hidden
        className="pointer-events-none absolute -left-40 -top-52 size-[680px] rounded-full opacity-[0.12] blur-[150px] dark:opacity-40"
        style={{ background: "radial-gradient(circle, #43156b, transparent 70%)" }}
      />
      <div
        aria-hidden
        className="pointer-events-none absolute -bottom-40 right-0 size-[560px] rounded-full opacity-[0.08] blur-[160px] dark:opacity-25"
        style={{ background: "radial-gradient(circle, #2e6e66, transparent 70%)" }}
      />

      {/* A permissive mode is stated for the whole session, not buried in a menu */}
      {permissive && (
        <div className="relative z-10 mx-8 mt-4 flex items-center gap-2.5 rounded-[10px] border border-coral/30 bg-coral/10 px-3.5 py-2.5">
          <span className="size-1.5 shrink-0 rounded-full bg-coral" />
          <p className="flex-1 text-[11px] font-medium">
            Running in {modeMeta.label} — this session will not ask before acting
          </p>
          <button
            type="button"
            onClick={() => setMode("Plan")}
            className="rounded-md bg-glass-hover px-2.5 py-1 text-[10px] font-medium transition-colors hover:text-on-glass"
          >
            Back to plan
          </button>
        </div>
      )}

      <div
        className={cn(
          "relative flex min-h-0 flex-1 flex-col px-8",
          empty ? "justify-center" : "justify-end",
        )}
      >
        {!empty && (
          <div
            ref={streamRef}
            className="mx-auto w-full max-w-[760px] flex-1 space-y-4 overflow-y-auto py-8"
          >
            <AnimatePresence initial={false}>
              {groupMessages(messages).map((b) =>
                b.kind === "tools" ? (
                  <motion.div
                    key={`tools-${b.index}`}
                    initial={{ opacity: 0, y: 6 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ duration: 0.16 }}
                  >
                    <ToolGroup tools={b.tools} />
                  </motion.div>
                ) : (
                  <MessageBlock key={b.index} m={b.message} />
                ),
              )}
            </AnimatePresence>

            {running && (
              <div className="av-glass flex w-fit items-center gap-2.5 rounded-full py-2 pl-2.5 pr-5">
                <motion.span
                  className="size-[15px] rounded-[5px]"
                  style={{
                    backgroundImage: "linear-gradient(135deg, #fb7cc7, #a78bfa)",
                  }}
                  animate={{ scale: [1, 1.14, 1] }}
                  transition={{ duration: 1.6, repeat: Infinity, ease: "easeInOut" }}
                />
                <span className="text-sm font-medium">Working…</span>
              </div>
            )}
          </div>
        )}

        {empty && (
          <div className="mx-auto mb-7 flex w-full max-w-[720px] flex-col items-center gap-3">
            <AviaryMark />
            <h2 className="text-[30px] font-semibold tracking-tight">
              What should we work on?
            </h2>
            <p className="text-[13px] text-on-glass-3">
              Runs your real CLI — tools, MCP and skills all apply
            </p>
          </div>
        )}

        <div className="mx-auto mb-6 w-full max-w-[760px]">
          <div className="av-glass rounded-[22px] p-[18px] pb-3 shadow-[0_16px_40px_-12px_rgba(0,0,0,0.35)]">
            <textarea
              value={value}
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  void send();
                }
              }}
              rows={1}
              disabled={running}
              placeholder={running ? "Running…" : "Ask anything…"}
              className="w-full resize-none bg-transparent text-[15px] outline-none placeholder:text-on-glass-3 disabled:opacity-60"
            />
            <div className="mt-4 flex flex-wrap items-center gap-2">
              <GlassButton icon={PlusSignIcon} label="Attach" />
              <RunnerPicker runner={runner} onChange={setRunner} />
              <ModelPicker
                runner={runner}
                catalogue={catalogue}
                model={model}
                onChange={(m) => {
                  setModel(m);
                  setEffort(m.defaultEffort ?? null);
                }}
              />
              {model.reasoningLevels.length > 1 && effort && (
                <EffortPicker
                  levels={model.reasoningLevels}
                  effort={effort}
                  onChange={setEffort}
                />
              )}
              <ModePicker mode={mode} onChange={setMode} />
              <div className="flex-1" />
              {session && (
                <span
                  className="font-mono text-[10px] text-on-glass-3"
                  title="Reported by the session itself"
                >
                  {session.model} · {session.tools} tools · {session.mcpServers} mcp
                </span>
              )}
              <motion.button
                type="button"
                whileTap={{ scale: 0.9 }}
                aria-label="Voice input"
                className="rounded-full p-1.5 text-on-glass-2 transition-colors hover:text-on-glass"
              >
                <HugeiconsIcon icon={Mic01Icon} size={18} strokeWidth={1.5} />
              </motion.button>
              <motion.button
                type="button"
                onClick={() => void send()}
                disabled={!value.trim() || running}
                whileHover={{ scale: value.trim() && !running ? 1.06 : 1 }}
                whileTap={{ scale: value.trim() && !running ? 0.92 : 1 }}
                transition={{ type: "spring", stiffness: 600, damping: 26 }}
                aria-label="Send"
                className="flex size-8 items-center justify-center rounded-full bg-foreground text-background disabled:opacity-35"
              >
                <HugeiconsIcon icon={ArrowUp01Icon} size={17} strokeWidth={2} />
              </motion.button>
            </div>
          </div>

          {empty && (
            <div className="mt-5 flex flex-wrap justify-center gap-2.5">
              {SUGGESTIONS.map((s) => (
                <motion.button
                  key={s}
                  type="button"
                  whileHover={{ y: -2 }}
                  whileTap={{ scale: 0.97 }}
                  transition={{ type: "spring", stiffness: 520, damping: 28 }}
                  onClick={() => setValue(s)}
                  className="rounded-full border border-glass-border bg-glass px-3.5 py-2 text-xs font-medium text-on-glass-2 transition-colors hover:text-on-glass"
                >
                  {s}
                </motion.button>
              ))}
            </div>
          )}

          {messages.length > 0 && !running && (
            <div className="mt-3 flex justify-center">
              <button
                type="button"
                onClick={() => {
                  setMessages([]);
                  setSession(null);
                }}
                className="text-[11px] text-on-glass-3 transition-colors hover:text-on-glass-2"
              >
                New chat
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function MessageBlock({ m }: { m: Message }) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.16 }}
    >
      {m.role === "user" && (
        <div className="flex justify-end">
          <div className="av-glass max-w-[460px] rounded-[20px] px-[18px] py-3 text-sm">
            {m.text}
          </div>
        </div>
      )}

      {m.role === "assistant" && (
        <p className="max-w-[660px] whitespace-pre-wrap text-sm leading-relaxed text-on-glass-2">
          {m.text}
        </p>
      )}

      {m.role === "error" && (
        <div className="flex max-w-[660px] items-start gap-2.5 rounded-[10px] border border-destructive/30 bg-destructive/10 px-3.5 py-2.5">
          <HugeiconsIcon
            icon={Alert02Icon}
            size={14}
            strokeWidth={1.8}
            className="mt-px shrink-0 text-destructive"
          />
          <p className="whitespace-pre-wrap text-[12px]">{m.text}</p>
        </div>
      )}
    </motion.div>
  );
}

/**
 * A run of consecutive tool calls, collapsed. Long runs are common — a single
 * question can trigger a dozen reads — and rendering each as its own pill
 * buries the prose that matters.
 */
function ToolGroup({ tools }: { tools: { name: string; summary: string }[] }) {
  const [open, setOpen] = useState(false);
  const single = tools.length === 1;

  // Distinct tool names, in order, for the collapsed summary.
  const names = [...new Set(tools.map((t) => t.name))];

  if (single) {
    return (
      <div className="av-glass flex w-fit max-w-full items-center gap-2.5 rounded-full py-2 pl-2.5 pr-5">
        <span
          className="size-[15px] shrink-0 rounded-[5px]"
          style={{
            backgroundImage: `linear-gradient(135deg, ${TOOL_TINT[0][0]}, ${TOOL_TINT[0][1]})`,
          }}
        />
        <span className="text-sm font-medium">{tools[0].name}</span>
        {tools[0].summary && (
          <span className="truncate font-mono text-[11px] text-on-glass-3">
            {tools[0].summary}
          </span>
        )}
      </div>
    );
  }

  return (
    <div className="av-glass w-fit max-w-full overflow-hidden rounded-[18px]">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center gap-2.5 py-2 pl-2.5 pr-4 text-left transition-colors hover:bg-glass-hover"
      >
        <span className="flex -space-x-1.5">
          {names.slice(0, 3).map((n, i) => (
            <span
              key={n}
              className="size-[15px] shrink-0 rounded-[5px] ring-2 ring-transparent"
              style={{
                backgroundImage: `linear-gradient(135deg, ${TOOL_TINT[i % 4][0]}, ${TOOL_TINT[i % 4][1]})`,
              }}
            />
          ))}
        </span>
        <span className="text-sm font-medium">
          {tools.length} tool calls
        </span>
        <span className="truncate font-mono text-[11px] text-on-glass-3">
          {names.slice(0, 3).join(" · ")}
          {names.length > 3 && ` +${names.length - 3}`}
        </span>
        <HugeiconsIcon
          icon={ArrowDown01Icon}
          size={13}
          strokeWidth={2}
          className={cn("shrink-0 transition-transform", open && "rotate-180")}
        />
      </button>

      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.18, ease: [0.22, 1, 0.36, 1] }}
            className="overflow-hidden"
          >
            <div className="space-y-1 border-t border-glass-border px-3 py-2.5">
              {tools.map((t, i) => (
                <div key={i} className="flex items-center gap-2.5">
                  <span
                    className="size-2 shrink-0 rounded-[3px]"
                    style={{
                      backgroundImage: `linear-gradient(135deg, ${TOOL_TINT[i % 4][0]}, ${TOOL_TINT[i % 4][1]})`,
                    }}
                  />
                  <span className="shrink-0 text-[12px] font-medium">{t.name}</span>
                  {t.summary && (
                    <span className="truncate font-mono text-[11px] text-on-glass-3">
                      {t.summary}
                    </span>
                  )}
                </div>
              ))}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/**
 * Effort lives beside the model rather than inside its menu: it is changed far
 * more often than the model, and burying it behind two clicks made a
 * per-message decision feel like a setting.
 */
function EffortPicker({
  levels,
  effort,
  onChange,
}: {
  levels: ReasoningLevel[];
  effort: string;
  onChange: (e: string) => void;
}) {
  const index = Math.max(0, levels.findIndex((l) => l.effort === effort));
  const pct = levels.length > 1 ? index / (levels.length - 1) : 0;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger className="flex items-center gap-2 rounded-full bg-glass-hover py-1.5 pl-2.5 pr-3 text-[13px] font-medium text-on-glass-2 transition-colors hover:text-on-glass">
        {/* A miniature of the track, so the level reads at a glance */}
        <span className="relative h-1.5 w-7 overflow-hidden rounded-full bg-white/15">
          <span
            className="av-gradient-fill absolute inset-y-0 left-0 rounded-full"
            style={{ width: `${Math.max(12, pct * 100)}%` }}
          />
        </span>
        <span className="capitalize">{effort}</span>
        <HugeiconsIcon icon={ArrowDown01Icon} size={12} strokeWidth={2} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-[300px] p-3">
        <p className="mb-2.5 text-[10px] font-semibold tracking-[0.8px] text-tertiary">
          REASONING EFFORT
        </p>
        <EffortSlider levels={levels} value={effort} onChange={onChange} />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ModelPicker({
  runner,
  catalogue,
  model,
  onChange,
}: {
  runner: Runner;
  catalogue: ModelCatalogue | null;
  model: ModelOption;
  onChange: (m: ModelOption) => void;
}) {
  const [custom, setCustom] = useState("");
  const models = catalogue?.models ?? [];

  return (
    <DropdownMenu>
      <DropdownMenuTrigger className="flex items-center gap-1.5 rounded-full bg-glass-hover py-1.5 pl-2.5 pr-3 text-[13px] font-medium text-on-glass-2 transition-colors hover:text-on-glass">
        <LabMark runner={runner} className="size-[13px]" />
        {model.label}
        <HugeiconsIcon icon={ArrowDown01Icon} size={12} strokeWidth={2} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-[320px]">
        <DropdownMenuRadioGroup
          value={model.id ?? "__default"}
          onValueChange={(v) => {
            const next = models.find((m) => (m.id ?? "__default") === v);
            if (next) onChange(next);
          }}
        >
          <DropdownMenuLabel className="flex items-center gap-2">
            <LabMark runner={runner} className="size-3" />
            {RUNNER_LAB[runner]}
          </DropdownMenuLabel>

          {models.map((m) => (
            <DropdownMenuRadioItem
              key={m.id ?? "__default"}
              value={m.id ?? "__default"}
            >
              <span className="min-w-0 flex-1">
                <span className="flex items-center gap-2">
                  <span className="text-[12px] font-medium">{m.label}</span>
                  {m.isAlias && (
                    <span
                      className="rounded-full bg-teal/20 px-1.5 py-px text-[9px] font-medium text-teal"
                      title="Always resolves to the newest model in this family"
                    >
                      latest
                    </span>
                  )}
                </span>
                {m.note && (
                  <span className="mt-0.5 block text-[11px] text-muted-foreground">
                    {m.note}
                  </span>
                )}
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>

        {/* An id we have never heard of must still be reachable. */}
        <div className="border-t border-border px-2 pb-1.5 pt-2">
          <Input
            value={custom}
            onChange={(e) => setCustom(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && custom.trim()) {
                onChange({
                  id: custom.trim(),
                  label: custom.trim(),
                  note: "Entered manually",
                  isAlias: false,
                  reasoningLevels: model.reasoningLevels,
                  defaultEffort: model.defaultEffort,
                });
                setCustom("");
              }
            }}
            placeholder="Or type a model id…"
            className="h-7 font-mono text-[11px]"
          />
          {catalogue && (
            <p className="mt-1.5 px-1 text-[10px] leading-snug text-tertiary">
              {catalogue.source}
            </p>
          )}
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function RunnerPicker({
  runner,
  onChange,
}: {
  runner: Runner;
  onChange: (r: Runner) => void;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger className="flex items-center gap-1.5 rounded-full bg-glass-hover py-1.5 pl-2.5 pr-3 text-[13px] font-medium text-on-glass-2 transition-colors hover:text-on-glass">
        <LabMark runner={runner} className="size-[13px]" />
        {runner === "claude-code" ? "Claude Code" : "Codex"}
        <HugeiconsIcon icon={ArrowDown01Icon} size={12} strokeWidth={2} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-44">
        <DropdownMenuRadioGroup
          value={runner}
          onValueChange={(v) => onChange(v as Runner)}
        >
          <DropdownMenuLabel>Runner</DropdownMenuLabel>
          <DropdownMenuRadioItem value="claude-code">
            Claude Code
          </DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="codex">Codex</DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ModePicker({
  mode,
  onChange,
}: {
  mode: PermissionMode;
  onChange: (m: PermissionMode) => void;
}) {
  const meta = PERMISSION_MODES.find((m) => m.id === mode)!;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        className={cn(
          "flex items-center gap-1.5 rounded-full py-1.5 pl-2.5 pr-3 font-mono text-[12px] font-medium transition-colors",
          meta.tone === "risky"
            ? "bg-coral/20 text-on-glass"
            : "bg-glass-hover text-on-glass-2 hover:text-on-glass",
        )}
      >
        <HugeiconsIcon icon={SparklesIcon} size={14} strokeWidth={1.5} />
        {meta.label}
        <HugeiconsIcon icon={ArrowDown01Icon} size={12} strokeWidth={2} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-[340px]">
        <DropdownMenuRadioGroup
          value={mode}
          onValueChange={(v) => {
            onChange(v as PermissionMode);
            const m = PERMISSION_MODES.find((x) => x.id === v);
            if (m?.tone === "risky") {
              notify(`Permission mode: ${m.label}`, {
                description: "This session will act without asking.",
              });
            }
          }}
        >
          <DropdownMenuLabel>Permission mode</DropdownMenuLabel>
          {PERMISSION_MODES.map((m) => (
            <DropdownMenuRadioItem key={m.id} value={m.id}>
              <span className="min-w-0 flex-1">
                <span className="flex items-center gap-2">
                  <span className="font-mono text-[12px]">{m.label}</span>
                  {m.tag && (
                    <span
                      className={cn(
                        "rounded-full px-1.5 py-px text-[9px] font-medium",
                        m.tone === "risky"
                          ? "bg-destructive/20 text-destructive"
                          : "bg-teal/20 text-teal",
                      )}
                    >
                      {m.tag}
                    </span>
                  )}
                </span>
                <span className="mt-0.5 block text-[11px] text-muted-foreground">
                  {m.desc}
                </span>
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function AviaryMark() {
  const petal =
    "M64 59C58.9 54.3 57.4 46.5 60.2 34.3L64 14C67.8 29.2 70.2 43.2 68.7 49.5C67.8 53.7 66.1 57 64 59Z";
  return (
    <svg viewBox="0 0 128 128" className="size-[42px] text-violet">
      {[0, 60, 120, 180, 240, 300].map((a) => (
        <path key={a} d={petal} fill="currentColor" transform={`rotate(${a} 64 64)`} />
      ))}
      <circle cx="64" cy="64" r="7.2" fill="currentColor" />
    </svg>
  );
}

function GlassButton({
  icon,
  label,
}: {
  icon: typeof PlusSignIcon;
  label: string;
}) {
  return (
    <motion.button
      type="button"
      aria-label={label}
      whileTap={{ scale: 0.94 }}
      className="flex size-[30px] items-center justify-center rounded-full bg-glass-hover text-on-glass-2 transition-colors hover:text-on-glass"
    >
      <HugeiconsIcon icon={icon} size={15} strokeWidth={1.5} />
    </motion.button>
  );
}
