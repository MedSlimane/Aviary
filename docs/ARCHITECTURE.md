# Architecture

How Aviary is put together, and why. This describes what exists today — see
[`ROADMAP.md`](ROADMAP.md) for what does not.

---

## The shape

**Tauri v2 · Rust core · React + TypeScript shell.**

Rust owns everything expensive: walking thousands of files, parsing frontmatter,
tokenising, hashing media, spawning runner processes. React owns presentation
and nothing else. Every command that touches the filesystem runs on
`spawn_blocking`, so the UI thread is never held by a subprocess or a directory
walk.

```
aviary/
├─ src-tauri/src/
│  ├─ providers/          one file per runner — the blast radius for format drift
│  │  ├─ mod.rs           Entry, Kind, Source, frontmatter parsing, dedupe
│  │  ├─ claude_code.rs   ~/.claude layout
│  │  └─ codex.rs         ~/.codex layout
│  ├─ library.rs          assembles the index from providers + registered projects
│  ├─ context.rs          resolves the instruction stack for (runner, cwd)
│  ├─ mcp.rs              static MCP inventory + explicit bounded health checks
│  ├─ mcp_protocol.rs     shared bounded MCP JSON-RPC lifecycle
│  ├─ media.rs            content-addressed media store, thumbnails, auto-tagging
│  ├─ mcp_media.rs        read-only tools for the media MCP server
│  ├─ mcp_library.rs      read-only tools for the live library MCP server
│  ├─ launch.rs           private, one-use Terminal handoffs
│  ├─ bin/                media, library, and launch helper entry points
│  ├─ runner.rs + runner/ CLI supervisor and runner protocol adapters
│  ├─ models.rs           model + reasoning-effort discovery, per runner
│  ├─ discovery.rs        candidate project detection
│  ├─ store.rs + store/   SQLite, sessions, and bundles
│  ├─ watcher.rs          debounced native filesystem invalidation
│  ├─ diagnostics.rs      bounded local logs and copyable reports
│  ├─ writer.rs           atomic writes, snapshots, conflict detection
│  ├─ tokens.rs           tiktoken (o200k_base)
│  └─ lib.rs              Tauri commands
│
└─ src/
   ├─ views/              home · chat · library · projects · bundles · mcp · context · inspiration · settings
   ├─ components/         rail, title bar, shared screen parts, shadcn ui
   ├─ lib/                api (the single IPC boundary), theme, motion, notify
   └─ index.css           design tokens → shadcn token bridge
```

`src/lib/api.ts` is the **only** place the frontend calls `invoke`. Every command
gets a typed wrapper that normalises Rust's `snake_case` into `camelCase`. If you
are adding an IPC call and not editing that file, you are doing it wrong.

---

## Files are the source of truth

The rule the whole design hangs on. Every user-visible entity is a real file on
disk, in the format its runner already expects. Aviary reads and writes those
files; it never becomes the system of record.

Consequences:

- **Editing a skill in Aviary changes agent behaviour on the next turn**, with no
  sync or export step, because it is the same file the runner reads.
- **Runner files remain the source for runner configuration.** Deleting
  `cache.db` changes no behaviour and only costs a re-index.
- **Aviary-owned data** — preferences, projects, media, collections, chat
  transcripts and bundles — has no complete home in a runner's format, so it
  lives in `~/.aviary/data.db` and is durable. Hence the split below.

### Skills are not owned by a runner

The finding that shapes `providers/mod.rs`: skills live in a shared pool
(`~/.agents/skills`) and are **symlinked** into each runner's directory. So
"enabled for Claude Code" is not metadata — it is the presence of a symlink.

Entries are therefore deduplicated by **canonical (symlink-resolved) path**, and
the runners that link to them are unioned into `Entry.runners`. A tool that
counts files rather than following links will disagree with Aviary; Aviary is
right.

---

## Storage: two databases

```
~/.aviary/
├─ data.db          durable — yours, never dropped
├─ cache.db         disposable — safe to delete at any time
├─ media/<hash>/    content-addressed originals
├─ cache/thumbs/    generated thumbnails
├─ history/         pre-write snapshots
└─ logs/            bounded local crash and error logs
```

