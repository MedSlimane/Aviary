import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as motionReact from "motion/react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { notify } from "@/lib/notify";
import { claudeMcpRegistrationCommand } from "@/lib/mcp-registration";
import { PageHeader, SectionLabel } from "@/components/screen-parts";
import { cn } from "@/lib/utils";
import {
  createCollection,
  importMedia,
  listCollections,
  listMedia,
  mediaMcpRegistration,
  removeMedia,
  searchMedia,
  setCollectionMembership,
  type MediaCollection,
  type MediaItem,
  type McpRegistration,
} from "@/lib/api";

const { motion, AnimatePresence } = motionReact;

const COLUMNS = 4;
const IMAGE_EXTS = [
  "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "mp4", "mov", "webm",
];

function tilde(p: string) {
  return p.replace(/^\/Users\/[^/]+/, "~");
}

function bytes(n: number) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} kB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function InspirationView() {
  const [items, setItems] = useState<MediaItem[] | null>(null);
  const [collections, setCollections] = useState<MediaCollection[]>([]);
  const [collection, setCollection] = useState<number | null>(null);
  const [query, setQuery] = useState("");
  const [dragging, setDragging] = useState(false);
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<MediaItem | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const loadRequestRef = useRef(0);

  const load = useCallback(async () => {
    const request = ++loadRequestRef.current;
    setItems(null);
    setLoadError(null);
    try {
      const [media, cols] = await Promise.all([
        query.trim() ? searchMedia(query) : listMedia(collection ?? undefined),
        listCollections(),
      ]);
      if (request !== loadRequestRef.current) return;
      setItems(media);
      setCollections(cols);
    } catch (e) {
      if (request !== loadRequestRef.current) return;
      const message = e instanceof Error ? e.message : String(e);
      setLoadError(message);
      notify("Could not load the board", { description: message });
    }
  }, [collection, query]);

  useEffect(() => {
    void load();
    return () => {
      loadRequestRef.current += 1;
    };
  }, [load]);

  const runImport = useCallback(
    async (paths: string[]) => {
      const usable = paths.filter((p) =>
        IMAGE_EXTS.includes(p.split(".").pop()?.toLowerCase() ?? ""),
      );
      if (usable.length === 0) {
        notify("Nothing to import", {
          description: "Drop images or clips — other file types aren't handled yet.",
        });
        return;
      }
      setBusy(true);
      try {
        const added = await importMedia(usable);
        // Import is content-addressed, so re-dropping a file is a no-op rather
        // than a duplicate tile. Report what actually landed.
        notify(`Imported ${added.length}`, {
          description:
            usable.length > added.length
              ? `${usable.length - added.length} already on the board`
              : undefined,
        });
        await load();
      } catch (e) {
        notify("Import failed", { description: String(e) });
      } finally {
        setBusy(false);
      }
    },
    [load],
  );

  // Tauri delivers OS-level file drops through the webview, not DOM events —
  // a browser `drop` handler receives no usable path.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "over") setDragging(true);
        else if (event.payload.type === "drop") {
          setDragging(false);
          void runImport(event.payload.paths);
        } else setDragging(false);
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {
        /* drag-drop unavailable; the Import button still works */
      });
    return () => unlisten?.();
  }, [runImport]);

  const pick = useCallback(async () => {
    const chosen = await openDialog({
      multiple: true,
      filters: [{ name: "Media", extensions: IMAGE_EXTS }],
    });
    if (!chosen) return;
    await runImport(Array.isArray(chosen) ? chosen : [chosen]);
  }, [runImport]);

  // Balance by running height rather than round-robin, so a column of tall
  // images doesn't leave the others short.
  const columns = useMemo(() => {
    const cols: MediaItem[][] = Array.from({ length: COLUMNS }, () => []);
    const heights = new Array(COLUMNS).fill(0);
    for (const item of items ?? []) {
      const ratio =
        item.width && item.height ? item.height / item.width : 0.72;
      const shortest = heights.indexOf(Math.min(...heights));
      cols[shortest].push(item);
      heights[shortest] += ratio;
    }
    return cols;
  }, [items]);

  const total = items?.length ?? 0;

  return (
    <div
      className={cn(
        "relative flex flex-col gap-[18px] p-[26px]",
        dragging && "ring-2 ring-inset ring-violet",
      )}
    >
      <PageHeader
        title="Inspiration"
        subtitle={
          loadError
            ? "Could not load the board"
            : items === null
              ? "Loading board…"
              : total === 1
                ? `1 item · ${collections.length} collections`
                : `${total} items · ${collections.length} collections`
        }
        action={
          <div className="flex items-center gap-2">
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search colour, tag, name…"
              className="w-[220px] rounded-[9px] border border-border bg-elevated px-3 py-[7px] text-xs outline-none transition-colors placeholder:text-tertiary focus:border-border-strong"
            />
            <Button size="sm" className="rounded-full" onClick={pick} disabled={busy}>
              {busy ? "Importing…" : "Import"}
            </Button>
          </div>
        }
      />

      <CollectionBar
        collections={collections}
        active={collection}
        onPick={setCollection}
        onCreate={async (name) => {
          await createCollection(name);
          await load();
        }}
      />

      <McpCallout collectionId={collection} />

      {loadError ? (
        <div className="rounded-[12px] border border-destructive/30 bg-destructive/5 px-4 py-4">
          <p className="text-xs font-medium">The board could not be loaded.</p>
          <p className="mt-1 break-words text-[11px] leading-relaxed text-muted-foreground">
            {loadError}
          </p>
          <Button
            size="sm"
            variant="outline"
            className="mt-3 rounded-full"
            onClick={() => void load()}
          >
            Try again
          </Button>
        </div>
      ) : items === null ? (
        <div className="flex gap-3">
          {Array.from({ length: COLUMNS }).map((_, i) => (
            <div key={i} className="flex flex-1 flex-col gap-3">
              <Skeleton className="h-[180px] rounded-xl" />
              <Skeleton className="h-[120px] rounded-xl" />
            </div>
          ))}
        </div>
      ) : total === 0 ? (
        <EmptyBoard onPick={pick} searching={query.trim().length > 0} />
      ) : (
        <div className="flex gap-3">
          {columns.map((col, ci) => (
            <div key={ci} className="flex flex-1 flex-col gap-3">
              <AnimatePresence mode="popLayout" initial={false}>
                {col.map((item) => (
                  <Tile
                    key={item.hash}
                    item={item}
                    onOpen={() => setSelected(item)}
                  />
                ))}
              </AnimatePresence>
            </div>
          ))}
        </div>
      )}

      {dragging && (
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center bg-canvas/60">
          <p className="rounded-full border border-violet bg-elevated px-5 py-2.5 text-sm font-medium">
            Drop to add to the board
          </p>
        </div>
      )}

      {selected && (
        <Detail
          item={selected}
          collections={collections}
          onClose={() => setSelected(null)}
          onChanged={load}
        />
      )}
    </div>
  );
}

