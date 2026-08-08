import { useCallback, useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Add01Icon,
  ArrowDown01Icon,
  ArrowUp01Icon,
  Delete02Icon,
  PackageIcon,
  SaveIcon,
  Search01Icon,
  TerminalIcon,
} from "@hugeicons/core-free-icons";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  NativeSelect,
  NativeSelectOption,
} from "@/components/ui/native-select";
import { Skeleton } from "@/components/ui/skeleton";
import { Textarea } from "@/components/ui/textarea";
import { PageHeader, Segmented, StatusDot } from "@/components/screen-parts";
import {
  createBundle,
  deleteBundle,
  listBundles,
  listCollections,
  launchBundleTerminal,
  listModels,
  scanMcp,
  updateBundle,
  RUNNER_LABEL,
  type BundleDraft,
  type BundleMemberDraft,
  type BundleMemberKind,
  type BundleMemberRole,
  type BundleMemberTarget,
  type BundleTargetResolution,
  type McpSnapshot,
  type MediaCollection,
  type ModelCatalogue,
  type ResolvedBundle,
  type Runner,
} from "@/lib/api";
import { useLibrary } from "@/lib/use-library";
import { notify } from "@/lib/notify";
import { cn } from "@/lib/utils";

const PICKER_FILTERS = [
  "All",
  "Projects",
  "Skills",
  "Prompts",
  "Agents",
  "Memory",
  "MCP",
  "Media",
] as const;
type PickerFilter = (typeof PICKER_FILTERS)[number];

const KIND_FILTER: Partial<Record<PickerFilter, BundleMemberKind>> = {
  Projects: "project",
  Skills: "skill",
  Prompts: "prompt",
  Agents: "agent",
  Memory: "memory",
  MCP: "mcp",
  Media: "media-collection",
};

const KIND_LABEL: Record<BundleMemberKind, string> = {
  project: "Project",
  skill: "Skill",
  prompt: "Prompt",
  agent: "Agent",
  memory: "Memory",
  mcp: "MCP",
  "media-collection": "Media",
};

const ROLE_LABEL: Record<BundleMemberRole, string> = {
  "working-directory": "Working directory",
  available: "Available",
  "invoke-first-turn": "Invoke on first turn",
  prefill: "Prefill",
  primary: "Primary",
  supplement: "Supplement",
  enabled: "Enabled",
  retrieval: "Retrieval",
};

const ROLE_OPTIONS: Record<BundleMemberKind, BundleMemberRole[]> = {
  project: ["working-directory"],
  skill: ["available", "invoke-first-turn"],
  prompt: ["prefill"],
  agent: ["available", "primary"],
  memory: ["supplement"],
  mcp: ["enabled"],
  "media-collection": ["retrieval"],
};

type TargetOption = {
  key: string;
  kind: BundleMemberKind;
  label: string;
  detail: string;
  target: BundleMemberTarget;
  defaultRole: BundleMemberRole;
};

function blankDraft(runner: Runner = "claude-code"): BundleDraft {
  return {
    name: "",
    description: "",
    runner,
    modelId: null,
    memoryMode: "inherit",
    members: [],
  };
}

function toDraft(row: ResolvedBundle): BundleDraft {
  return {
    name: row.bundle.name,
    description: row.bundle.description,
    runner: row.bundle.runner,
    modelId: row.bundle.modelId,
    memoryMode: row.bundle.memoryMode,
    members: row.bundle.members.map((member, ordinal) => ({
      id: member.id,
      ordinal,
      kind: member.kind,
      role: member.role,
      target: member.target,
    })),
  };
}

function canonicalDraft(draft: BundleDraft) {
  return JSON.stringify({
    ...draft,
    members: draft.members.map((member, ordinal) => ({ ...member, ordinal })),
  });
}

function targetKey(kind: BundleMemberKind, target: BundleMemberTarget) {
  switch (target.type) {
    case "project":
      return `${kind}:project:${target.path}`;
    case "entry":
      return `${kind}:entry:${target.id}`;
    case "mcp-declaration":
      return `${kind}:mcp:${target.id}`;
    case "media-collection":
      return `${kind}:media:${target.id}`;
  }
}

