import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import * as motionReact from "motion/react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Alert02Icon,
  ArrowDown01Icon,
  ArrowUp01Icon,
  Cancel01Icon,
  Folder01Icon,
  PackageIcon,
  PlusSignIcon,
  RefreshIcon,
  SparklesIcon,
} from "@hugeicons/core-free-icons";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { EffortSlider } from "@/components/effort-slider";
import { LabMark, RUNNER_LAB } from "@/lib/lab-marks";
import {
  createChatSession,
  createChatSessionWithBundle,
  discoverRunnerSafety,
  interruptTurn,
  listChatSessions,
  listBundles,
  listModels,
  listProjects,
  loadChatSession,
  loadSessionBundle,
  prepareBundleChat,
  readEntry,
  respondPermission,
  resumeChatSession,
  RUNNER_LABEL,
  type EngineEvent,
  type ChatRunHandle,
  type ModelCatalogue,
  type ModelOption,
  type PermissionDecision,
  type PermissionQuestion,
  type Project,
  type ResolvedBundle,
  type ReasoningLevel,
  type Runner,
  type SafetyCapabilities,
  type SafetyOption,
  type SessionDetail,
  type SessionEvent,
  type SessionSummary,
  type SessionBundleAttachment,
  type StoredEvent,
  type ToolResultStatus,
  type TurnStatus,
} from "@/lib/api";
import { notify } from "@/lib/notify";
import { useBoolPreference } from "@/lib/use-preference";
import { useLibrary } from "@/lib/use-library";
import { cn } from "@/lib/utils";

const { motion } = motionReact;

type ViewTurn = {
  id: string;
  prompt: string;
  status: TurnStatus;
  permissionMode: string | null;
  requestedModel: string | null;
  requestedEffort: string | null;
  durationMs: number | null;
  events: StoredEvent[];
};

type ToolBlock = {
  kind: "tool";
  key: string;
  callId: string | null;
  name: string;
  summary: string;
  detail: string | null;
  status: "running" | ToolResultStatus;
};

type PermissionRequestEvent = Extract<
  SessionEvent,
  { kind: "permission-request" }
>;

type TranscriptBlock =
  | { kind: "text"; key: string; text: string }
  | { kind: "thinking"; key: string; text: string }
  | ToolBlock
  | {
      kind: "permission";
      key: string;
      request: PermissionRequestEvent;
      decision: string | null;
    }
  | { kind: "failed"; key: string; message: string; failure: string }
  | { kind: "interrupted"; key: string };

const EMPTY_MODEL: ModelOption = {
  id: null,
  label: "Runner default",
  note: "",
  isAlias: false,
  reasoningLevels: [],
  defaultEffort: null,
};

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

function formatTimestamp(epochSeconds: number) {
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(new Date(epochSeconds * 1_000));
}

function formatDuration(durationMs: number) {
  if (durationMs < 1_000) return `${durationMs} ms`;
  return `${(durationMs / 1_000).toFixed(durationMs < 10_000 ? 1 : 0)} s`;
}

function formatCount(value: number) {
  return new Intl.NumberFormat().format(value);
}

function statusLabel(status: TurnStatus) {
  switch (status) {
    case "queued":
      return "Queued";
    case "running":
      return "Running";
    case "completed":
      return "Completed";
    case "failed":
      return "Failed";
    case "interrupted":
      return "Interrupted";
  }
}

function decisionLabel(decision: string) {
  switch (decision) {
    case "allow-once":
      return "Allowed once";
    case "allow-session":
      return "Allowed for session";
    case "deny":
      return "Denied";
    case "cancel":
      return "Cancelled";
    case "submit":
      return "Submitted";
    case "turn-interrupted":
      return "Expired when turn was interrupted";
    case "runner-ended":
      return "Expired when runner ended";
    case "cancelled-by-runner":
      return "Cancelled by runner";
    default:
      return decision;
  }
}

function actionLabel(action: PermissionDecision) {
  switch (action) {
    case "allow-once":
      return "Allow once";
    case "allow-session":
      return "Allow for session";
    case "deny":
      return "Deny";
    case "cancel":
      return "Cancel turn";
    case "submit":
      return "Submit answers";
  }
}

function eventTurnStatus(events: StoredEvent[], stored: TurnStatus): TurnStatus {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index].event;
    if (event.kind === "finished") return "completed";
    if (event.kind === "failed") return "failed";
    if (event.kind === "interrupted") return "interrupted";
  }
  return stored;
}

function mergeViewTurns(
  detail: SessionDetail | null,
  liveEvents: StoredEvent[],
  optimisticPrompt: string | null,
  activeTurnId: string | null,
): ViewTurn[] {
  const turns: ViewTurn[] = (detail?.turns ?? []).map(({ turn, events }) => ({
    id: turn.id,
    prompt: turn.prompt,
    status: turn.status,
    permissionMode: turn.permissionMode,
    requestedModel: turn.requestedModel,
    requestedEffort: turn.requestedEffort,
    durationMs: turn.durationMs,
    events: [...events],
  }));
  const byTurn = new Map(turns.map((turn) => [turn.id, turn]));

  for (const event of liveEvents) {
    let turn = byTurn.get(event.turnId);
    if (!turn) {
      turn = {
        id: event.turnId,
        prompt: optimisticPrompt ?? "",
        status: "running",
        permissionMode: null,
        requestedModel: null,
        requestedEffort: null,
        durationMs: null,
        events: [],
      };
      byTurn.set(event.turnId, turn);
      turns.push(turn);
    }
    if (!turn.events.some((stored) => stored.id === event.id)) {
      turn.events.push(event);
    }
  }

  if (optimisticPrompt && !activeTurnId) {
    turns.push({
      id: "optimistic-turn",
      prompt: optimisticPrompt,
      status: "queued",
      permissionMode: null,
      requestedModel: null,
      requestedEffort: null,
      durationMs: null,
      events: [],
    });
  }

  return turns.map((turn) => {
    turn.events.sort((left, right) => left.sequence - right.sequence);
    return { ...turn, status: eventTurnStatus(turn.events, turn.status) };
  });
}

function buildTranscript(events: StoredEvent[]): TranscriptBlock[] {
  const blocks: TranscriptBlock[] = [];
  const toolByCall = new Map<string, number>();
  const permissionByRequest = new Map<string, number>();

  for (const stored of events) {
    const event = stored.event;
    const key = String(stored.id);
    switch (event.kind) {
      case "text":
      case "thinking": {
        const last = blocks[blocks.length - 1];
        if (last?.kind === event.kind) {
          last.text += event.text;
        } else {
          blocks.push({ kind: event.kind, key, text: event.text });
        }
        break;
      }
      case "tool-call":
      case "tool-started": {
        const callId = event.callId;
        const index = blocks.length;
        blocks.push({
          kind: "tool",
          key,
          callId,
          name: event.name,
          summary: event.summary,
          detail: event.detail,
          status: "running",
        });
        if (callId) toolByCall.set(callId, index);
        break;
      }
      case "tool-updated": {
        const index = toolByCall.get(event.callId);
        const block = index === undefined ? undefined : blocks[index];
        if (block?.kind === "tool") {
          block.name = event.name || block.name;
          block.summary = event.summary || block.summary;
          block.detail = event.detail ?? block.detail;
        } else {
          const nextIndex = blocks.length;
          blocks.push({
            kind: "tool",
            key,
            callId: event.callId,
            name: event.name,
            summary: event.summary,
            detail: event.detail,
            status: "running",
          });
          toolByCall.set(event.callId, nextIndex);
        }
        break;
      }
      case "tool-result":
      case "tool-finished": {
        const index = event.callId ? toolByCall.get(event.callId) : undefined;
        const block = index === undefined ? undefined : blocks[index];
        if (block?.kind === "tool") {
          if ("name" in event && event.name) block.name = event.name;
          block.summary = event.summary || block.summary;
          block.detail = event.detail ?? block.detail;
          block.status = event.status;
        } else {
          const nextIndex = blocks.length;
          blocks.push({
            kind: "tool",
            key,
            callId: event.callId,
            name: "name" in event ? event.name : "Tool result",
            summary: event.summary,
            detail: event.detail,
            status: event.status,
          });
          if (event.callId) toolByCall.set(event.callId, nextIndex);
        }
        break;
      }
      case "permission-request": {
        permissionByRequest.set(event.requestId, blocks.length);
        blocks.push({
          kind: "permission",
          key,
          request: event,
          decision: null,
        });
        break;
      }
      case "permission-resolved": {
        const index = permissionByRequest.get(event.requestId);
        const block = index === undefined ? undefined : blocks[index];
        if (block?.kind === "permission") block.decision = event.decision;
        break;
      }
      case "failed":
        blocks.push({
          kind: "failed",
          key,
          message: event.displayMessage,
          failure: event.failure,
        });
        break;
      case "interrupted":
        blocks.push({ kind: "interrupted", key });
        break;
      case "started":
      case "token-usage":
      case "finished":
        break;
    }
  }
  return blocks;
}

function lastEventOfKind<K extends SessionEvent["kind"]>(
  turns: ViewTurn[],
  kind: K,
): Extract<SessionEvent, { kind: K }> | null {
  for (let turnIndex = turns.length - 1; turnIndex >= 0; turnIndex -= 1) {
    const events = turns[turnIndex].events;
    for (let eventIndex = events.length - 1; eventIndex >= 0; eventIndex -= 1) {
      const event = events[eventIndex].event;
      if (event.kind === kind) {
        return event as Extract<SessionEvent, { kind: K }>;
      }
    }
  }
  return null;
}