function Tile({ item, onOpen }: { item: MediaItem; onOpen: () => void }) {
  const src = item.thumb ?? item.path;
  const ratio = item.width && item.height ? item.height / item.width : 0.72;

  return (
    <motion.button
      layout
      type="button"
      initial={{ opacity: 0, scale: 0.94 }}
      animate={{ opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.94 }}
      whileHover={{ scale: 1.015 }}
      whileTap={{ scale: 0.985 }}
      transition={{ type: "spring", stiffness: 400, damping: 32 }}
      onClick={onOpen}
      className="av-hover-ring group relative w-full shrink-0 overflow-hidden rounded-xl ring-1 ring-inset ring-glass-border"
      // The sampled colour stands in while the image decodes, so the board
      // never flashes empty boxes.
      style={{ backgroundColor: item.dominant ?? "var(--av-bg-elevated)" }}
    >
      <div style={{ paddingBottom: `${Math.min(Math.max(ratio, 0.4), 2) * 100}%` }} />
      {item.kind === "image" ? (
        <img
          src={convertFileSrc(src)}
          alt={item.title ?? ""}
          loading="lazy"
          className="absolute inset-0 size-full object-cover"
        />
      ) : (
        <span className="absolute inset-0 flex items-center justify-center font-mono text-[11px] text-secondary">
          {item.ext.toUpperCase()}
        </span>
      )}
      <span className="absolute inset-x-0 bottom-0 translate-y-full bg-gradient-to-t from-black/80 to-transparent px-2.5 pb-2 pt-6 text-left transition-transform group-hover:translate-y-0">
        <span className="block truncate text-[11px] font-medium text-white">
          {item.title ?? item.hash.slice(0, 8)}
        </span>
      </span>
    </motion.button>
  );
}

