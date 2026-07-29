import { useEffect, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeHighlight from "rehype-highlight";
import prettyBytes from "pretty-bytes";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Cancel01Icon,
  Alert02Icon,
  Copy01Icon,
  Folder01Icon,
} from "@hugeicons/core-free-icons";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  readEntry,
  RUNNER_LABEL,
  type Entry,
  type EntryContent,
} from "@/lib/api";
import { notify } from "@/lib/notify";
import { cn } from "@/lib/utils";

function tilde(p: string) {
  return p.replace(/^\/Users\/[^/]+/, "~");
}

const RELATIVE = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

function relativeDate(unix: number) {
  if (!unix) return "unknown";
  const days = Math.round((unix * 1000 - Date.now()) / 86_400_000);
  if (Math.abs(days) < 30) return RELATIVE.format(days, "day");
  return new Date(unix * 1000).toLocaleDateString();
}

function Field({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex items-baseline gap-3">
      <span className="w-[74px] shrink-0 text-[11px] text-tertiary">{label}</span>
      <span className="min-w-0 flex-1 text-[12px]">{value}</span>
    </div>
  );
}

export function EntryDetail({
  entry,
  onClose,
}: {
  entry: Entry;
  onClose: () => void;
}) {
  const [content, setContent] = useState<EntryContent | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setContent(null);
    setError(null);
    readEntry(entry.path)
      .then((c) => !cancelled && setContent(c))
      .catch(
        (e) => !cancelled && setError(e instanceof Error ? e.message : String(e)),
      );
    return () => {
      cancelled = true;
    };
  }, [entry.path]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  const readOnly = entry.source === "plugin";

  return (
    <aside className="sticky top-0 flex h-full max-h-full min-h-0 w-[460px] shrink-0 flex-col overflow-hidden rounded-[14px] border border-border bg-card">
      <div className="flex shrink-0 items-start gap-3 border-b border-border px-4 py-3.5">
        <div className="min-w-0 flex-1 space-y-1">
          <h2 className="truncate text-[15px] font-semibold">{entry.name}</h2>
          <p className="truncate font-mono text-[11px] text-tertiary">
            {tilde(entry.path)}
          </p>
        </div>
        <button
          type="button"
          onClick={onClose}
          aria-label="Close"
          className="shrink-0 rounded-md p-1 text-muted-foreground transition-colors hover:bg-hover hover:text-foreground"
        >
          <HugeiconsIcon icon={Cancel01Icon} size={15} strokeWidth={2} />
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {/* Metadata */}
        <div className="space-y-2 border-b border-border p-4">
          <Field label="Kind" value={entry.kind} />
          <Field
            label="Runners"
            value={
              <span className="flex flex-wrap gap-1.5">
                {entry.runners.map((r) => (
                  <span
                    key={r}
                    className="flex items-center gap-1.5 rounded-full border border-border bg-elevated px-2 py-0.5 text-[11px]"
                  >
                    <span
                      className={cn(
                        "size-1.5 rounded-full",
                        r === "claude-code" ? "bg-claude" : "bg-codex",
                      )}
                    />
                    {RUNNER_LABEL[r]}
                  </span>
                ))}
              </span>
            }
          />
          {entry.group && <Field label="Pack" value={entry.group} />}
          <Field
            label="Source"
            value={
              readOnly ? (
                <span className="text-warn">plugin · read-only</span>
              ) : (
                entry.source
              )
            }
          />
          <Field
            label="Size"
            value={
              <>
                {prettyBytes(entry.bytes)}
                {content && (
                  <span className="text-tertiary">
                    {" · "}
                    {content.tokens.toLocaleString()} tokens
                  </span>
                )}
              </>
            }
          />
          <Field label="Changed" value={relativeDate(entry.modified)} />
          {entry.realPath !== entry.path && (
            <Field
              label="Links to"
              value={
                <span className="break-all font-mono text-[11px] text-muted-foreground">
                  {tilde(entry.realPath)}
                </span>
              }
            />
          )}
        </div>

        {/* Content */}
        <div className="p-4">
          {error ? (
            <div className="flex items-center gap-2.5 rounded-[10px] border border-destructive/30 bg-destructive/10 px-3 py-2.5">
              <HugeiconsIcon
                icon={Alert02Icon}
                size={14}
                strokeWidth={1.8}
                className="text-destructive"
              />
              <p className="text-[12px]">{error}</p>
            </div>
          ) : !content ? (
            <div className="space-y-2">
              {Array.from({ length: 8 }).map((_, i) => (
                <Skeleton key={i} className="h-4 w-full rounded" />
              ))}
            </div>
          ) : (
            <Tabs defaultValue="preview">
              <div className="mb-3 flex items-center gap-2">
                <TabsList>
                  <TabsTrigger value="preview">Preview</TabsTrigger>
                  <TabsTrigger value="source">Source</TabsTrigger>
                  {content.frontmatter && (
                    <TabsTrigger value="frontmatter">Frontmatter</TabsTrigger>
                  )}
                </TabsList>
                <div className="flex-1" />
                <button
                  type="button"
                  onClick={() => {
                    void navigator.clipboard.writeText(content.raw);
                    notify("Copied", { description: entry.name });
                  }}
                  className="flex items-center gap-1.5 text-[11px] text-muted-foreground transition-colors hover:text-foreground"
                >
                  <HugeiconsIcon icon={Copy01Icon} size={12} strokeWidth={1.8} />
                  Copy
                </button>
              </div>

              <TabsContent value="preview">
                <div className="av-markdown text-[13px] leading-relaxed">
                  <Markdown
                    remarkPlugins={[remarkGfm]}
                    rehypePlugins={[rehypeHighlight]}
                  >
                    {content.body.trim() || "_(empty)_"}
                  </Markdown>
                </div>
              </TabsContent>

              <TabsContent value="source">
                <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded-[10px] border border-border bg-inset p-3 font-mono text-[11px] leading-relaxed text-muted-foreground">
                  {content.raw}
                </pre>
              </TabsContent>

              {content.frontmatter && (
                <TabsContent value="frontmatter">
                  <pre className="overflow-x-auto rounded-[10px] border border-border bg-inset p-3 font-mono text-[11px] leading-relaxed text-teal">
                    {content.frontmatter.trim()}
                  </pre>
                </TabsContent>
              )}
            </Tabs>
          )}
        </div>
      </div>

      <div className="flex shrink-0 items-center gap-2 border-t border-border px-4 py-3">
        <Button
          size="sm"
          variant="outline"
          onClick={() =>
            notify("Reveal in Finder", { description: tilde(entry.path) })
          }
        >
          <HugeiconsIcon icon={Folder01Icon} size={13} strokeWidth={1.8} />
          Reveal
        </Button>
        <div className="flex-1" />
        <Button
          size="sm"
          disabled
          title={readOnly ? "Plugin entries are read-only" : "Editing lands next"}
        >
          Edit
        </Button>
      </div>
    </aside>
  );
}