function unfinishedTurnId(detail: SessionDetail | null) {
  if (!detail) return null;
  for (let index = detail.turns.length - 1; index >= 0; index -= 1) {
    const turn = detail.turns[index].turn;
    if (turn.status === "queued" || turn.status === "running") return turn.id;
  }
  return null;
}

function isTerminalEvent(event: SessionEvent) {
  return (
    event.kind === "finished" ||
    event.kind === "failed" ||
    event.kind === "interrupted"
  );
}

function isTerminalStatus(status: TurnStatus) {
  return status === "completed" || status === "failed" || status === "interrupted";
}

export function ChatView() {
  const { data: library } = useLibrary();
  const [runner, setRunner] = useState<Runner>("claude-code");
  const [sessions, setSessions] = useState<SessionSummary[] | null>(null);
  const [sessionsError, setSessionsError] = useState<string | null>(null);
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [bundles, setBundles] = useState<ResolvedBundle[] | null>(null);
  const [selectedBundle, setSelectedBundle] = useState<ResolvedBundle | null>(null);
  const [sessionBundle, setSessionBundle] =
    useState<SessionBundleAttachment | null>(null);
  const [bundleBusy, setBundleBusy] = useState(false);
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [cwd, setCwd] = useState("");
  const [value, setValue] = useState("");
  const [running, setRunning] = useState(false);
  const [interrupting, setInterrupting] = useState(false);
  const [activeTurnId, setActiveTurnId] = useState<string | null>(null);
  const [liveEvents, setLiveEvents] = useState<StoredEvent[]>([]);
  const [optimisticPrompt, setOptimisticPrompt] = useState<string | null>(null);
  const [runError, setRunError] = useState<string | null>(null);
  const [safety, setSafety] = useState<SafetyCapabilities | null>(null);
  const [safetyId, setSafetyId] = useState<string | null>(null);
  const [catalogue, setCatalogue] = useState<ModelCatalogue | null>(null);
  const [model, setModel] = useState<ModelOption>(EMPTY_MODEL);
  const [effort, setEffort] = useState<string | null>(null);
  const [allowRisky] = useBoolPreference("chat.allowRiskyPermissionModes");
  const streamRef = useRef<HTMLDivElement>(null);
  const detailRequestRef = useRef(0);
  const sessionsRequestRef = useRef(0);
  const runChannelRef = useRef<ChatRunHandle["channel"] | null>(null);
  const reconciliationTimerRef = useRef<number | null>(null);
  const finishedTurnRef = useRef<string | null>(null);
  const runGenerationRef = useRef(0);
  const runInFlightRef = useRef(false);
  const interruptInFlightRef = useRef(false);
  const bundleInFlightRef = useRef(false);
  const bundleRequestRef = useRef(0);

  const refreshSessions = useCallback(async () => {
    const request = ++sessionsRequestRef.current;
    try {
      const next = await listChatSessions();
      if (request !== sessionsRequestRef.current) return;
      setSessions(next);
      setSessionsError(null);
    } catch (error) {
      if (request !== sessionsRequestRef.current) return;
      setSessions((current) => current ?? []);
      setSessionsError(errorMessage(error));
    }
  }, []);

  useEffect(() => {
    void refreshSessions();
    listProjects()
      .then(setProjects)
      .catch((error) => {
        setProjects([]);
        notify("Could not load projects", { description: errorMessage(error) });
      });
    listBundles()
      .then(setBundles)
      .catch((error) => {
        setBundles([]);
        notify("Could not load bundles", { description: errorMessage(error) });
      });
  }, [refreshSessions]);

  useEffect(() => {
    let alive = true;
    setSafety(null);
    setSafetyId(null);
    setCatalogue(null);
    setModel(EMPTY_MODEL);
    setEffort(null);
    void Promise.allSettled([
      discoverRunnerSafety(runner),
      listModels(runner),
    ]).then(([safetyResult, modelResult]) => {
      if (!alive) return;
      if (safetyResult.status === "fulfilled") {
        setSafety(safetyResult.value);
        setSafetyId(safetyResult.value.defaultOptionId);
      } else {
        setSafety({
          runner,
          available: false,
          protocol: "",
          defaultOptionId: null,
          options: [],
          warning: errorMessage(safetyResult.reason),
        });
      }
      if (modelResult.status === "fulfilled") {
        const nextCatalogue = modelResult.value;
        const attachedModel =
          selectedBundle?.bundle.modelId ?? sessionBundle?.snapshot.modelId;
        const hasAttachedBundle = selectedBundle !== null || sessionBundle !== null;
        const initial = hasAttachedBundle
          ? nextCatalogue.models.find(
              (candidate) => candidate.id === (attachedModel ?? null),
            ) ??
            (attachedModel
              ? {
                  id: attachedModel,
                  label: attachedModel,
                  note: "Locked by bundle",
                  isAlias: false,
                  reasoningLevels: [],
                  defaultEffort: null,
                }
              : EMPTY_MODEL)
          : nextCatalogue.models.find(
                (candidate) => candidate.id === nextCatalogue.configuredDefault,
              ) ?? nextCatalogue.models[0] ?? EMPTY_MODEL;
        setCatalogue(nextCatalogue);
        setModel(initial);
        setEffort(initial.defaultEffort);
      } else {
        notify(`Could not discover ${RUNNER_LABEL[runner]} models`, {
          description: errorMessage(modelResult.reason),
        });
      }
    });
    return () => {
      alive = false;
    };
  }, [runner, selectedBundle, sessionBundle]);

  useEffect(() => {
    if (allowRisky || !safety || !safetyId) return;
    const selected = safety.options.find((option) => option.id === safetyId);
    if (selected?.dangerous) setSafetyId(safety.defaultOptionId);
  }, [allowRisky, safety, safetyId]);

  const turns = useMemo(
    () => mergeViewTurns(detail, liveEvents, optimisticPrompt, activeTurnId),
    [activeTurnId, detail, liveEvents, optimisticPrompt],
  );
  const started = useMemo(() => lastEventOfKind(turns, "started"), [turns]);
  const tokenUsage = useMemo(
    () => lastEventOfKind(turns, "token-usage"),
    [turns],
  );
  const lastStoredTurn = detail?.turns[detail.turns.length - 1];
  const detachedTurnId = unfinishedTurnId(detail);
  const interruptibleTurnId = activeTurnId ?? detachedTurnId;
  const scrollSignal =
    liveEvents[liveEvents.length - 1]?.id ??
    lastStoredTurn?.events[lastStoredTurn.events.length - 1]?.id ??
    0;

  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      streamRef.current?.scrollTo({
        top: streamRef.current.scrollHeight,
        behavior: running ? "smooth" : "auto",
      });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [running, scrollSignal, selectedSessionId]);

  const openSession = useCallback(
    async (summary: SessionSummary) => {
      if (runInFlightRef.current) return;
      bundleRequestRef.current += 1;
      bundleInFlightRef.current = false;
      setBundleBusy(false);
      runGenerationRef.current += 1;
      finishedTurnRef.current = null;
      interruptInFlightRef.current = false;
      setInterrupting(false);
      const request = ++detailRequestRef.current;
      setSelectedSessionId(summary.session.id);
      setLoadingDetail(true);
      setRunError(null);
      setLiveEvents([]);
      setOptimisticPrompt(null);
      setActiveTurnId(null);
      setSelectedBundle(null);
      setSessionBundle(null);
      try {
        const [loaded, attached] = await Promise.all([
          loadChatSession(summary.session.id),
          loadSessionBundle(summary.session.id),
        ]);
        if (request !== detailRequestRef.current) return;
        if (!loaded) throw new Error("The chat session no longer exists.");
        setDetail(loaded);
        setSessionBundle(attached);
        setRunner(loaded.session.runner);
        setCwd(loaded.session.cwd);
      } catch (error) {
        if (request !== detailRequestRef.current) return;
        setDetail(null);
        setRunError(errorMessage(error));
      } finally {
        if (request === detailRequestRef.current) setLoadingDetail(false);
      }
    },
    [],
  );

  const newChat = useCallback(() => {
    if (runInFlightRef.current) return;
    bundleRequestRef.current += 1;
    bundleInFlightRef.current = false;
    runGenerationRef.current += 1;
    finishedTurnRef.current = null;
    interruptInFlightRef.current = false;
    detailRequestRef.current += 1;
    setSelectedSessionId(null);
    setDetail(null);
    setSelectedBundle(null);
    setSessionBundle(null);
    setBundleBusy(false);
    setCwd("");
    setValue("");
    setRunError(null);
    setLoadingDetail(false);
    setInterrupting(false);
    setLiveEvents([]);
    setOptimisticPrompt(null);
    setActiveTurnId(null);
  }, []);

  const handleEngineEvent = useCallback((incoming: EngineEvent) => {
    setSelectedSessionId(incoming.sessionId);
    setActiveTurnId(incoming.stored.turnId);
    setLiveEvents((current) =>
      current.some((event) => event.id === incoming.stored.id)
        ? current
        : [...current, incoming.stored],
    );
  }, []);

  const finishLiveRun = useCallback(
    async (sessionId: string, turnId: string, generation: number) => {
      if (generation !== runGenerationRef.current) return;
      if (finishedTurnRef.current === turnId) return;
      finishedTurnRef.current = turnId;
      if (reconciliationTimerRef.current !== null) {
        window.clearTimeout(reconciliationTimerRef.current);
        reconciliationTimerRef.current = null;
      }
      runChannelRef.current = null;
      runInFlightRef.current = false;
      interruptInFlightRef.current = false;
      setRunning(false);
      setInterrupting(false);
      setOptimisticPrompt(null);
      setActiveTurnId(null);
      try {
        const loaded = await loadChatSession(sessionId);
        if (generation !== runGenerationRef.current) return;
        if (loaded) {
          setDetail(loaded);
          setSelectedSessionId(loaded.session.id);
          setRunner(loaded.session.runner);
          setCwd(loaded.session.cwd);
          setLiveEvents([]);
          setRunError(null);
        }
      } catch (error) {
        if (generation === runGenerationRef.current) {
          setRunError(errorMessage(error));
        }
      } finally {
        if (generation === runGenerationRef.current) {
          void refreshSessions();
        }
      }
    },
    [refreshSessions],
  );

  const startReconciliationPoll = useCallback(
    (sessionId: string, turnId: string, generation: number) => {
      if (generation !== runGenerationRef.current) return;
      if (reconciliationTimerRef.current !== null) {
        window.clearTimeout(reconciliationTimerRef.current);
      }
      const schedule = () => {
        if (
          generation !== runGenerationRef.current ||
          finishedTurnRef.current === turnId
        ) {
          return;
        }
        reconciliationTimerRef.current = window.setTimeout(() => {
          reconciliationTimerRef.current = null;
          void loadChatSession(sessionId)
            .then((loaded) => {
              if (generation !== runGenerationRef.current) return;
              const stored = loaded?.turns.find(
                (turn) => turn.turn.id === turnId,
              );
              if (stored && isTerminalStatus(stored.turn.status)) {
                void finishLiveRun(sessionId, turnId, generation);
                return;
              }
              schedule();
            })
            .catch(() => {
              // The live channel remains authoritative. A later poll can
              // recover if this transient reconciliation read fails.
              schedule();
            });
        }, 1_500);
      };
      schedule();
    },
    [finishLiveRun],
  );

  useEffect(
    () => () => {
      if (reconciliationTimerRef.current !== null) {
        window.clearTimeout(reconciliationTimerRef.current);
      }
      runChannelRef.current = null;
      runInFlightRef.current = false;
      interruptInFlightRef.current = false;
      runGenerationRef.current += 1;
    },
    [],
  );

  const send = useCallback(async () => {
    const prompt = value.trim();
    if (
      !prompt ||
      runInFlightRef.current ||
      running ||
      loadingDetail ||
      Boolean(detachedTurnId) ||
      !cwd ||
      !safety?.available ||
      !safetyId
    ) {
      return;
    }

    runInFlightRef.current = true;
    interruptInFlightRef.current = false;
    detailRequestRef.current += 1;
    setValue("");
    setRunError(null);
    setRunning(true);
    setOptimisticPrompt(prompt);
    setLiveEvents([]);
    setActiveTurnId(null);
    finishedTurnRef.current = null;
    const generation = runGenerationRef.current + 1;
    runGenerationRef.current = generation;
    let eventSessionId = selectedSessionId;
    const onEvent = (incoming: EngineEvent) => {
      if (generation !== runGenerationRef.current) return;
      eventSessionId = incoming.sessionId;
      handleEngineEvent(incoming);
      if (isTerminalEvent(incoming.stored.event)) {
        void finishLiveRun(
          incoming.sessionId,
          incoming.stored.turnId,
          generation,
        );
      }
    };

    const options = {
      prompt,
      safetyOptionId: safetyId,
      model: selectedBundle ? selectedBundle.bundle.modelId : model.id,
      effort,
    };
    let handle: ChatRunHandle;
    try {
      handle = selectedSessionId
        ? await resumeChatSession(selectedSessionId, options, onEvent)
        : selectedBundle
          ? await createChatSessionWithBundle(
              selectedBundle.bundle.id,
              selectedBundle.bundle.revision,
              null,
              options,
              onEvent,
            )
          : await createChatSession(runner, cwd, null, options, onEvent);
    } catch (error) {
      if (generation !== runGenerationRef.current) return;
      const recoveryGeneration = generation + 1;
      runGenerationRef.current = recoveryGeneration;
      const message = errorMessage(error);
      setRunError(message);
      if (reconciliationTimerRef.current !== null) {
        window.clearTimeout(reconciliationTimerRef.current);
        reconciliationTimerRef.current = null;
      }
      runChannelRef.current = null;
      runInFlightRef.current = false;
      interruptInFlightRef.current = false;
      setRunning(false);
      setInterrupting(false);
      setOptimisticPrompt(null);
      setActiveTurnId(null);
      finishedTurnRef.current = null;
      if (eventSessionId) {
        const loaded = await loadChatSession(eventSessionId).catch(() => null);
        if (recoveryGeneration !== runGenerationRef.current) return;
        if (loaded) {
          setDetail(loaded);
          setSelectedSessionId(loaded.session.id);
          setRunner(loaded.session.runner);
          setCwd(loaded.session.cwd);
        }
      }
      void refreshSessions();
      return;
    }

    if (generation !== runGenerationRef.current) return;
    const { receipt } = handle;
    const alreadyFinished = finishedTurnRef.current === receipt.turn.id;
    setSelectedSessionId(receipt.session.id);
    setRunner(receipt.session.runner);
    setCwd(receipt.session.cwd);
    if (alreadyFinished) return;

    runChannelRef.current = handle.channel;
    setActiveTurnId(receipt.turn.id);
    startReconciliationPoll(receipt.session.id, receipt.turn.id, generation);
    try {
      const loaded = await loadChatSession(receipt.session.id);
      if (
        generation !== runGenerationRef.current ||
        finishedTurnRef.current === receipt.turn.id
      ) {
        return;
      }
      if (loaded) {
        setDetail(loaded);
        setSelectedSessionId(loaded.session.id);
        setRunner(loaded.session.runner);
        setCwd(loaded.session.cwd);
        const stored = loaded.turns.find(
          (turn) => turn.turn.id === receipt.turn.id,
        );
        if (stored && isTerminalStatus(stored.turn.status)) {
          void finishLiveRun(receipt.session.id, receipt.turn.id, generation);
        }
      }
    } catch (error) {
      if (
        generation === runGenerationRef.current &&
        finishedTurnRef.current !== receipt.turn.id
      ) {
        setRunError(
          `The turn started, but its saved transcript could not be refreshed: ${errorMessage(error)}`,
        );
      }
    }
  }, [
    cwd,
    detachedTurnId,
    effort,
    finishLiveRun,
    handleEngineEvent,
    loadingDetail,
    model.id,
    refreshSessions,
    runner,
    running,
    safety?.available,
    safetyId,
    selectedBundle,
    selectedSessionId,
    startReconciliationPoll,
    value,
  ]);

  const interrupt = useCallback(async () => {
    const turnId = interruptibleTurnId;
    if (!turnId || interruptInFlightRef.current) return;
    interruptInFlightRef.current = true;
    const generation = runGenerationRef.current;
    const detailRequest = detailRequestRef.current;
    const sessionId = selectedSessionId;
    const wasLiveRun = runInFlightRef.current && activeTurnId === turnId;
    let accepted = false;
    setInterrupting(true);
    try {
      await interruptTurn(turnId);
      accepted = true;
      if (generation !== runGenerationRef.current || wasLiveRun || !sessionId) {
        return;
      }

      notify("Interrupt requested");
      await new Promise<void>((resolve) => {
        window.setTimeout(resolve, 500);
      });
      if (
        generation !== runGenerationRef.current ||
        detailRequest !== detailRequestRef.current
      ) {
        return;
      }
      const loaded = await loadChatSession(sessionId);
      if (
        generation !== runGenerationRef.current ||
        detailRequest !== detailRequestRef.current
      ) {
        return;
      }
      if (loaded) {
        setDetail(loaded);
        setRunner(loaded.session.runner);
        setCwd(loaded.session.cwd);
      }
    } catch (error) {
      if (
        generation === runGenerationRef.current &&
        finishedTurnRef.current !== turnId
      ) {
        notify(
          accepted
            ? "Could not refresh the interrupted turn"
            : "Could not interrupt the turn",
          {
            description: errorMessage(error),
          },
        );
      }
    } finally {
      if (generation !== runGenerationRef.current) return;
      const waitingForTerminal =
        accepted &&
        wasLiveRun &&
        runInFlightRef.current &&
        finishedTurnRef.current !== turnId;
      if (!waitingForTerminal) {
        interruptInFlightRef.current = false;
        setInterrupting(false);
        void refreshSessions();
      }
    }
  }, [
    activeTurnId,
    interruptibleTurnId,
    refreshSessions,
    selectedSessionId,
  ]);

  const chooseBundle = useCallback(
    async (next: ResolvedBundle | null) => {
      if (
        bundleInFlightRef.current ||
        running ||
        detail !== null ||
        turns.length > 0
      ) {
        return;
      }
      if (next === null) {
        bundleRequestRef.current += 1;
        bundleInFlightRef.current = false;
        setBundleBusy(false);
        setSelectedBundle(null);
        setSessionBundle(null);
        setRunError(null);
        return;
      }
      const unavailable = next.members.find(
        ({ resolution }) => resolution.status !== "ready",
      );
      if (unavailable) {
        setRunError(
          `${unavailable.member.snapshotLabel}: ${unavailable.resolution.reason ?? unavailable.resolution.status}`,
        );
        return;
      }

      bundleInFlightRef.current = true;
      const request = ++bundleRequestRef.current;
      setBundleBusy(true);
      setRunError(null);
      try {
        const plan = await prepareBundleChat(
          next.bundle.id,
          next.bundle.revision,
        );
        if (request !== bundleRequestRef.current) return;
        const promptMember = next.bundle.members.find(
          (member) => member.kind === "prompt" && member.role === "prefill",
        );
        let prefill: string | null = null;
        if (promptMember) {
          if (promptMember.target.type !== "entry") {
            throw new Error("The bundle prompt target is invalid.");
          }
          const promptTargetId = promptMember.target.id;
          const entry = library?.entries.find(
            (candidate) => candidate.id === promptTargetId,
          );
          if (!entry) {
            throw new Error(
              "The bundle prompt is not present in the current library index.",
            );
          }
          prefill = (await readEntry(entry.path)).body;
          if (request !== bundleRequestRef.current) return;
          if (
            value.trim() &&
            prefill !== value &&
            !window.confirm("Replace the current draft with this bundle's prompt?")
          ) {
            return;
          }
        }

        setSelectedBundle(next);
        setSessionBundle(null);
        setRunner(plan.runner);
        setCwd(plan.cwd);
        if (prefill !== null) setValue(prefill);
        notify(`Attached ${next.bundle.name}`, {
          description: "Runner, model, and working directory now follow this saved revision.",
        });
      } catch (error) {
        const message = errorMessage(error);
        setRunError(message);
        notify("Could not attach bundle", { description: message });
      } finally {
        if (request === bundleRequestRef.current) {
          bundleInFlightRef.current = false;
          setBundleBusy(false);
        }
      }
    },
    [detail, library, running, turns.length, value],
  );

  const locked = turns.length > 0 || detail !== null;
  const bundleConfiguration = selectedBundle !== null || sessionBundle !== null;
  const activeBundleName =
    selectedBundle?.bundle.name ?? sessionBundle?.snapshot.bundleName ?? null;
  const selectedSafety = safety?.options.find((option) => option.id === safetyId);
  const composerBusy = running || Boolean(detachedTurnId);
  const canSend = Boolean(
    value.trim() &&
      !runInFlightRef.current &&
      !composerBusy &&
      !bundleBusy &&
      !loadingDetail &&
      cwd &&
      safety?.available &&
      safetyId,
  );
  const title = detail?.session.title ?? (running ? "Starting chat…" : "New chat");
  const displayedCwd = detail?.session.cwd ?? started?.cwd ?? cwd;

  return (
    <div className="flex h-full min-w-0 bg-background text-foreground">
      <SessionSidebar
        sessions={sessions}
        error={sessionsError}
        selectedId={selectedSessionId}
        running={running}
        onOpen={openSession}
        onNew={newChat}
        onRefresh={refreshSessions}
      />

      <section className="flex min-w-0 flex-1 flex-col" aria-label="Chat workspace">
        <header className="flex min-h-[62px] items-center gap-3 border-b border-border px-5 py-3">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <LabMark runner={runner} className="size-3.5" />
              <h1 className="truncate text-sm font-semibold">{title}</h1>
              {activeBundleName ? (
                <span className="max-w-[220px] truncate rounded-full border border-violet/25 bg-violet/10 px-2 py-0.5 text-[9px] font-medium text-violet">
                  {activeBundleName}
                </span>
              ) : null}
              {running ? (
                <span className="rounded-full bg-blue/15 px-2 py-0.5 text-[10px] font-medium text-blue">
                  Running
                </span>
              ) : null}
            </div>
            <p
              className="mt-1 truncate font-mono text-[10px] text-tertiary"
              title={displayedCwd || undefined}
            >
              {displayedCwd || "Choose a working directory before the first turn"}
            </p>
          </div>
          <SessionMetadata started={started} tokenUsage={tokenUsage} />
          {running || detachedTurnId ? (
            <button
              type="button"
              onClick={() => void interrupt()}
              disabled={!interruptibleTurnId || interrupting}
              className="flex items-center gap-1.5 rounded-full border border-coral/40 bg-coral/10 px-3 py-1.5 text-[11px] font-medium text-coral transition-colors hover:bg-coral/15 disabled:cursor-not-allowed disabled:opacity-50"
            >
              <HugeiconsIcon icon={Cancel01Icon} size={12} strokeWidth={2} />
              {interrupting
                ? "Interrupting…"
                : interruptibleTurnId
                  ? "Interrupt"
                  : "Starting…"}
            </button>
          ) : null}
        </header>

        {selectedSafety?.dangerous ? (
          <div className="mx-5 mt-3 flex items-start gap-2.5 rounded-[9px] border border-coral/35 bg-coral/10 px-3 py-2.5 text-[11px]">
            <HugeiconsIcon
              icon={Alert02Icon}
              size={14}
              strokeWidth={1.8}
              className="mt-px shrink-0 text-coral"
            />
            <p className="flex-1">
              <span className="font-semibold">{selectedSafety.label}</span>{" "}
              is marked dangerous by the installed {RUNNER_LABEL[runner]} CLI. {selectedSafety.description}
            </p>
            <button
              type="button"
              onClick={() => setSafetyId(safety?.defaultOptionId ?? null)}
              className="shrink-0 font-medium text-coral underline-offset-2 hover:underline"
            >
              Use safe default
            </button>
          </div>
        ) : null}

        <div
          ref={streamRef}
          className="min-h-0 flex-1 overflow-y-auto px-5"
          aria-live="polite"
        >
          <div className="mx-auto flex min-h-full w-full max-w-[820px] flex-col justify-end py-7">
            {loadingDetail ? (
              <TranscriptSkeleton />
            ) : turns.length === 0 && !runError ? (
              <EmptyChat cwd={cwd} runner={runner} safety={safety} />
            ) : (
              <div className="space-y-8">
                {turns.map((turn) => (
                  <TurnTranscript
                    key={turn.id}
                    turn={turn}
                    actionable={running && activeTurnId === turn.id}
                  />
                ))}
              </div>
            )}

            {runError ? <ErrorCard message={runError} /> : null}
            {running && liveEvents.length === 0 ? (
              <div className="mt-5 flex items-center gap-2 text-[12px] text-muted-foreground">
                <span className="size-1.5 animate-pulse rounded-full bg-blue" />
                Starting {RUNNER_LABEL[runner]}…
              </div>
            ) : null}
          </div>
        </div>

        <Composer
          value={value}
          onChange={setValue}
          onSend={send}
          running={composerBusy}
          canSend={canSend}
          locked={locked}
          bundleConfiguration={bundleConfiguration}
          bundles={bundles}
          selectedBundleId={
            selectedBundle?.bundle.id ?? sessionBundle?.snapshot.bundleId ?? null
          }
          activeBundleName={activeBundleName}
          bundleBusy={bundleBusy}
          onBundleChange={chooseBundle}
          runner={runner}
          onRunnerChange={setRunner}
          cwd={cwd}
          onCwdChange={setCwd}
          projects={projects}
          safety={safety}
          safetyId={safetyId}
          onSafetyChange={setSafetyId}
          allowRisky={allowRisky}
          catalogue={catalogue}
          model={model}
          onModelChange={(next) => {
            setModel(next);
            setEffort(next.defaultEffort);
          }}
          effort={effort}
          onEffortChange={setEffort}
        />
      </section>
    </div>
  );
}