function CollectionBar({
  collections,
  active,
  onPick,
  onCreate,
}: {
  collections: MediaCollection[];
  active: number | null;
  onPick: (id: number | null) => void;
  onCreate: (name: string) => Promise<void>;
}) {
  const [adding, setAdding] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  return (
    <div className="flex flex-wrap items-center gap-2">
      <Chip active={active === null} onClick={() => onPick(null)}>
        All
      </Chip>
      {collections.map((c) => (
        <Chip key={c.id} active={active === c.id} onClick={() => onPick(c.id)}>
          {c.name}
          <span className="ml-1.5 text-tertiary">{c.count}</span>
        </Chip>
      ))}

      {adding ? (
        <input
          ref={inputRef}
          autoFocus
          placeholder="Collection name"
          className="w-[150px] rounded-full border border-border bg-elevated px-3 py-1.5 text-xs outline-none focus:border-border-strong"
          onBlur={() => setAdding(false)}
          onKeyDown={async (e) => {
            if (e.key === "Escape") setAdding(false);
            if (e.key === "Enter" && e.currentTarget.value.trim()) {
              await onCreate(e.currentTarget.value.trim());
              setAdding(false);
            }
          }}
        />
      ) : (
        <button
          type="button"
          onClick={() => setAdding(true)}
          className="rounded-full border border-dashed border-border px-3 py-1.5 text-xs text-tertiary transition-colors hover:border-border-strong hover:text-secondary"
        >
          + Collection
        </button>
      )}
    </div>
  );
}

function Chip({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "rounded-full border px-3 py-1.5 text-xs transition-colors",
        active
          ? "border-transparent bg-violet text-canvas"
          : "border-border text-secondary hover:border-border-strong",
      )}
    >
      {children}
    </button>
  );
}

function McpCallout({ collectionId }: { collectionId: number | null }) {
  const [copied, setCopied] = useState(false);
  const [registration, setRegistration] = useState<McpRegistration | null>(null);
  const [registrationError, setRegistrationError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setRegistration(null);
    setRegistrationError(null);
    void mediaMcpRegistration(collectionId ?? undefined)
      .then((next) => {
        if (live) setRegistration(next);
      })
      .catch((reason) => {
        if (live) setRegistrationError(String(reason));
      });
    return () => {
      live = false;
    };
  }, [collectionId]);

  const command = registration
    ? claudeMcpRegistrationCommand(registration)
    : null;

  return (
    <motion.div
      whileHover={{ y: -1 }}
      transition={{ type: "spring", stiffness: 480, damping: 30 }}
      className="av-hover-grad flex items-center gap-3.5 overflow-hidden rounded-[14px] border border-border bg-card p-3.5"
    >
      <div className="min-w-0 flex-1 space-y-0.5">
        <p className="truncate text-[13px] font-medium">
          Your agents can search this board
        </p>
        <p className="truncate font-mono text-[11px] text-tertiary">
          {command ??
            (registrationError
              ? "Bundled MCP server unavailable"
              : "Locating bundled MCP server…")}
        </p>
      </div>
      <Button
        size="sm"
        variant="outline"
        className="shrink-0"
        disabled={!command}
        onClick={async () => {
          if (!command) return;
          try {
            await navigator.clipboard.writeText(command);
            setCopied(true);
            setTimeout(() => setCopied(false), 1600);
          } catch (reason) {
            notify("Could not copy MCP registration", {
              description: String(reason),
            });
          }
        }}
      >
        {copied ? "Copied" : "Copy"}
      </Button>
    </motion.div>
  );
}