function tilde(path: string) {
  return path.replace(/^\/Users\/[^/]+/, "~");
}

export function BundlesView() {
  const { data: library, loading: libraryLoading } = useLibrary();
  const [bundles, setBundles] = useState<ResolvedBundle[] | null>(null);
  const [mcp, setMcp] = useState<McpSnapshot | null>(null);
  const [collections, setCollections] = useState<MediaCollection[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [revision, setRevision] = useState<number | null>(null);
  const [draft, setDraft] = useState<BundleDraft>(() => blankDraft());
  const [baseline, setBaseline] = useState(() => canonicalDraft(blankDraft()));
  const [resolutions, setResolutions] = useState<
    Map<string, BundleTargetResolution>
  >(new Map());
  const [catalogue, setCatalogue] = useState<ModelCatalogue | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [launching, setLaunching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pickerOpen, setPickerOpen] = useState(false);

  const selectBundle = useCallback((row: ResolvedBundle) => {
    const next = toDraft(row);
    setSelectedId(row.bundle.id);
    setRevision(row.bundle.revision);
    setDraft(next);
    setBaseline(canonicalDraft(next));
    setResolutions(
      new Map(
        row.members.map(({ member, resolution }) => [member.id, resolution]),
      ),
    );
    setError(null);
  }, []);

  const beginNew = useCallback((runner: Runner = "claude-code") => {
    const next = blankDraft(runner);
    setSelectedId(null);
    setRevision(null);
    setDraft(next);
    setBaseline(canonicalDraft(next));
    setResolutions(new Map());
    setError(null);
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [nextBundles, nextMcp, nextCollections] = await Promise.all([
        listBundles(),
        scanMcp(),
        listCollections(),
      ]);
      setBundles(nextBundles);
      setMcp(nextMcp);
      setCollections(nextCollections);
      if (nextBundles.length > 0) selectBundle(nextBundles[0]);
      else beginNew();
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      setBundles([]);
      setError(message);
      notify("Could not load bundles", { description: message });
    } finally {
      setLoading(false);
    }
  }, [beginNew, selectBundle]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    let live = true;
    setCatalogue(null);
    void listModels(draft.runner)
      .then((models) => {
        if (live) setCatalogue(models);
      })
      .catch((reason) => {
        if (live) {
          notify(`Could not discover ${RUNNER_LABEL[draft.runner]} models`, {
            description: String(reason),
          });
        }
      });
    return () => {
      live = false;
    };
  }, [draft.runner]);

  const dirty = canonicalDraft(draft) !== baseline;

  const options = useMemo<TargetOption[]>(() => {
    const rows: TargetOption[] = [];
    for (const project of library?.projects ?? []) {
      rows.push({
        key: `project:${project.path}`,
        kind: "project",
        label: project.name,
        detail: tilde(project.path),
        target: { type: "project", path: project.path },
        defaultRole: "working-directory",
      });
    }
    for (const entry of library?.entries ?? []) {
      if (!entry.runners.includes(draft.runner)) continue;
      const kind: BundleMemberKind | null =
        entry.kind === "skill"
          ? "skill"
          : entry.kind === "agent"
            ? "agent"
            : entry.kind === "prompt" || entry.kind === "command"
              ? "prompt"
              : entry.kind === "memory"
                ? "memory"
                : null;
      if (!kind) continue;
      rows.push({
        key: `${kind}:${entry.id}`,
        kind,
        label: entry.name,
        detail: entry.description || tilde(entry.path),
        target: { type: "entry", id: entry.id },
        defaultRole:
          kind === "prompt"
            ? "prefill"
            : kind === "memory"
              ? "supplement"
              : "available",
      });
    }
    for (const declaration of mcp?.declarations ?? []) {
      if (declaration.runner !== draft.runner || declaration.state === "invalid") {
        continue;
      }
      rows.push({
        key: `mcp:${declaration.id}`,
        kind: "mcp",
        label: declaration.name,
        detail: `${declaration.source}, effective as ${declaration.effectiveName}`,
        target: { type: "mcp-declaration", id: declaration.id },
        defaultRole: "enabled",
      });
    }
    for (const collection of collections) {
      rows.push({
        key: `media:${collection.id}`,
        kind: "media-collection",
        label: collection.name,
        detail: `${collection.count} ${collection.count === 1 ? "reference" : "references"}`,
        target: { type: "media-collection", id: collection.id },
        defaultRole: "retrieval",
      });
    }
    return rows.sort((left, right) =>
      left.kind.localeCompare(right.kind) || left.label.localeCompare(right.label),
    );
  }, [collections, draft.runner, library, mcp]);

  const optionByTarget = useMemo(
    () =>
      new Map(
        options.map((option) => [targetKey(option.kind, option.target), option]),
      ),
    [options],
  );

  const choose = (row: ResolvedBundle) => {
    if (dirty && !window.confirm("Discard unsaved bundle changes?")) return;
    selectBundle(row);
  };

  const newBundle = () => {
    if (dirty && !window.confirm("Discard unsaved bundle changes?")) return;
    beginNew(draft.runner);
  };

  const save = async () => {
    if (!draft.name.trim()) {
      setError("A bundle name is required.");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const saved =
        selectedId && revision !== null
          ? await updateBundle(selectedId, revision, draft)
          : await createBundle(draft);
      setBundles((current) => {
        const rest = (current ?? []).filter(
          (row) => row.bundle.id !== saved.bundle.id,
        );
        return [saved, ...rest];
      });
      selectBundle(saved);
      notify(selectedId ? "Bundle saved" : "Bundle created");
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      setError(message);
      notify("Could not save bundle", { description: message });
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    if (!selectedId || revision === null) return;
    if (!window.confirm(`Delete ${draft.name || "this bundle"}?`)) return;
    setSaving(true);
    try {
      await deleteBundle(selectedId, revision);
      const remaining = (bundles ?? []).filter(
        (row) => row.bundle.id !== selectedId,
      );
      setBundles(remaining);
      if (remaining[0]) selectBundle(remaining[0]);
      else beginNew(draft.runner);
      notify("Bundle deleted");
    } catch (reason) {
      notify("Could not delete bundle", { description: String(reason) });
    } finally {
      setSaving(false);
    }
  };

  const launch = async () => {
    if (!selectedId || revision === null || dirty || unavailableCount > 0) return;
    setLaunching(true);
    setError(null);
    try {
      await launchBundleTerminal(selectedId, revision);
      notify("Opened bundle in Terminal", {
        description: "The saved revision was resolved into a private one-use handoff.",
      });
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      setError(message);
      notify("Could not launch bundle", { description: message });
    } finally {
      setLaunching(false);
    }
  };

  const addMember = (option: TargetOption) => {
    const duplicate = draft.members.some(
      (member) =>
        targetKey(member.kind, member.target) ===
        targetKey(option.kind, option.target),
    );
    if (duplicate) return;
    if (
      (option.kind === "project" || option.kind === "prompt") &&
      draft.members.some((member) => member.kind === option.kind)
    ) {
      notify(`A bundle can contain one ${KIND_LABEL[option.kind].toLowerCase()}`);
      return;
    }
    setDraft((current) => ({
      ...current,
      memoryMode:
        option.kind === "memory" ? "supplement" : current.memoryMode,
      members: [
        ...current.members,
        {
          id: null,
          ordinal: current.members.length,
          kind: option.kind,
          role: option.defaultRole,
          target: option.target,
        },
      ],
    }));
    setPickerOpen(false);
  };

  const editMember = (index: number, patch: Partial<BundleMemberDraft>) => {
    setDraft((current) => ({
      ...current,
      members: current.members.map((member, memberIndex) =>
        memberIndex === index ? { ...member, ...patch } : member,
      ),
    }));
  };

  const moveMember = (index: number, direction: -1 | 1) => {
    setDraft((current) => {
      const target = index + direction;
      if (target < 0 || target >= current.members.length) return current;
      const members = [...current.members];
      [members[index], members[target]] = [members[target], members[index]];
      return {
        ...current,
        members: members.map((member, ordinal) => ({ ...member, ordinal })),
      };
    });
  };

  const removeMember = (index: number) => {
    setDraft((current) => ({
      ...current,
      members: current.members
        .filter((_, memberIndex) => memberIndex !== index)
        .map((member, ordinal) => ({ ...member, ordinal })),
    }));
  };

  const selected = bundles?.find((row) => row.bundle.id === selectedId) ?? null;
  const unavailableCount = selected?.members.filter(
    ({ resolution }) => resolution.status !== "ready",
  ).length ?? 0;
  const selectedProjectCount = selected?.bundle.members.filter(
    (member) => member.kind === "project",
  ).length ?? 0;
  const executionIssue = selected
    ? unavailableCount > 0
      ? "Resolve unavailable members before launching"
      : selectedProjectCount !== 1
        ? "Add exactly one project before launching"
        : null
    : null;

  return (
    <div className="flex h-full min-h-0 gap-4 p-[26px]">
      <aside className="flex w-[230px] shrink-0 flex-col rounded-[14px] border border-border bg-card/60">
        <div className="flex items-center gap-2 border-b border-border p-3">
          <span className="min-w-0 flex-1 text-xs font-semibold">Saved bundles</span>
          <Button size="icon-sm" variant="outline" aria-label="New bundle" onClick={newBundle}>
            <HugeiconsIcon icon={Add01Icon} size={13} strokeWidth={2} />
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          {loading && bundles === null ? (
            <div className="space-y-1.5">
              {Array.from({ length: 5 }, (_, index) => (
                <Skeleton key={index} className="h-[62px] rounded-[9px]" />
              ))}
            </div>
          ) : bundles?.length ? (
            <div className="space-y-1">
              {bundles.map((row) => {
                const selected = row.bundle.id === selectedId;
                const missing = row.members.filter(
                  ({ resolution }) => resolution.status !== "ready",
                ).length;
                const projectCount = row.bundle.members.filter(
                  (member) => member.kind === "project",
                ).length;
                return (
                  <button
                    type="button"
                    key={row.bundle.id}
                    className={cn(
                      "w-full rounded-[9px] px-2.5 py-2 text-left transition-colors",
                      selected
                        ? "bg-selected text-foreground"
                        : "text-muted-foreground hover:bg-hover hover:text-foreground",
                    )}
                    onClick={() => choose(row)}
                  >
                    <span className="flex items-center gap-2">
                      <HugeiconsIcon icon={PackageIcon} size={13} strokeWidth={1.6} />
                      <span className="min-w-0 flex-1 truncate text-[11px] font-medium">
                        {row.bundle.name}
                      </span>
                      {missing > 0 || projectCount !== 1 ? (
                        <StatusDot status="warn" />
                      ) : null}
                    </span>
                    <span className="mt-1.5 block pl-5 text-[9px] text-tertiary">
                      {RUNNER_LABEL[row.bundle.runner]}, {row.bundle.members.length} members
                    </span>
                  </button>
                );
              })}
            </div>
          ) : (
            <div className="px-3 py-10 text-center">
              <p className="text-[11px] font-medium">No saved bundles</p>
              <p className="mt-1 text-[10px] leading-relaxed text-tertiary">
                Compose one from the real entries already indexed by Aviary.
              </p>
            </div>
          )}
        </div>
      </aside>

      <main className="min-w-0 flex-1 overflow-y-auto pr-1">
        <div className="flex flex-col gap-[18px]">
          <PageHeader
            title={selectedId ? draft.name || "Untitled bundle" : "New bundle"}
            subtitle={
              selectedId
                ? `Revision ${revision}, ${draft.members.length} members${
                    unavailableCount
                      ? `, ${unavailableCount} need attention`
                      : selectedProjectCount !== 1
                        ? ", needs exactly one project"
                        : ""
                  }`
                : "A saved composition for chat and terminal sessions"
            }
            action={
              <div className="flex items-center gap-2">
                {selectedId ? (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={
                      saving ||
                      launching ||
                      dirty ||
                      executionIssue !== null ||
                      revision === null
                    }
                    title={
                      dirty
                        ? "Save this revision before launching"
                        : executionIssue ?? "Launch this saved revision in Terminal"
                    }
                    onClick={() => void launch()}
                  >
                    <HugeiconsIcon icon={TerminalIcon} size={14} strokeWidth={1.8} />
                    {launching ? "Opening…" : "Terminal"}
                  </Button>
                ) : null}
                {selectedId ? (
                  <Button
                    size="icon-sm"
                    variant="outline"
                    aria-label="Delete bundle"
                    title="Delete bundle"
                    disabled={saving}
                    onClick={() => void remove()}
                  >
                    <HugeiconsIcon icon={Delete02Icon} size={14} strokeWidth={1.8} />
                  </Button>
                ) : null}
                <Button size="sm" disabled={saving || !dirty} onClick={() => void save()}>
                  <HugeiconsIcon icon={SaveIcon} size={14} strokeWidth={1.8} />
                  {saving ? "Saving…" : "Save"}
                </Button>
              </div>
            }
          />

          {error ? (
            <div className="rounded-[10px] border border-destructive/30 bg-destructive/10 px-3.5 py-3 text-xs">
              {error}
            </div>
          ) : null}

          <section className="grid grid-cols-2 gap-3 rounded-[14px] border border-border bg-card p-4">
            <Field label="Name" className="col-span-2">
              <Input
                value={draft.name}
                maxLength={120}
                onChange={(event) =>
                  setDraft((current) => ({ ...current, name: event.target.value }))
                }
                placeholder="Bundle name"
              />
            </Field>
            <Field label="Description" className="col-span-2">
              <Textarea
                value={draft.description}
                maxLength={4_000}
                rows={2}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    description: event.target.value,
                  }))
                }
                placeholder="What this composition is for"
              />
            </Field>
            <Field label="Runner">
              <NativeSelect
                className="w-full"
                value={draft.runner}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    runner: event.target.value as Runner,
                    modelId: null,
                  }))
                }
              >
                <NativeSelectOption value="claude-code">Claude Code</NativeSelectOption>
                <NativeSelectOption value="codex">Codex</NativeSelectOption>
              </NativeSelect>
            </Field>
            <Field label="Model ID" hint={catalogue?.source ?? "Discovering from the CLI"}>
              <Input
                value={draft.modelId ?? ""}
                maxLength={256}
                list={`bundle-models-${draft.runner}`}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    modelId: event.target.value || null,
                  }))
                }
                placeholder="Runner default"
              />
              <datalist id={`bundle-models-${draft.runner}`}>
                {catalogue?.models
                  .filter((model) => model.id !== null)
                  .map((model) => (
                    <option key={model.id} value={model.id ?? ""}>
                      {model.label}
                    </option>
                  ))}
              </datalist>
            </Field>
            <Field label="Memory policy" className="col-span-2">
              <NativeSelect
                className="w-full"
                value={draft.memoryMode}
                onChange={(event) =>
                  setDraft((current) => ({
                    ...current,
                    memoryMode: event.target.value as BundleDraft["memoryMode"],
                  }))
                }
              >
                <NativeSelectOption value="inherit">Inherit runner memory</NativeSelectOption>
                <NativeSelectOption value="supplement">
                  Inherit and add selected memory
                </NativeSelectOption>
              </NativeSelect>
            </Field>
          </section>

          <section className="space-y-2">
            <div className="flex items-center justify-between">
              <div>
                <h2 className="text-[13px] font-semibold">Composition</h2>
                <p className="mt-0.5 text-[10px] text-tertiary">
                  Order is saved. Missing targets keep their original identity and label.
                </p>
              </div>
              <Button
                size="sm"
                variant="outline"
                disabled={libraryLoading || loading}
                onClick={() => setPickerOpen(true)}
              >
                <HugeiconsIcon icon={Add01Icon} size={13} strokeWidth={2} />
                Add member
              </Button>
            </div>

            {draft.members.length === 0 ? (
              <div className="rounded-[12px] border border-dashed border-border px-5 py-10 text-center">
                <p className="text-[13px] font-medium">No members yet</p>
                <p className="mt-1 text-xs text-muted-foreground">
                  Add a project, prompt, skills, agents, MCP servers, memory, or media.
                </p>
              </div>
            ) : (
              <div className="space-y-1.5">
                {draft.members.map((member, index) => {
                  const option = optionByTarget.get(targetKey(member.kind, member.target));
                  const storedResolution = member.id
                    ? resolutions.get(member.id)
                    : undefined;
                  const resolution: BundleTargetResolution = option
                    ? {
                        status: "ready",
                        currentLabel: option.label,
                        reason: null,
                      }
                    : storedResolution ?? {
                        status: "incompatible",
                        currentLabel: null,
                        reason: `Not available to ${RUNNER_LABEL[draft.runner]}`,
                      };
                  const fallbackLabel =
                    selected?.bundle.members.find((saved) => saved.id === member.id)
                      ?.snapshotLabel ?? "Unavailable target";
                  return (
                    <MemberRow
                      key={member.id ?? targetKey(member.kind, member.target)}
                      member={member}
                      index={index}
                      count={draft.members.length}
                      label={resolution.currentLabel ?? fallbackLabel}
                      resolution={resolution}
                      hasOtherPrimary={draft.members.some(
                        (candidate, candidateIndex) =>
                          candidateIndex !== index &&
                          candidate.kind === "agent" &&
                          candidate.role === "primary",
                      )}
                      onRole={(role) => editMember(index, { role })}
                      onMove={(direction) => moveMember(index, direction)}
                      onRemove={() => removeMember(index)}
                    />
                  );
                })}
              </div>
            )}
          </section>
        </div>
      </main>

      <MemberPicker
        open={pickerOpen}
        onOpenChange={setPickerOpen}
        options={options}
        selected={new Set(
          draft.members.map((member) => targetKey(member.kind, member.target)),
        )}
        onAdd={addMember}
      />
    </div>
  );
}