function SessionSidebar({
  sessions,
  error,
  selectedId,
  running,
  onOpen,
  onNew,
  onRefresh,
}: {
  sessions: SessionSummary[] | null;
  error: string | null;
  selectedId: string | null;
  running: boolean;
  onOpen: (summary: SessionSummary) => void;
  onNew: () => void;
  onRefresh: () => Promise<void>;
}) {
  return (
    <aside className="flex w-[232px] shrink-0 flex-col border-r border-border bg-card/50" aria-label="Chat history">
      <div className="flex items-center gap-2 border-b border-border px-3 py-3">
        <button
          type="button"
          onClick={onNew}
          disabled={running}
          className="flex min-w-0 flex-1 items-center justify-center gap-1.5 rounded-[8px] bg-foreground px-3 py-2 text-[11px] font-semibold text-background transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-45"
        >
          <HugeiconsIcon icon={PlusSignIcon} size={13} strokeWidth={2} />
          New chat
        </button>
        <button
          type="button"
          onClick={() => void onRefresh()}
          aria-label="Refresh chat history"
          className="flex size-8 items-center justify-center rounded-[8px] border border-border text-muted-foreground transition-colors hover:bg-hover hover:text-foreground"
        >
          <HugeiconsIcon icon={RefreshIcon} size={13} strokeWidth={1.8} />
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        {sessions === null ? (
          <div className="space-y-1.5">
            {Array.from({ length: 5 }).map((_, index) => (
              <Skeleton key={index} className="h-[62px] rounded-[8px]" />
            ))}
          </div>
        ) : sessions.length === 0 ? (
          <p className="px-3 py-8 text-center text-[11px] leading-relaxed text-tertiary">
            Your durable chat history will appear here after the first turn.
          </p>
        ) : (
          <div className="space-y-1">
            {sessions.map((summary) => {
              const selected = summary.session.id === selectedId;
              return (
                <button
                  type="button"
                  key={summary.session.id}
                  onClick={() => onOpen(summary)}
                  disabled={running && !selected}
                  aria-current={selected ? "page" : undefined}
                  className={cn(
                    "w-full rounded-[8px] px-2.5 py-2 text-left transition-colors",
                    selected
                      ? "bg-selected text-foreground"
                      : "text-muted-foreground hover:bg-hover hover:text-foreground",
                    running && !selected && "cursor-not-allowed opacity-45",
                  )}
                >
                  <span className="flex items-center gap-2">
                    <LabMark runner={summary.session.runner} className="size-3" />
                    <span className="min-w-0 flex-1 truncate text-[11px] font-medium">
                      {summary.session.title}
                    </span>
                    <StatusDot status={summary.lastTurnStatus} />
                  </span>
                  <span className="mt-1.5 flex items-center gap-1.5 pl-5 text-[9px] text-tertiary">
                    <span>{formatTimestamp(summary.session.updatedAt)}</span>
                    <span aria-hidden>·</span>
                    <span>
                      {summary.turnCount} {summary.turnCount === 1 ? "turn" : "turns"}
                    </span>
                  </span>
                </button>
              );
            })}
          </div>
        )}
        {error ? (
          <div className="mt-2 rounded-[8px] border border-destructive/25 bg-destructive/5 px-2.5 py-2 text-[10px] leading-relaxed text-destructive">
            {error}
          </div>
        ) : null}
      </div>
    </aside>
  );
}

