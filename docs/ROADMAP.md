# Roadmap

Implementation ledger for the scheduled product work. Updated 2026-08-08 from
the post-`v0.1.0-alpha.1` source tree.

The ordering principle: **nothing ships that pretends to work.** A screen with
convincing fake data is worse than an absent one, because it spends trust that
the working parts have to earn back. Everything below is ranked by whether it
unblocks the next thing, not by how interesting it is to build.

---

## Where we are

`v0.1.0-alpha.1` remains the latest published build. The source tree now
contains the P1–P4 implementation described below, but it is not a released
proof by itself. In particular, the public updater and Developer ID paths still
need a real signed old-version-to-new-version release exercise.

| Surface | State |
|---|---|
| **Library** | Real. Discovery across both runners, dedupe by canonical path, editor with atomic writes + snapshots + conflict detection. |
| **Context** | Real. Resolves canonical load order, file costs and complete cached MCP schema measurements without numeric fallbacks. |
| **MCP Servers** | Real. Static sanitized inventory, explicit bounded health checks, and writer-backed per-runner toggles. |
| **Chat** | Real. Durable resumable CLI sessions, permission decisions, structured tool rendering and Bundle attachment. |
| **Projects** | Real. Discovers candidate repos, opt-in registration. |
| **Bundles** | Real. Revisioned editor, immutable chat attachments and fail-closed Terminal handoff. |
| **Inspiration** | Real. Content-addressed media store, auto-tagging, collection-scoped `aviary-media` server. |
| **Home** | Real. Reports the live index. |
| **Settings** | Real. Durable preferences, local diagnostics and signed-update controls. |

**Nothing in the app is mock data.** That was the bar for the alpha and it is
the bar for every release after.

---

## P1 — Make the alpha survivable

*Goal: an alpha tester can install it, keep it, and tell you what broke.*

### 1. Auto-updater — implementation complete, delivery proof outstanding

The Tauri updater, fixed release channel, signed manifest verification, durable
UI state and install/relaunch recovery are implemented. Release CI refuses a
private update channel, mismatched versions, missing signing secrets, an
unverifiable archive, or a public URL that returns different bytes.

The repository is still private and has no post-alpha release. The original
done condition remains open: install the signed bootstrap build, publish a newer
version, and complete the in-app update without visiting GitHub.

### 2. Developer ID signing + notarisation — automation complete, Apple proof outstanding

CI imports an ephemeral Developer ID identity, builds the app and all helpers as
universal binaries, notarizes and staples the app and final DMG, downloads the
draft artifacts back, applies quarantine, and requires Gatekeeper acceptance
before publication. This machine has no Developer ID identity, so the final
downloaded-DMG proof cannot be claimed yet.

### 3. Crash and error reporting — implemented

Rust panics and explicit frontend failures go to bounded rotating local logs.
The Error Boundary and Settings build a redacted, bounded, user-copyable report.
Nothing is uploaded automatically and IPC arguments are never logged.

### 4. File watching — implemented

A native watcher maps real and symlinked paths back to affected scopes,
debounces event storms with a maximum deadline, refreshes targeted snapshots,
and publishes live library updates. Tests exercise atomic replacement, shared
targets, saturated queues and real filesystem notification.

---

## P2 — Chat becomes the reason to stay

*Goal: Aviary is where you run agents, not just where you configure them.*

### 5. Session persistence and resume — implemented

Sessions, turns and normalized events live in `data.db`. Session creation and a
queued turn commit before runner startup; runner session identity binds once,
survives relaunch and resumes through the CLI's own protocol. Startup
reconciliation interrupts abandoned work without replaying it. The frontend
retains the live channel and reconciles against durable state when events race
the initial receipt or the channel disappears.

### 6. Permission approval UI — implemented

Claude and Codex permission requests normalize into durable typed forms. The UI
renders runner-supported choices, validates answers, guards same-tick duplicate
submissions, and supports interruption. Dangerous up-front modes remain behind
the explicit preference.

### 7. Tool call rendering — implemented

Tool lifecycle events stay grouped by call ID, with separate summaries and
bounded detail for reads, commands, edits, diffs, completion, failure and
interruption. Unknown payloads remain sanitized structured data rather than
fabricated presentation.

---

## P3 — Context becomes trustworthy

*Goal: the "why did the agent do that?" screen answers the whole question.*

### 8. MCP stdio health checks — implemented