function EmptyBoard({
  onPick,
  searching,
}: {
  onPick: () => void;
  searching: boolean;
}) {
  return (
    <div className="rounded-[14px] border border-dashed border-border bg-card p-10 text-center">
      <p className="text-[15px] font-semibold">
        {searching ? "Nothing matches" : "Nothing on the board yet"}
      </p>
      <p className="mx-auto mt-1.5 max-w-[420px] text-xs text-muted-foreground">
        {searching
          ? "Try a colour like “teal”, an orientation like “landscape”, or part of a filename."
          : "Drag images anywhere on this page, or import them. Aviary copies each file into its own store, so your tiles survive the original moving."}
      </p>
      {!searching && (
        <Button size="sm" className="mt-4 rounded-full" onClick={onPick}>
          Import media
        </Button>
      )}
    </div>
  );
}

function Detail({
  item,
  collections,
  onClose,
  onChanged,
}: {
  item: MediaItem;
  collections: MediaCollection[];
  onClose: () => void;
  onChanged: () => Promise<void>;
}) {
  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-canvas/70 p-10"
      onClick={onClose}
    >
      <motion.div
        initial={{ opacity: 0, scale: 0.97 }}
        animate={{ opacity: 1, scale: 1 }}
        onClick={(e) => e.stopPropagation()}
        className="flex max-h-full w-full max-w-[860px] gap-5 overflow-hidden rounded-[16px] border border-border bg-card p-5"
      >
        <div
          className="flex min-h-[300px] flex-1 items-center justify-center overflow-hidden rounded-xl"
          style={{ backgroundColor: item.dominant ?? "var(--av-bg-inset)" }}
        >
          {item.kind === "image" && (
            <img
              src={convertFileSrc(item.path)}
              alt={item.title ?? ""}
              className="max-h-[62vh] max-w-full object-contain"
            />
          )}
        </div>

        <div className="w-[260px] shrink-0 space-y-4 overflow-y-auto">
          <div className="space-y-1">
            <p className="text-sm font-semibold">{item.title ?? "Untitled"}</p>
            <p className="font-mono text-[11px] text-tertiary">
              {item.width && item.height ? `${item.width}×${item.height} · ` : ""}
              {bytes(item.bytes)} · {item.ext}
            </p>
          </div>

          <div>
            <SectionLabel>TAGS</SectionLabel>
            <div className="mt-2 flex flex-wrap gap-1.5">
              {item.tags.length === 0 && (
                <span className="text-[11px] text-tertiary">None</span>
              )}
              {item.tags.map((t) => (
                <span
                  key={t}
                  className="rounded-full bg-hover px-2 py-0.5 text-[10px] text-secondary"
                >
                  {t}
                </span>
              ))}
            </div>
          </div>

          <div>
            <SectionLabel>COLLECTIONS</SectionLabel>
            <div className="mt-2 flex flex-wrap gap-1.5">
              {collections.length === 0 && (
                <span className="text-[11px] text-tertiary">
                  Create one to group references
                </span>
              )}
              {collections.map((c) => (
                <button
                  key={c.id}
                  type="button"
                  onClick={async () => {
                    await setCollectionMembership(c.id, item.hash, true);
                    await onChanged();
                  }}
                  className="rounded-full border border-border px-2.5 py-1 text-[10px] text-secondary transition-colors hover:border-border-strong"
                >
                  + {c.name}
                </button>
              ))}
            </div>
          </div>

          {item.origin && (
            <div>
              <SectionLabel>IMPORTED FROM</SectionLabel>
              <p className="mt-1.5 break-all font-mono text-[10px] text-tertiary">
                {tilde(item.origin)}
              </p>
            </div>
          )}

          <div>
            <SectionLabel>PATH FOR AGENTS</SectionLabel>
            <p className="mt-1.5 break-all font-mono text-[10px] text-secondary">
              {tilde(item.path)}
            </p>
          </div>

          <div className="flex gap-2 pt-2">
            <Button size="sm" variant="secondary" className="flex-1" onClick={onClose}>
              Close
            </Button>
            <Button
              size="sm"
              variant="ghost"
              className="text-coral"
              onClick={async () => {
                await removeMedia(item.hash);
                onClose();
                await onChanged();
                notify("Removed from the board");
              }}
            >
              Remove
            </Button>
          </div>
        </div>
      </motion.div>
    </div>
  );
}