function StatusDot({ status }: { status: TurnStatus | null }) {
  if (!status) return null;
  return (
    <span
      className={cn(
        "size-1.5 shrink-0 rounded-full",
        status === "completed" && "bg-ok",
        (status === "queued" || status === "running") && "bg-blue",
        status === "interrupted" && "bg-warn",
        status === "failed" && "bg-destructive",
      )}
      title={statusLabel(status)}
    />
  );
}

function SessionMetadata({
  started,
  tokenUsage,
}: {
  started: Extract<SessionEvent, { kind: "started" }> | null;
  tokenUsage: Extract<SessionEvent, { kind: "token-usage" }> | null;
}) {
  const metadata: string[] = [];
  if (started?.model) metadata.push(started.model);
  if (started?.tools !== null && started?.tools !== undefined) {
    metadata.push(`${formatCount(started.tools)} tools`);
  }
  if (started?.mcpServers !== null && started?.mcpServers !== undefined) {
    metadata.push(`${formatCount(started.mcpServers)} MCP`);
  }
  if (tokenUsage?.totalTokens !== null && tokenUsage?.totalTokens !== undefined) {
    metadata.push(`${formatCount(tokenUsage.totalTokens)} tokens`);
  }
  if (metadata.length === 0) return null;
  return (
    <p
      className="hidden max-w-[360px] truncate font-mono text-[9px] text-tertiary xl:block"
      title="Reported by the runner"
    >
      {metadata.join(" · ")}
    </p>
  );
}