Static scans never start a process or contact a network. A separately warned,
explicit check performs bounded initialization and paginated tool listing,
isolates and kills timed-out process groups, distinguishes authentication and
policy states, and reports runner-provided servers without pretending they came
from a file declaration. Serialized inventory excludes commands, arguments,
URLs, headers, environment values, schemas and raw probe errors.

### 9. Measure MCP tool definitions — implemented when knowable

A complete health result stores tool count and an o200k estimate of the exact
schemas returned, keyed by runner, canonical directory, declaration revision
and expiry. Context includes it only when the result is complete and the runner
loads it. Before a check, after expiry, or after a partial result, the layer is
explicitly unknown or incomplete; the built-in system prompt remains the one
intrinsically unavailable cost.

### 10. Per-runner MCP toggles — implemented

The grid exposes only mutations that the owning runner configuration can
represent. Every write is atomic, snapshotted, conflict-checked and
mode-preserving through `writer.rs`; comments, unknown fields and secrets round
trip. Managed, runner-provided, invalid and policy-controlled declarations show
why they are not writable. Shared project files require confirmation.

---

## P4 — Context Bundles

*Goal: the idea that makes Aviary a product rather than five good tabs.*

A bundle is a saved composition of real target identities. Exact runner
semantics remain a hard boundary: an unsupported member blocks execution with a
reason instead of being silently ignored.

### 11. Bundle model and editor — implemented

Durable versioned `bundle` and ordered `bundle_member` tables store opaque
targets and snapshot labels. The two-pane editor composes registered projects,
live library entries, MCP declarations and media collections, enforces
cardinality and roles, preserves missing identities, and uses compare-and-set
revisions for update/delete.

### 12. Attach to chat — implemented with explicit compatibility checks

Attachment preflight resolves the saved revision again, requires exactly one
canonical working directory, and verifies that every member can be represented
by both the selected runner and the current chat adapters. Runner, model and cwd
are locked; the prompt is editable prefill. The secret-free attachment snapshot
and first turn commit atomically and survive Bundle deletion.

Globally available skills and agents are supported. First-turn skill invocation,
primary-agent selection, supplemental memory, isolated MCP selection and scoped
media retrieval currently fail closed because the cross-runner execution
channel for them has not been proven. They remain valid saved compositions and
are never presented as applied chat state.

### 13. Launch into a terminal — implemented with the same boundary

A saved revision becomes an expiring, owner-only one-use descriptor. The shell
file contains only quoted paths to `aviary-launch` and the descriptor; typed
runner arguments never become shell text. The helper validates ownership,
permissions, hashes, cwd identity, expiry and replay state before spawning the
real CLI. Unsupported composition semantics are rejected before Terminal opens.

### 14. `aviary-library` MCP server — implemented

The bundled read-only server exposes `search_library`, `get_skill`,
`get_prompt`, `get_agent` and `list_bundles` through the shared bounded MCP
runtime. It uses opaque IDs, strict schemas, UTF-8-safe limits, fresh target
resolution and read-only database access. The Library UI derives a copyable
registration command from the verified installed sibling path.

---

## Later — deliberately unscheduled

Real ideas, no date. Listed so they stop competing for attention.

- **Windows and Linux.** Tauri keeps the door open; nothing else does. Only
  worth it with users asking.
- **iOS companion.** Designed (Figma pages `08 · iOS`, `09 · Material 3`) and a
  SwiftUI prototype exists in `ios/`, but there is no sync story, and without one
  a mobile client has nothing to show. Blocked on a reason, not on effort.
- **Version history UI.** Every write is already snapshotted to
  `~/.aviary/history`; nothing surfaces them yet.
- **Additional runners.** The `Provider` trait exists so a third runner is one
  file. Waiting for one worth adding.
- **Perceptual-hash dedupe for media.** Content addressing catches identical
  bytes; it will not catch a re-export of the same image.

---

## Non-goals

Stated so they stop being re-litigated.

- **Not a team product.** Single user, local-first, no accounts, no sync server.
- **Not a replacement for the CLIs.** Aviary orchestrates them; it does not fork
  them or reimplement their behaviour.
- **Not a general-purpose IDE.** Editing is scoped to config, prompt and memory
  files.
- **No telemetry.** Release builds contact the public GitHub update feed on
  launch and when the user asks for a check. Diagnostics stay local and are
  copied by the user, never uploaded.

---

## How this list is maintained

An item moves out of the roadmap when it is real on a real machine — not when
the code merges. "Real" means there is a test that exercises it against actual
files, or a verified end-to-end run, and no placeholder data anywhere in the
path. See [`CONTRIBUTING.md`](../CONTRIBUTING.md).