function Field({
  label,
  hint,
  className,
  children,
}: {
  label: string;
  hint?: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <label className={cn("space-y-1.5", className)}>
      <span className="flex items-center justify-between gap-3 text-[10px] font-medium text-muted-foreground">
        {label}
        {hint ? <span className="truncate font-normal text-tertiary">{hint}</span> : null}
      </span>
      {children}
    </label>
  );
}

function MemberRow({
  member,
  index,
  count,
  label,
  resolution,
  hasOtherPrimary,
  onRole,
  onMove,
  onRemove,
}: {
  member: BundleMemberDraft;
  index: number;
  count: number;
  label: string;
  resolution: BundleTargetResolution;
  hasOtherPrimary: boolean;
  onRole: (role: BundleMemberRole) => void;
  onMove: (direction: -1 | 1) => void;
  onRemove: () => void;
}) {
  return (
    <div className="grid grid-cols-[26px_82px_minmax(0,1fr)_170px_92px] items-center gap-3 rounded-[10px] border border-border bg-card px-3 py-2.5">
      <span className="flex size-[22px] items-center justify-center rounded-md bg-hover font-mono text-[10px] text-tertiary">
        {index + 1}
      </span>
      <span className="rounded-[5px] border border-border bg-hover px-1.5 py-1 text-center font-mono text-[9px] text-tertiary">
        {KIND_LABEL[member.kind]}
      </span>
      <span className="min-w-0">
        <span className="flex items-center gap-2">
          <StatusDot status={resolution.status === "ready" ? "ok" : "warn"} />
          <span className="truncate text-xs font-medium">{label}</span>
        </span>
        {resolution.reason ? (
          <span className="mt-1 block truncate text-[10px] text-gold">
            {resolution.reason}
          </span>
        ) : null}
      </span>
      <NativeSelect
        size="sm"
        className="w-full"
        aria-label={`Role for ${label}`}
        value={member.role}
        onChange={(event) => onRole(event.target.value as BundleMemberRole)}
      >
        {ROLE_OPTIONS[member.kind].map((role) => (
          <NativeSelectOption
            key={role}
            value={role}
            disabled={role === "primary" && hasOtherPrimary}
          >
            {ROLE_LABEL[role]}
          </NativeSelectOption>
        ))}
      </NativeSelect>
      <span className="flex items-center justify-end gap-1">
        <SmallAction
          label={`Move ${label} up`}
          disabled={index === 0}
          icon={ArrowUp01Icon}
          onClick={() => onMove(-1)}
        />
        <SmallAction
          label={`Move ${label} down`}
          disabled={index === count - 1}
          icon={ArrowDown01Icon}
          onClick={() => onMove(1)}
        />
        <SmallAction label={`Remove ${label}`} icon={Delete02Icon} onClick={onRemove} />
      </span>
    </div>
  );
}