function EmptyChat({
  cwd,
  runner,
  safety,
}: {
  cwd: string;
  runner: Runner;
  safety: SafetyCapabilities | null;
}) {
  return (
    <div className="mx-auto flex max-w-[520px] flex-col items-center py-14 text-center">
      <AviaryMark />
      <h2 className="mt-4 text-[26px] font-semibold tracking-tight">
        What should we work on?
      </h2>
      <p className="mt-2 text-[12px] leading-relaxed text-muted-foreground">
        Aviary runs your installed {RUNNER_LABEL[runner]} CLI and stores a durable,
        normalized transcript. Prompts are sent over stdin, never command arguments.
      </p>
      {!cwd ? (
        <p className="mt-4 rounded-full border border-border bg-card px-3 py-1.5 text-[10px] font-medium text-tertiary">
          Choose a project or folder below to begin
        </p>
      ) : safety && !safety.available ? (
        <p className="mt-4 max-w-full rounded-[8px] border border-destructive/25 bg-destructive/5 px-3 py-2 text-[10px] text-destructive">
          {safety.warning ?? `${RUNNER_LABEL[runner]} is unavailable.`}
        </p>
      ) : null}
    </div>
  );
}

function TranscriptSkeleton() {
  return (
    <div className="space-y-5">
      <Skeleton className="ml-auto h-12 w-[48%] rounded-[16px]" />
      <Skeleton className="h-4 w-[78%] rounded" />
      <Skeleton className="h-4 w-[61%] rounded" />
      <Skeleton className="h-16 w-[54%] rounded-[10px]" />
    </div>
  );
}

function TurnTranscript({
  turn,
  actionable,
}: {
  turn: ViewTurn;
  actionable: boolean;
}) {
  const blocks = useMemo(() => buildTranscript(turn.events), [turn.events]);
  return (
    <article className="[content-visibility:auto]" aria-label={`Chat turn: ${statusLabel(turn.status)}`}>
      <div className="flex justify-end">
        <div className="max-w-[620px] whitespace-pre-wrap rounded-[17px] bg-hover px-4 py-3 text-[13px] leading-relaxed">
          {turn.prompt || "Prompt unavailable"}
        </div>
      </div>

      <div className="mt-4 space-y-3.5">
        {blocks.map((block) => {
          switch (block.kind) {
            case "text":
              return <AssistantMessage key={block.key} text={block.text} />;
            case "thinking":
              return <ThinkingBlock key={block.key} text={block.text} />;
            case "tool":
              return <StructuredToolCard key={block.key} tool={block} />;
            case "permission":
              return (
                <PermissionCard
                  key={block.key}
                  request={block.request}
                  decision={block.decision}
                  actionable={actionable && turn.status === "running"}
                  turnStatus={turn.status}
                />
              );
            case "failed":
              return (
                <ErrorCard
                  key={block.key}
                  message={block.message}
                  note={block.failure}
                />
              );
            case "interrupted":
              return (
                <div
                  key={block.key}
                  className="flex items-center gap-2 text-[11px] text-warn"
                >
                  <span className="size-1.5 rounded-full bg-warn" />
                  Turn interrupted
                </div>
              );
          }
        })}
        {turn.status === "queued" && turn.events.length === 0 ? (
          <div className="flex items-center gap-2 text-[11px] text-muted-foreground">
            <span className="size-1.5 animate-pulse rounded-full bg-blue" />
            Queued
          </div>
        ) : null}
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-1.5 text-[9px] text-tertiary">
        <span>{statusLabel(turn.status)}</span>
        {turn.durationMs !== null ? (
          <>
            <span aria-hidden>·</span>
            <span>{formatDuration(turn.durationMs)}</span>
          </>
        ) : null}
        {turn.requestedModel ? (
          <>
            <span aria-hidden>·</span>
            <span className="font-mono">{turn.requestedModel}</span>
          </>
        ) : null}
        {turn.requestedEffort ? (
          <>
            <span aria-hidden>·</span>
            <span>{turn.requestedEffort} effort</span>
          </>
        ) : null}
        {turn.permissionMode ? (
          <>
            <span aria-hidden>·</span>
            <span>{turn.permissionMode}</span>
          </>
        ) : null}
      </div>
    </article>
  );
}

function AssistantMessage({ text }: { text: string }) {
  return (
    <div className="max-w-[760px] text-[13px] leading-[1.7] text-foreground/90 [&_a]:text-blue [&_a]:underline [&_blockquote]:border-l-2 [&_blockquote]:border-border-strong [&_blockquote]:pl-3 [&_code]:rounded [&_code]:bg-inset [&_code]:px-1 [&_code]:py-0.5 [&_li]:my-1 [&_ol]:my-3 [&_ol]:pl-5 [&_p+p]:mt-3 [&_pre]:my-3 [&_pre]:overflow-x-auto [&_pre]:rounded-[9px] [&_pre]:bg-inset [&_pre]:p-3 [&_pre_code]:bg-transparent [&_pre_code]:p-0 [&_ul]:my-3 [&_ul]:pl-5">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
    </div>
  );
}

function ThinkingBlock({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="max-w-[720px] rounded-[9px] border border-border bg-card/60">
      <button
        type="button"
        onClick={() => setOpen((current) => !current)}
        aria-expanded={open}
        className="flex w-full items-center gap-2 px-3 py-2 text-left text-[11px] font-medium text-muted-foreground transition-colors hover:text-foreground"
      >
        <HugeiconsIcon icon={SparklesIcon} size={13} strokeWidth={1.5} />
        Reasoning
        <HugeiconsIcon
          icon={ArrowDown01Icon}
          size={12}
          strokeWidth={2}
          className={cn("ml-auto transition-transform", open && "rotate-180")}
        />
      </button>
      {open ? (
        <p className="whitespace-pre-wrap border-t border-border px-3 py-2.5 text-[11px] leading-relaxed text-muted-foreground">
          {text}
        </p>
      ) : null}
    </div>
  );
}

function prettyDetail(detail: string) {
  try {
    return JSON.stringify(JSON.parse(detail) as unknown, null, 2);
  } catch {
    return detail;
  }
}