| | `data.db` | `cache.db` |
|---|---|---|
| Holds | preferences, projects, media, collections, tags, chat sessions/turns/events, bundles and attachment snapshots | library/MCP/project scan snapshots, health results, token counts, thumbnail paths |
| Recreatable | **No** | Yes, by re-scanning |
| Deleting it | loses your data | costs a re-index |

The split is what makes caching safe. The design rule is *"deleting `cache.db`
must cost nothing but a re-index"*. Both databases use WAL and explicit,
transactional `PRAGMA user_version` migrations; a newer durable schema is
refused instead of guessed at.

### Identity choices

- **Library entries** are keyed by canonical path, so a favourite survives a file
  moving between runner directories.
- **Media** is keyed by the sha256 of its bytes. Re-importing the same file is a
  no-op rather than a duplicate tile, and a tile never dies because the original
  left `~/Downloads`.
- **Bundles** keep opaque target identities and snapshot labels. A missing
  target stays missing; it is never silently rebound to a similarly named file.

---

## Write safety

Writing to live agent configuration is the highest-risk thing this app does.
`writer.rs` enforces four rules, and they are not optional:

1. **Atomic** — write to a temp file in the same directory, `fsync`, rename.
2. **Snapshot first** — prior content is copied to `~/.aviary/history/<hash>/<timestamp>`.
3. **Conflict detection** — a write is refused if the on-disk hash no longer
   matches what the editor loaded. The user sees a diff and chooses.
4. **Never delete without confirmation.**

If you add a code path that writes to a runner's config, it goes through
`writer.rs`. There is no second way to write.

---

## Live indexing

`watcher.rs` watches the real provider and project roots. Events are debounced
with both a quiet period and a maximum deadline, mapped back through canonical
paths, and refreshed by scope. Atomic replacement and shared symlink targets are
tested explicitly. A bounded event queue cannot starve watcher control traffic,
and the UI receives the refreshed real snapshot through a Tauri event.

---

## Diagnostics stay local

Rust panics, rejected frontend IPC calls, React render failures and unhandled
webview errors are written under `~/.aviary/logs`. The active log rotates at
1,000,000 bytes and keeps four archives, so this directory is bounded to five
log files. Aviary does not forward the browser console wholesale: only explicit
error records are persisted, without command arguments, prompts, environment
values or config payloads.

The Error Boundary and Settings can build a user-copyable report from runtime
facts and at most 200,000 bytes from the newest Aviary logs. Home-directory
paths are shortened to `~`, log reads happen on the blocking pool, and no report
is uploaded automatically. A process-killing crash can therefore be inspected
after relaunch from Settings.

---

## Honesty about what can be measured

`context.rs` exists to answer *"why did the agent behave that way?"*, so its
value rests entirely on being true. Every layer carries an optional token count,
its measurement basis, whether the measurement is complete, whether the runner
loads it, and whether it contributes to the displayed subtotal.

- Files are tokenised from their real bytes with the bundled o200k encoder.
- MCP discovery is static and inert. Only an explicit, warned health check may
  start a local server or contact a configured endpoint. A complete tool-list
  handshake yields an o200k schema estimate cached against the exact runner,
  directory, declaration revision and expiry.
- The runner's built-in system prompt is not exposed by either CLI. It therefore
  has no number and is excluded from the known subtotal.
- Partial, stale and unavailable measurements stay visibly incomplete. Zero is
  never used as a substitute for unknown.

A related correction: **skills contribute their frontmatter, not their bodies.**
A runner lists available skills up front and loads a `SKILL.md` only when
invoked. Counting whole bodies would inflate the figure roughly tenfold and make
the entire screen untrustworthy.

---

## Durable CLI chat

Claude Code and Codex emit into one normalised event stream, so transcript
rendering does not branch on runner:

```
SessionEvent = Started | Thinking | Text(delta)
             | ToolStarted | ToolUpdated | ToolFinished
             | PermissionRequest | PermissionResolved
             | TokenUsage | Finished | Interrupted | Failed
```