function SmallAction({
  label,
  disabled,
  icon,
  onClick,
}: {
  label: string;
  disabled?: boolean;
  icon: typeof ArrowUp01Icon;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      disabled={disabled}
      className="flex size-6 items-center justify-center rounded-md text-tertiary transition-colors hover:bg-hover hover:text-foreground disabled:opacity-25"
      onClick={onClick}
    >
      <HugeiconsIcon icon={icon} size={12} strokeWidth={1.8} />
    </button>
  );
}

function MemberPicker({
  open,
  onOpenChange,
  options,
  selected,
  onAdd,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  options: TargetOption[];
  selected: Set<string>;
  onAdd: (option: TargetOption) => void;
}) {
  const [filter, setFilter] = useState<PickerFilter>("All");
  const [query, setQuery] = useState("");
  const visible = useMemo(() => {
    const kind = KIND_FILTER[filter];
    const needle = query.trim().toLocaleLowerCase();
    return options.filter(
      (option) =>
        (!kind || option.kind === kind) &&
        (!needle ||
          option.label.toLocaleLowerCase().includes(needle) ||
          option.detail.toLocaleLowerCase().includes(needle)),
    );
  }, [filter, options, query]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[720px]">
        <DialogHeader>
          <DialogTitle>Add a real bundle member</DialogTitle>
          <DialogDescription>
            Targets come from the current library, registered projects, MCP declarations, and media collections.
          </DialogDescription>
        </DialogHeader>
        <Segmented
          options={PICKER_FILTERS}
          value={filter}
          onChange={setFilter}
          layoutId="bundle-picker-filter"
        />
        <label className="relative block">
          <span className="sr-only">Filter bundle targets</span>
          <HugeiconsIcon
            icon={Search01Icon}
            size={14}
            strokeWidth={1.8}
            className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-tertiary"
          />
          <Input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Filter by name or source"
            className="pl-9"
          />
        </label>
        <div className="max-h-[390px] space-y-1 overflow-y-auto pr-1">
          {visible.map((option) => {
            const key = targetKey(option.kind, option.target);
            const added = selected.has(key);
            return (
              <button
                key={option.key}
                type="button"
                disabled={added}
                className="av-hover-grad flex w-full items-center gap-3 rounded-[9px] border border-border bg-card px-3 py-2.5 text-left transition-colors hover:border-border-strong disabled:cursor-not-allowed disabled:opacity-45"
                onClick={() => onAdd(option)}
              >
                <span className="w-[62px] shrink-0 rounded-[5px] bg-hover px-1.5 py-1 text-center font-mono text-[9px] text-tertiary">
                  {KIND_LABEL[option.kind]}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-xs font-medium">{option.label}</span>
                  <span className="mt-0.5 block truncate text-[10px] text-tertiary">
                    {option.detail}
                  </span>
                </span>
                <span className="text-[10px] text-muted-foreground">
                  {added ? "Added" : "Add"}
                </span>
              </button>
            );
          })}
          {visible.length === 0 ? (
            <div className="rounded-[9px] border border-dashed border-border px-4 py-8 text-center text-xs text-muted-foreground">
              No real targets match this filter.
            </div>
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}