function StructuredToolCard({ tool }: { tool: ToolBlock }) {
  const [open, setOpen] = useState(tool.status === "failed");
  const lowerName = tool.name.toLowerCase();
  const isCommand = lowerName.includes("command") || lowerName.includes("shell");
  const isFile = ["file", "read", "write", "edit", "patch", "diff"].some(
    (part) => lowerName.includes(part),
  );
  const detail = tool.detail ? prettyDetail(tool.detail) : null;
  const expandable = Boolean(detail);
  return (
    <div className="max-w-[740px] overflow-hidden rounded-[10px] border border-border bg-card/70">
      <button
        type="button"
        onClick={() => expandable && setOpen((current) => !current)}
        disabled={!expandable}
        aria-expanded={expandable ? open : undefined}
        className="flex w-full items-center gap-2.5 px-3 py-2.5 text-left disabled:cursor-default"
      >
        <span
          className={cn(
            "size-2 shrink-0 rounded-[3px]",
            tool.status === "running" && "animate-pulse bg-blue",
            tool.status === "succeeded" && "bg-ok",
            tool.status === "failed" && "bg-destructive",
          )}
        />
        <span className="shrink-0 text-[11px] font-semibold">{tool.name}</span>
        {tool.summary ? (
          <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-tertiary">
            {tool.summary}
          </span>
        ) : (
          <span className="flex-1" />
        )}
        <span className="text-[9px] font-medium text-tertiary">
          {tool.status === "running"
            ? "Running"
            : tool.status === "succeeded"
              ? "Done"
              : "Failed"}
        </span>
        {expandable ? (
          <HugeiconsIcon
            icon={ArrowDown01Icon}
            size={12}
            strokeWidth={2}
            className={cn("text-tertiary transition-transform", open && "rotate-180")}
          />
        ) : null}
      </button>
      {open && detail ? (
        <pre
          className={cn(
            "max-h-[360px] overflow-auto border-t border-border bg-inset px-3 py-2.5 font-mono text-[10px] leading-relaxed text-muted-foreground",
            isCommand && "text-foreground/80",
          )}
        >
          {isFile
            ? detail.split("\n").map((line, index) => (
                <span
                  key={`${index}-${line.slice(0, 16)}`}
                  className={cn(
                    "block min-h-[1em]",
                    line.startsWith("+") && !line.startsWith("+++") && "bg-ok/10 text-ok",
                    line.startsWith("-") && !line.startsWith("---") && "bg-destructive/10 text-destructive",
                  )}
                >
                  {line}
                </span>
              ))
            : detail}
        </pre>
      ) : null}
    </div>
  );
}

function PermissionCard({
  request,
  decision,
  actionable,
  turnStatus,
}: {
  request: PermissionRequestEvent;
  decision: string | null;
  actionable: boolean;
  turnStatus: TurnStatus;
}) {
  const [answers, setAnswers] = useState<Record<string, string[]>>({});
  const [otherAnswers, setOtherAnswers] = useState<Record<string, string>>({});
  const [sending, setSending] = useState<PermissionDecision | null>(null);
  const [expiredLocally, setExpiredLocally] = useState(false);
  const sendingRef = useRef(false);
  const questions = request.prompt?.kind === "questions" ? request.prompt.questions : [];
  const active = actionable && !decision && !expiredLocally;

  useEffect(() => {
    if (decision || !active) {
      sendingRef.current = false;
      setAnswers({});
      setOtherAnswers({});
      setSending(null);
    }
  }, [active, decision]);

  const answerQuestion = useCallback(
    (question: PermissionQuestion, answer: string, checked: boolean) => {
      setAnswers((current) => {
        const selected = new Set(current[question.id] ?? []);
        if (checked) selected.add(answer);
        else selected.delete(answer);
        return { ...current, [question.id]: [...selected] };
      });
    },
    [],
  );

  const complete = questions.every((question) => {
    const selected = answers[question.id] ?? [];
    const other = otherAnswers[question.id]?.trim() ?? "";
    return selected.length > 0 || other.length > 0;
  });

  const respond = useCallback(
    async (nextDecision: PermissionDecision) => {
      if (!active || sendingRef.current) return;
      sendingRef.current = true;
      setSending(nextDecision);
      const submittedAnswers =
        nextDecision === "submit"
          ? Object.fromEntries(
              questions.map((question) => {
                const values = [...(answers[question.id] ?? [])];
                const other = otherAnswers[question.id]?.trim();
                if (other) values.push(other);
                return [question.id, { answers: values }];
              }),
            )
          : undefined;
      try {
        await respondPermission(request.requestId, {
          decision: nextDecision,
          answers: submittedAnswers,
        });
        setAnswers({});
        setOtherAnswers({});
      } catch (error) {
        sendingRef.current = false;
        setSending(null);
        setExpiredLocally(true);
        notify("This request is no longer pending", {
          description: errorMessage(error),
        });
      }
    }, [
      active,
      answers,
      otherAnswers,
      questions,
      request.requestId,
    ],
  );

  const visibleState = decision
    ? decisionLabel(decision)
    : !active
      ? turnStatus === "running"
        ? "Expired after the live runner request detached"
        : "Expired when the live turn ended"
      : null;

  return (
    <section className="max-w-[740px] rounded-[11px] border border-gold/35 bg-gold/5 p-3" aria-label="Runner permission request">
      <div className="flex items-start gap-2.5">
        <span className="mt-1 size-2 shrink-0 rounded-full bg-gold" />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-[12px] font-semibold">{request.toolName}</h3>
            <span className="rounded-full border border-gold/25 px-1.5 py-0.5 text-[9px] font-medium text-gold">
              Permission
            </span>
          </div>
          {request.summary ? (
            <p className="mt-1 whitespace-pre-wrap font-mono text-[10px] leading-relaxed text-muted-foreground">
              {request.summary}
            </p>
          ) : null}
        </div>
      </div>

      {request.prompt?.kind === "unsupported" ? (
        <p className="mt-3 rounded-[7px] border border-border bg-card px-2.5 py-2 text-[10px] leading-relaxed text-muted-foreground">
          {request.prompt.message}
        </p>
      ) : null}

      {questions.length > 0 ? (
        <div className="mt-3 space-y-3 border-t border-gold/20 pt-3">
          {questions.map((question) => (
            <QuestionField
              key={question.id}
              question={question}
              selected={answers[question.id] ?? []}
              other={otherAnswers[question.id] ?? ""}
              disabled={!active || Boolean(sending)}
              onToggle={(answer, checked) =>
                answerQuestion(question, answer, checked)
              }
              onOther={(answer) =>
                setOtherAnswers((current) => ({
                  ...current,
                  [question.id]: answer,
                }))
              }
            />
          ))}
        </div>
      ) : null}

      {visibleState ? (
        <p className="mt-3 text-[10px] font-medium text-tertiary">{visibleState}</p>
      ) : (
        <div className="mt-3 flex flex-wrap justify-end gap-1.5">
          {request.options.map((action) => (
            <button
              type="button"
              key={action}
              onClick={() => void respond(action)}
              disabled={
                Boolean(sending) ||
                (action === "submit" && questions.length > 0 && !complete)
              }
              className={cn(
                "rounded-[7px] border px-2.5 py-1.5 text-[10px] font-semibold transition-colors disabled:cursor-not-allowed disabled:opacity-45",
                action === "allow-once" || action === "allow-session" || action === "submit"
                  ? "border-foreground bg-foreground text-background hover:opacity-90"
                  : action === "cancel"
                    ? "border-coral/35 text-coral hover:bg-coral/10"
                    : "border-border bg-card text-muted-foreground hover:text-foreground",
              )}
            >
              {sending === action ? "Sending…" : actionLabel(action)}
            </button>
          ))}
        </div>
      )}
    </section>
  );
}

function QuestionField({
  question,
  selected,
  other,
  disabled,
  onToggle,
  onOther,
}: {
  question: PermissionQuestion;
  selected: string[];
  other: string;
  disabled: boolean;
  onToggle: (answer: string, checked: boolean) => void;
  onOther: (answer: string) => void;
}) {
  const needsText = question.options.length === 0 || question.isOther;
  return (
    <fieldset disabled={disabled} className="space-y-2">
      <legend className="text-[11px] font-semibold">
        {question.header || question.question}
      </legend>
      {question.header && question.question ? (
        <p className="text-[10px] leading-relaxed text-muted-foreground">
          {question.question}
        </p>
      ) : null}
      {question.options.length > 0 ? (
        <div className="grid gap-1.5">
          {question.options.map((option) => {
            const checked = selected.includes(option.label);
            return (
              <label
                key={option.label}
                className="flex cursor-pointer items-start gap-2 rounded-[7px] border border-border bg-card px-2.5 py-2 has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-55"
              >
                <Checkbox
                  checked={checked}
                  onCheckedChange={(next) => onToggle(option.label, next === true)}
                  disabled={disabled}
                  aria-label={option.label}
                  className="mt-0.5"
                />
                <span className="min-w-0">
                  <span className="block text-[10px] font-medium">{option.label}</span>
                  {option.description ? (
                    <span className="mt-0.5 block text-[9px] leading-relaxed text-tertiary">
                      {option.description}
                    </span>
                  ) : null}
                </span>
              </label>
            );
          })}
        </div>
      ) : null}
      {needsText ? (
        <Input
          type={question.isSecret ? "password" : "text"}
          autoComplete="off"
          value={other}
          onChange={(event) => onOther(event.target.value)}
          disabled={disabled}
          aria-label={question.question || question.header}
          placeholder={
            question.isSecret
              ? "Enter a private answer"
              : question.isOther
                ? "Other answer…"
                : "Your answer…"
          }
          className="h-8 text-[11px]"
        />
      ) : null}
      {question.isSecret ? (
        <p className="text-[9px] text-tertiary">
          Hidden while typing and sent only to the active runner request.
        </p>
      ) : null}
    </fieldset>
  );
}