The runner adapters use each installed CLI's machine-readable protocol. Prompts
go over stdin, never command arguments. A session, its first queued turn, and an
optional Bundle attachment are committed in one transaction before process
startup; the UI receives that durable receipt immediately and reconciles the
terminal state from both the live channel and `data.db`. Runner session IDs are
bound once and used for real resume after relaunch. Startup reconciliation marks
orphaned work interrupted rather than replaying it.

Permission requests are durable typed events. The UI allows only decisions the
runner protocol advertises, guards duplicate submissions, and rejects late
responses. Tool calls, results, diffs and command output stay structured instead
of being flattened into invented prose.

Models and reasoning-effort levels are **discovered from the machine** — parsed
from the CLI's `--help` and probed — never hardcoded. That is why `models.rs`
has tests that skip when the CLI is absent.

---

## Bundles and bundled helpers

A Bundle is a durable, revisioned composition of one working directory plus
optional prompt, skills, agents, memory, MCP declarations and media collection.
The editor composes only current library identities. Updates use compare-and-set
revisions, and a chat stores an immutable, secret-free attachment snapshot.
Runner, working directory and model are locked to that snapshot. Prompt members
are editable UI prefill and never hidden process input.

Execution fails closed when a current target is missing or when the installed
runner has no proven way to represent a member. Aviary does not broaden an MCP
selection, append memory by guesswork, or invent primary-agent flags.

Three executable helpers ship beside the app:

- `aviary-media` serves bounded, optionally collection-scoped media retrieval.
- `aviary-library` serves `search_library`, `get_skill`, `get_prompt`,
  `get_agent`, and `list_bundles` from fresh read-only data.
- `aviary-launch` claims a private, expiring Terminal descriptor once and then
  starts the real CLI with typed OS arguments.

The two MCP servers share a bounded JSON-RPC lifecycle and open Aviary's SQLite
files read-only. Their registration descriptors derive verified sibling paths
from the running app, so the UI never assumes `/Applications` or `PATH`.
Terminal handoffs contain no prompt or secrets in shell text, reject symlinks
and unsafe ownership/modes, and scrub failed or expired payloads.

---

## Performance contract

Requirements, not aspirations:

- No file I/O, parsing or hashing on the UI thread. Ever.
- Scans are cached; a cold launch paints real content, not a spinner.
- Subprocess output, JSON-RPC frames, identifiers, diagnostics and persisted
  event payloads all have explicit bounds.
- Live `backdrop-filter` only on small transient surfaces — card and hero
  "glass" is a pre-rendered gradient texture plus a 1 px inner stroke. This is
  precisely why the asset pack exists: to buy the look without paying runtime
  blur on every scroll frame.

---

## Design system

Five colour modes driven by the same 24 semantic variables. The Figma file is the
source; `src/index.css` aliases shadcn's token names onto them, so every shadcn
component inherits the design system rather than being restyled per-component.

Type is **Inter** and **Geist Mono**. Icons are [HugeIcons](https://hugeicons.com).

> Note for anyone working in the Figma file: SF Pro is listed by Figma's font API
> on this machine but has **no usable metrics** — text set in it measures zero
> width. Probe with a temp node before committing to a typeface.

---

## Testing philosophy

The suite deliberately runs against the **real machine** rather than fixtures:
`scans_real_machine`, `resolves_real_machine`, `catalogues_come_from_the_machine`.
Fixtures would have hidden every interesting bug in this codebase — the symlink
dedupe, the plugin cache duplication, the runner-specific config shapes.

The trade-off is that a bare CI runner has no `~/.claude`. Those tests
**skip with a printed reason** rather than failing, because an empty result on a
machine with no runners installed is correct, not broken.

The release floor is `bunx tsc --noEmit`, `cargo test --lib`, and
`cargo build --bins`, followed by driving the affected flow in the real app.
Release CI additionally verifies each universal helper, every signature, the
updater archive, the notarized DMG and a quarantined installed copy before it
publishes anything.
