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
│  ├─ mcp.rs              MCP server discovery across user/plugin/project configs
│  ├─ media.rs            content-addressed media store, thumbnails, auto-tagging
│  ├─ mcp_media.rs        JSON-RPC handling for the aviary-media MCP server
│  ├─ bin/aviary_media.rs the MCP server binary (stdio)
│  ├─ runner.rs           CLI supervisor, NDJSON stream → Tauri channel
│  ├─ models.rs           model + reasoning-effort discovery, per runner
│  ├─ discovery.rs        candidate project detection
│  ├─ store.rs            SQLite — data.db and cache.db
│  ├─ writer.rs           atomic writes, snapshots, conflict detection
│  ├─ tokens.rs           tiktoken (o200k_base)
│  └─ lib.rs              Tauri commands
│
└─ src/
   ├─ views/              home · chat · library · projects · mcp · context · inspiration · settings
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
- **The database can be deleted** without losing anything a runner needs.
- **Aviary-only data** — preferences, media, collections, tags — has no home in a
  runner's format, so it lives in `~/.aviary/` and *is* durable. Hence the split
  below.

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
└─ history/         pre-write snapshots
```

| | `data.db` | `cache.db` |
|---|---|---|
| Holds | preferences, projects, media, collections, tags, entry metadata | library/mcp/project scan snapshots, token counts, thumbnail paths |
| Recreatable | **No** | Yes, by re-scanning |
| Deleting it | loses your data | costs a re-index |

The split is what makes caching safe. The design rule is *"deleting the database
must cost nothing but a re-index"* — which only holds if durable data lives
somewhere else. Both use WAL and `PRAGMA user_version` migrations.

**Measured:** 101 ms fresh scan → 1 ms cached, on a real 117-entry library.

### Identity choices

- **Library entries** are keyed by canonical path, so a favourite survives a file
  moving between runner directories.
- **Media** is keyed by the sha256 of its bytes. Re-importing the same file is a
  no-op rather than a duplicate tile, and a tile never dies because the original
  left `~/Downloads`.

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

## Honesty about what can be measured

`context.rs` exists to answer *"why did the agent behave that way?"*, so its
value rests entirely on being true. Two rules follow:

- **Only count what is on disk.** Every token figure comes from tokenising a real
  file.
- **Say so when a cost cannot be known.** The runner's built-in system prompt
  ships inside the binary; MCP tool schemas only arrive after a handshake. Those
  layers are reported with `measured: false` and *no token figure* rather than a
  plausible guess, and are excluded from the total.

A related correction: **skills contribute their frontmatter, not their bodies.**
A runner lists available skills up front and loads a `SKILL.md` only when
invoked. Counting whole bodies would inflate the figure roughly tenfold and make
the entire screen untrustworthy.

---

## The two chat engines

Both emit into one normalised event stream, so the renderer never branches on
engine:

```
SessionEvent = Started | Thinking | Text(delta) | ToolCall | ToolResult
             | PermissionRequest | TokenUsage | Finished | Error
```

| | CLI engine (default) | API engine |
|---|---|---|
| Invocation | `claude -p --output-format stream-json`, `codex exec --json` | provider SDK |
| Gets | tools, MCP, skills, permissions, session resume — free | nothing but the model |
| Library edits apply | immediately, same files | n/a |
| Used for | all agentic work | quick no-tool questions |

Models and reasoning-effort levels are **discovered from the machine** — parsed
from the CLI's `--help` and probed — never hardcoded. That is why `models.rs`
has tests that skip when the CLI is absent.

---

## Aviary serves, it doesn't just show

`aviary-media` is a **separate binary** because MCP speaks JSON-RPC over stdio —
the desktop app cannot be the thing a bare `claude` session spawns. Both read the
same `data.db`; the server only ever reads, so running it while Aviary is open is
safe (WAL).

```
claude mcp add aviary-media -- /Applications/aviary.app/Contents/MacOS/aviary-media
```

It is read-only **by construction**, not by convention: no tool in
`mcp_media.rs` mutates anything, so an agent cannot delete a designer's
references.

`aviary-library` is the planned sibling — see [`ROADMAP.md`](ROADMAP.md) P4.

---

## Performance contract

Requirements, not aspirations:

- No file I/O, parsing or hashing on the UI thread. Ever.
- Scans are cached; a cold launch paints real content, not a spinner.
- Live `backdrop-filter` only on small transient surfaces — card and hero
  "glass" is a pre-rendered gradient texture plus a 1 px inner stroke. This is
  precisely why the asset pack exists: to buy the look without paying runtime
  blur on every scroll frame.
- Cold start < 400 ms, idle RSS < 120 MB.

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

28 tests, green in CI on every push.