function ErrorCard({ message, note }: { message: string; note?: string }) {
  return (
    <div className="mt-4 flex max-w-[740px] items-start gap-2.5 rounded-[9px] border border-destructive/30 bg-destructive/5 px-3 py-2.5">
      <HugeiconsIcon
        icon={Alert02Icon}
        size={14}
        strokeWidth={1.8}
        className="mt-px shrink-0 text-destructive"
      />
      <div className="min-w-0">
        <p className="whitespace-pre-wrap text-[11px] leading-relaxed">{message}</p>
        {note ? <p className="mt-1 font-mono text-[9px] text-tertiary">{note}</p> : null}
      </div>
    </div>
  );
}

function Composer({
  value,
  onChange,
  onSend,
  running,
  canSend,
  locked,
  bundleConfiguration,
  bundles,
  selectedBundleId,
  activeBundleName,
  bundleBusy,
  onBundleChange,
  runner,
  onRunnerChange,
  cwd,
  onCwdChange,
  projects,
  safety,
  safetyId,
  onSafetyChange,
  allowRisky,
  catalogue,
  model,
  onModelChange,
  effort,
  onEffortChange,
}: {
  value: string;
  onChange: (value: string) => void;
  onSend: () => Promise<void>;
  running: boolean;
  canSend: boolean;
  locked: boolean;
  bundleConfiguration: boolean;
  bundles: ResolvedBundle[] | null;
  selectedBundleId: string | null;
  activeBundleName: string | null;
  bundleBusy: boolean;
  onBundleChange: (bundle: ResolvedBundle | null) => Promise<void>;
  runner: Runner;
  onRunnerChange: (runner: Runner) => void;
  cwd: string;
  onCwdChange: (cwd: string) => void;
  projects: Project[] | null;
  safety: SafetyCapabilities | null;
  safetyId: string | null;
  onSafetyChange: (id: string) => void;
  allowRisky: boolean;
  catalogue: ModelCatalogue | null;
  model: ModelOption;
  onModelChange: (model: ModelOption) => void;
  effort: string | null;
  onEffortChange: (effort: string) => void;
}) {
  return (
    <div className="border-t border-border bg-background px-5 py-4">
      <div className="mx-auto w-full max-w-[820px] rounded-[15px] border border-border-strong bg-card p-3 shadow-[0_10px_28px_-20px_rgba(0,0,0,0.7)] focus-within:ring-2 focus-within:ring-ring/25">
        <label htmlFor="chat-prompt" className="sr-only">
          Message {RUNNER_LABEL[runner]}
        </label>
        <textarea
          id="chat-prompt"
          value={value}
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              if (canSend) void onSend();
            }
          }}
          rows={2}
          disabled={running}
          placeholder={
            running
              ? `${RUNNER_LABEL[runner]} is working…`
              : cwd
                ? "Ask anything…"
                : "Choose a working directory to begin…"
          }
          className="max-h-40 min-h-[46px] w-full resize-none bg-transparent px-1 text-[13px] leading-relaxed outline-none placeholder:text-tertiary disabled:opacity-55"
        />

        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <BundlePicker
            bundles={bundles}
            selectedId={selectedBundleId}
            selectedName={activeBundleName}
            busy={bundleBusy}
            disabled={locked || running}
            onChange={onBundleChange}
          />
          <RunnerPicker
            runner={runner}
            onChange={onRunnerChange}
            disabled={locked || bundleConfiguration || running}
          />
          <CwdPicker
            cwd={cwd}
            projects={projects}
            onChange={onCwdChange}
            disabled={locked || bundleConfiguration || running}
          />
          <ModelPicker
            runner={runner}
            catalogue={catalogue}
            model={model}
            onChange={onModelChange}
            disabled={locked || bundleConfiguration || running}
          />
          {model.reasoningLevels.length > 1 && effort ? (
            <EffortPicker
              levels={model.reasoningLevels}
              effort={effort}
              onChange={onEffortChange}
              disabled={running}
            />
          ) : null}
          <SafetyPicker
            capabilities={safety}
            selectedId={safetyId}
            onChange={onSafetyChange}
            allowRisky={allowRisky}
            disabled={running}
          />
          <span className="flex-1" />
          <motion.button
            type="button"
            onClick={() => void onSend()}
            disabled={!canSend}
            whileTap={{ scale: canSend ? 0.92 : 1 }}
            aria-label="Send message"
            className="flex size-8 items-center justify-center rounded-full bg-foreground text-background transition-opacity disabled:cursor-not-allowed disabled:opacity-30"
          >
            <HugeiconsIcon icon={ArrowUp01Icon} size={16} strokeWidth={2} />
          </motion.button>
        </div>
      </div>
      <p className="mx-auto mt-2 max-w-[820px] text-center text-[9px] text-tertiary">
        Enter to send · Shift+Enter for a new line
        {locked
          ? " · Runner, model, and working directory are locked for this session"
          : bundleConfiguration
            ? " · Bundle controls runner, model, and working directory"
            : ""}
      </p>
    </div>
  );
}

