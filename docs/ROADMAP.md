# Roadmap

Where Aviary is, and what it needs next. Updated 2026-08-01, at `v0.1.0-alpha.1`.

The ordering principle: **nothing ships that pretends to work.** A screen with
convincing fake data is worse than an absent one, because it spends trust that
the working parts have to earn back. Everything below is ranked by whether it
unblocks the next thing, not by how interesting it is to build.

---

## Where we are

`v0.1.0-alpha.1` — [released](https://github.com/MedSlimane/Aviary/releases/tag/v0.1.0-alpha.1),
universal macOS DMG, ad-hoc signed, 28 tests green in CI.

| Surface | State |
|---|---|
| **Library** | Real. Discovery across both runners, dedupe by canonical path, editor with atomic writes + snapshots + conflict detection. |
| **Context** | Real. Resolves the instruction stack for a runner and directory, tokenised from the actual files. |
| **MCP Servers** | Real, read-only. Discovers servers across user config, plugins and projects. |
| **Chat** | Real. Drives the runner's own CLI; model and effort discovered from the machine. |
| **Projects** | Real. Discovers candidate repos, opt-in registration. |
| **Inspiration** | Real. Content-addressed media store, auto-tagging, `aviary-media` MCP server. |
| **Home** | Real. Reports the live index. |
| **Settings** | Real. Preferences persist to SQLite. |

**Nothing in the app is mock data.** That was the bar for the alpha and it is
the bar for every release after.

---

## P1 — Make the alpha survivable

*Goal: an alpha tester can install it, keep it, and tell you what broke.*

Without these, feedback is not actionable — you cannot ship a fix to anyone who
already installed, and you cannot tell a crash from a misunderstanding.

### 1. Auto-updater ▲ highest priority
Tauri's updater plugin, signed update manifests, a GitHub Releases feed. Today
an alpha tester who hits a bug is stuck on the broken build forever, which makes
every other item on this list worth less.

**Done when:** a tester on `0.1.0` is prompted for `0.1.1` and takes it without
visiting GitHub.

### 2. Developer ID signing + notarisation
The current build is ad-hoc signed. macOS cannot confirm who built it, so first
launch requires a right-click or an `xattr` incantation — a real drop-off point
for non-technical users.

Needs an Apple Developer account. `scripts/make-dmg.sh` already honours
`SIGN_IDENTITY`; the remaining work is notarisation and stapling in CI.

**Done when:** `spctl -a -t exec` accepts a freshly downloaded DMG.

### 3. Crash and error reporting
Currently a panic in Rust or an unhandled rejection in the webview is invisible
to you. At minimum: write to a rotating log under `~/.aviary/logs`, and surface
"something went wrong — copy diagnostics" in the error boundary.

Local-first: no automatic upload. The user copies and pastes.

### 4. File watching
The index is a cache and nothing invalidates it except the Rebuild button. Edit
a skill in your editor and Aviary shows stale content until you notice.

`notify` watcher → debounce → targeted re-scan of the changed root. The
`scan` cache table already carries `scanned_at`, so partial invalidation is a
matter of scoping, not schema.

**Done when:** touching `~/.claude/skills/foo/SKILL.md` updates the row without
a manual rebuild.

---

## P2 — Chat becomes the reason to stay

*Goal: Aviary is where you run agents, not just where you configure them.*

Chat works but forgets everything. That is the difference between a demo and a
tool you keep open.

### 5. Session persistence and resume
Conversations vanish on relaunch. Store turns in `data.db` (the schema has room:
this is a `session` and `turn` table alongside `project`), and resume a runner
session by its id so context is not rebuilt from scratch.

**Done when:** quitting mid-conversation and reopening continues the same
session, with the runner's own session id intact.

### 6. Permission approval UI
`stream-json` emits `PermissionRequest` events. Today the permission *mode* is
chosen up front and the app never mediates a decision. A real approval prompt is
what makes `manual` and `acceptEdits` usable — and what makes hiding
`bypassPermissions` behind a setting the right default rather than an annoyance.

### 7. Tool call rendering
Grouped tool calls exist; diffs, file reads and command output deserve real
presentation rather than a summary line.

---

## P3 — Context becomes trustworthy

*Goal: the "why did the agent do that?" screen answers the whole question.*

### 8. MCP stdio health checks
Perform a real `initialize` handshake, report the tool count each server
returns, and surface auth failures. Read-only discovery already works; this is
the difference between "declared" and "actually reachable".

### 9. Measure MCP tool definitions
The single largest unknown in Context. Tool schemas are often the biggest block
of context a runner loads, and Aviary currently reports them as unmeasurable
because their size is only knowable after a handshake. #8 unblocks this.

**Done when:** the Context total includes tool definitions, and the
"unmeasurable" count drops to one (the built-in system prompt).

### 10. Per-runner MCP toggles
Write config back — enable and disable a server per runner from one grid. This
is the first place Aviary *writes* MCP config, so it inherits the write-safety
rules: atomic, snapshotted, conflict-checked.

---

## P4 — Context Bundles

*Goal: the idea that makes Aviary a product rather than five good tabs.*

A bundle is a saved composition — prompt, skills, agents, MCP servers, memory
scope, media collection — that can be attached to a chat or launched into a
terminal session.

This is deliberately **last among the core work**, not first. A bundle is only
useful once the things it composes are all real and all writable; building it
earlier would mean composing mock data. Everything in P1–P3 is a prerequisite.

### 11. Bundle model and editor
`bundle` and `bundle_member` tables. Compose from the existing library index.

### 12. Attach to chat
Applying a bundle sets the runner, model, MCP servers and working directory for
a session.

### 13. Launch into a terminal
Generate the CLI invocation a bundle represents and hand it to a real terminal.
The inverse of the app being a wrapper: Aviary composes, the CLI runs.

### 14. `aviary-library` MCP server
The sibling of `aviary-media`, already proven. `search_library`, `get_skill`,
`get_prompt`, `get_agent`, `list_bundles` — this is what makes the library a
retrieval surface for agents rather than a browser for humans.

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
- **No telemetry.** Aviary makes no network requests of its own. Diagnostics are
  copied by the user, never uploaded.

---

## How this list is maintained

An item moves out of the roadmap when it is real on a real machine — not when
the code merges. "Real" means there is a test that exercises it against actual
files, or a verified end-to-end run, and no placeholder data anywhere in the
path. See [`CONTRIBUTING.md`](../CONTRIBUTING.md).