function BundlePicker({
  bundles,
  selectedId,
  selectedName,
  busy,
  disabled,
  onChange,
}: {
  bundles: ResolvedBundle[] | null;
  selectedId: string | null;
  selectedName: string | null;
  busy: boolean;
  disabled: boolean;
  onChange: (bundle: ResolvedBundle | null) => Promise<void>;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        disabled={disabled || busy}
        title={
          selectedName
            ? `Attached bundle: ${selectedName}`
            : "Attach a saved bundle"
        }
        className={cn(
          "flex max-w-[190px] items-center gap-1.5 rounded-full border px-2.5 py-1.5 text-[10px] font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-55",
          selectedName
            ? "border-violet/30 bg-violet/10 text-violet"
            : "border-border bg-card text-muted-foreground hover:text-foreground",
        )}
      >
        <HugeiconsIcon icon={PackageIcon} size={12} strokeWidth={1.7} />
        <span className="truncate">
          {busy ? "Checking bundle…" : selectedName ?? "Attach bundle"}
        </span>
        <HugeiconsIcon icon={ArrowDown01Icon} size={11} strokeWidth={2} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-[340px]">
        <DropdownMenuGroup>
          <DropdownMenuLabel>Saved bundles</DropdownMenuLabel>
        </DropdownMenuGroup>
        {selectedId ? (
          <>
            <DropdownMenuItem onClick={() => void onChange(null)}>
              Continue without a bundle
            </DropdownMenuItem>
            <DropdownMenuSeparator />
          </>
        ) : null}
        {bundles === null ? (
          <DropdownMenuItem disabled>Loading bundles…</DropdownMenuItem>
        ) : bundles.length === 0 ? (
          <p className="px-2 py-2 text-[10px] leading-relaxed text-tertiary">
            No saved bundles. Create one from the Bundles screen first.
          </p>
        ) : (
          bundles.map((row) => {
            const unavailable = row.members.filter(
              ({ resolution }) => resolution.status !== "ready",
            ).length;
            const projectCount = row.bundle.members.filter(
              (member) => member.kind === "project",
            ).length;
            const selected = row.bundle.id === selectedId;
            return (
              <DropdownMenuItem
                key={row.bundle.id}
                disabled={unavailable > 0 || projectCount !== 1 || selected}
                onClick={() => void onChange(row)}
              >
                <span className="min-w-0 flex-1">
                  <span className="flex items-center gap-2">
                    <span className="truncate text-[11px] font-medium">
                      {row.bundle.name}
                    </span>
                    {selected ? (
                      <span className="text-[8px] font-semibold text-violet">
                        attached
                      </span>
                    ) : null}
                  </span>
                  <span className="mt-0.5 block truncate text-[9px] text-tertiary">
                    {RUNNER_LABEL[row.bundle.runner]} · {row.bundle.members.length}{" "}
                    {row.bundle.members.length === 1 ? "member" : "members"}
                    {unavailable > 0
                      ? ` · ${unavailable} need attention`
                      : projectCount !== 1
                        ? " · needs exactly one project"
                      : ""}
                  </span>
                </span>
              </DropdownMenuItem>
            );
          })
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function RunnerPicker({
  runner,
  onChange,
  disabled,
}: {
  runner: Runner;
  onChange: (runner: Runner) => void;
  disabled: boolean;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        disabled={disabled}
        className="flex items-center gap-1.5 rounded-full bg-hover px-2.5 py-1.5 text-[10px] font-medium text-muted-foreground transition-colors hover:text-foreground disabled:cursor-not-allowed disabled:opacity-55"
      >
        <LabMark runner={runner} className="size-3" />
        {RUNNER_LABEL[runner]}
        <HugeiconsIcon icon={ArrowDown01Icon} size={11} strokeWidth={2} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-44">
        <DropdownMenuRadioGroup
          value={runner}
          onValueChange={(next) => onChange(next as Runner)}
        >
          <DropdownMenuLabel>Runner</DropdownMenuLabel>
          <DropdownMenuRadioItem value="claude-code">Claude Code</DropdownMenuRadioItem>
          <DropdownMenuRadioItem value="codex">Codex</DropdownMenuRadioItem>
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function CwdPicker({
  cwd,
  projects,
  onChange,
  disabled,
}: {
  cwd: string;
  projects: Project[] | null;
  onChange: (cwd: string) => void;
  disabled: boolean;
}) {
  const browse = useCallback(async () => {
    try {
      const chosen = await openDialog({ directory: true, multiple: false });
      if (typeof chosen === "string") onChange(chosen);
    } catch (error) {
      notify("Could not open the folder picker", {
        description: errorMessage(error),
      });
    }
  }, [onChange]);
  const selectedProject = projects?.find((project) => project.path === cwd);
  const cwdParts = cwd.split("/").filter(Boolean);
  const label =
    selectedProject?.name ?? (cwd ? cwdParts[cwdParts.length - 1] : null);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        disabled={disabled}
        title={cwd || "Choose a registered project or folder"}
        className={cn(
          "flex max-w-[210px] items-center gap-1.5 rounded-full bg-hover px-2.5 py-1.5 text-[10px] font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-55",
          cwd ? "text-muted-foreground hover:text-foreground" : "text-warn",
        )}
      >
        <HugeiconsIcon icon={Folder01Icon} size={12} strokeWidth={1.7} />
        <span className="truncate">{label ?? "Choose folder"}</span>
        <HugeiconsIcon icon={ArrowDown01Icon} size={11} strokeWidth={2} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-[340px]">
        <DropdownMenuGroup>
          <DropdownMenuLabel>Working directory</DropdownMenuLabel>
        </DropdownMenuGroup>
        {projects === null ? (
          <DropdownMenuItem disabled>Loading projects…</DropdownMenuItem>
        ) : projects.length > 0 ? (
          <DropdownMenuRadioGroup value={cwd} onValueChange={onChange}>
            {projects.map((project) => (
              <DropdownMenuRadioItem key={project.path} value={project.path}>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[11px] font-medium">{project.name}</span>
                  <span className="mt-0.5 block truncate font-mono text-[9px] text-tertiary">
                    {project.path}
                  </span>
                </span>
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        ) : (
          <p className="px-2 py-2 text-[10px] leading-relaxed text-tertiary">
            No registered projects. Choose a folder directly.
          </p>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem onClick={() => void browse()}>
          <HugeiconsIcon icon={Folder01Icon} size={13} strokeWidth={1.7} />
          Browse for folder…
        </DropdownMenuItem>
        {cwd ? (
          <p className="break-all border-t border-border px-2 py-2 font-mono text-[9px] leading-relaxed text-tertiary">
            {cwd}
          </p>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function SafetyPicker({
  capabilities,
  selectedId,
  onChange,
  allowRisky,
  disabled,
}: {
  capabilities: SafetyCapabilities | null;
  selectedId: string | null;
  onChange: (id: string) => void;
  allowRisky: boolean;
  disabled: boolean;
}) {
  const selected = capabilities?.options.find((option) => option.id === selectedId);
  const options =
    capabilities?.options.filter(
      (option) => allowRisky || !option.dangerous || option.id === selectedId,
    ) ?? [];
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        disabled={disabled || !capabilities?.available}
        className={cn(
          "flex items-center gap-1.5 rounded-full px-2.5 py-1.5 font-mono text-[10px] font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-55",
          selected?.dangerous
            ? "bg-coral/15 text-coral"
            : "bg-hover text-muted-foreground hover:text-foreground",
        )}
      >
        <HugeiconsIcon icon={SparklesIcon} size={12} strokeWidth={1.5} />
        {selected?.label ?? (capabilities ? "Safety unavailable" : "Discovering safety…")}
        <HugeiconsIcon icon={ArrowDown01Icon} size={11} strokeWidth={2} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-[350px]">
        <DropdownMenuRadioGroup value={selectedId ?? ""} onValueChange={onChange}>
          <DropdownMenuLabel>Installed runner safety</DropdownMenuLabel>
          {options.map((option) => (
            <SafetyMenuItem key={option.id} option={option} />
          ))}
        </DropdownMenuRadioGroup>
        {capabilities?.warning ? (
          <p className="border-t border-border px-2 py-2 text-[9px] leading-relaxed text-warn">
            {capabilities.warning}
          </p>
        ) : null}
        {capabilities ? (
          <p className="border-t border-border px-2 py-2 font-mono text-[9px] text-tertiary">
            {capabilities.protocol}
          </p>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function SafetyMenuItem({ option }: { option: SafetyOption }) {
  return (
    <DropdownMenuRadioItem value={option.id}>
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-2">
          <span className="text-[11px] font-medium">{option.label}</span>
          {option.dangerous ? (
            <span className="rounded-full bg-coral/15 px-1.5 py-0.5 text-[8px] font-semibold text-coral">
              dangerous
            </span>
          ) : null}
          {option.interactiveApprovals ? (
            <span className="rounded-full bg-blue/10 px-1.5 py-0.5 text-[8px] font-semibold text-blue">
              asks here
            </span>
          ) : null}
        </span>
        <span className="mt-0.5 block text-[9px] leading-relaxed text-tertiary">
          {option.description}
        </span>
      </span>
    </DropdownMenuRadioItem>
  );
}

function ModelPicker({
  runner,
  catalogue,
  model,
  onChange,
  disabled,
}: {
  runner: Runner;
  catalogue: ModelCatalogue | null;
  model: ModelOption;
  onChange: (model: ModelOption) => void;
  disabled: boolean;
}) {
  const [custom, setCustom] = useState("");
  const models = catalogue?.models ?? [];
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        disabled={disabled || !catalogue}
        className="flex max-w-[180px] items-center gap-1.5 rounded-full bg-hover px-2.5 py-1.5 text-[10px] font-medium text-muted-foreground transition-colors hover:text-foreground disabled:cursor-not-allowed disabled:opacity-55"
      >
        <LabMark runner={runner} className="size-3" />
        <span className="truncate">{catalogue ? model.label : "Discovering models…"}</span>
        <HugeiconsIcon icon={ArrowDown01Icon} size={11} strokeWidth={2} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-[330px]">
        <DropdownMenuRadioGroup
          value={model.id ?? "__default"}
          onValueChange={(nextId) => {
            const next = models.find(
              (candidate) => (candidate.id ?? "__default") === nextId,
            );
            if (next) onChange(next);
          }}
        >
          <DropdownMenuLabel className="flex items-center gap-2">
            <LabMark runner={runner} className="size-3" />
            {RUNNER_LAB[runner]}
          </DropdownMenuLabel>
          {models.map((candidate) => (
            <DropdownMenuRadioItem
              key={candidate.id ?? "__default"}
              value={candidate.id ?? "__default"}
            >
              <span className="min-w-0 flex-1">
                <span className="flex items-center gap-2">
                  <span className="text-[11px] font-medium">{candidate.label}</span>
                  {candidate.isAlias ? (
                    <span className="rounded-full bg-teal/15 px-1.5 py-0.5 text-[8px] font-semibold text-teal">
                      latest
                    </span>
                  ) : null}
                </span>
                {candidate.note ? (
                  <span className="mt-0.5 block text-[9px] leading-relaxed text-tertiary">
                    {candidate.note}
                  </span>
                ) : null}
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <div className="border-t border-border px-2 py-2">
          <Input
            value={custom}
            onChange={(event) => setCustom(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && custom.trim()) {
                const id = custom.trim();
                onChange({
                  id,
                  label: id,
                  note: "Entered manually",
                  isAlias: false,
                  reasoningLevels: [],
                  defaultEffort: null,
                });
                setCustom("");
              }
            }}
            placeholder="Or type a model id…"
            className="h-7 font-mono text-[10px]"
          />
          {catalogue ? (
            <p className="mt-1.5 text-[9px] leading-relaxed text-tertiary">
              {catalogue.source}
            </p>
          ) : null}
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function EffortPicker({
  levels,
  effort,
  onChange,
  disabled,
}: {
  levels: ReasoningLevel[];
  effort: string;
  onChange: (effort: string) => void;
  disabled: boolean;
}) {
  const index = Math.max(0, levels.findIndex((level) => level.effort === effort));
  const pct = levels.length > 1 ? index / (levels.length - 1) : 0;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        disabled={disabled}
        className="flex items-center gap-2 rounded-full bg-hover px-2.5 py-1.5 text-[10px] font-medium text-muted-foreground transition-colors hover:text-foreground disabled:cursor-not-allowed disabled:opacity-55"
      >
        <span className="relative h-1.5 w-6 overflow-hidden rounded-full bg-border-strong">
          <span
            className="absolute inset-y-0 left-0 rounded-full bg-violet"
            style={{ width: `${Math.max(12, pct * 100)}%` }}
          />
        </span>
        <span className="capitalize">{effort}</span>
        <HugeiconsIcon icon={ArrowDown01Icon} size={11} strokeWidth={2} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-[300px] p-3">
        <p className="mb-2.5 text-[9px] font-semibold tracking-[0.08em] text-tertiary">
          REASONING EFFORT
        </p>
        <EffortSlider levels={levels} value={effort} onChange={onChange} />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function AviaryMark() {
  const petal =
    "M64 59C58.9 54.3 57.4 46.5 60.2 34.3L64 14C67.8 29.2 70.2 43.2 68.7 49.5C67.8 53.7 66.1 57 64 59Z";
  return (
    <svg viewBox="0 0 128 128" className="size-[38px] text-violet" aria-hidden>
      {[0, 60, 120, 180, 240, 300].map((angle) => (
        <path
          key={angle}
          d={petal}
          fill="currentColor"
          transform={`rotate(${angle} 64 64)`}
        />
      ))}
      <circle cx="64" cy="64" r="7.2" fill="currentColor" />
    </svg>
  );
}
