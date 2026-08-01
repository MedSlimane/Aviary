<div align="center">

<img src="docs/images/mark.svg" width="84" alt="Aviary" />

# Aviary

**Your agents live in many places. Aviary is the one place you shape them.**

A local-first desktop app for managing personal AI agents — the prompts, skills,
subagents, MCP servers, context and media that Claude Code, Codex and whatever
comes next all read from.

<br />

![Aviary](docs/images/cover.png)

<br />

[![Tauri](https://img.shields.io/badge/Tauri-2-24C8DB?style=flat-square&logo=tauri&logoColor=white)](https://tauri.app)
[![React](https://img.shields.io/badge/React-19-61DAFB?style=flat-square&logo=react&logoColor=black)](https://react.dev)
[![Rust](https://img.shields.io/badge/Rust-1.94-CE422B?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![TypeScript](https://img.shields.io/badge/TypeScript-5.8-3178C6?style=flat-square&logo=typescript&logoColor=white)](https://www.typescriptlang.org)
[![Release](https://img.shields.io/github/v/release/MedSlimane/Aviary?include_prereleases&style=flat-square&color=A78BFA)](https://github.com/MedSlimane/Aviary/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/MedSlimane/Aviary/ci.yml?branch=main&style=flat-square)](https://github.com/MedSlimane/Aviary/actions)

</div>

<br />

---

## Why

Coding agents read their behaviour from files scattered across your disk —
`~/.claude/skills/`, `~/.codex/AGENTS.md`, per-project `CLAUDE.md`, MCP blocks in
three incompatible config formats, memory directories, plugin caches. The
configuration surface is large, powerful, and completely unmanaged.

Aviary is a fast, native home for that surface — **and a place to actually use
it**, not merely curate it.

<br />

## The library

Every prompt, skill, subagent and command across every runner, in one searchable
index. Grouped by the pack it ships with, so `superpowers`' fifteen skills
collapse into one row instead of scattering through the list.

![Library](docs/images/library.png)

Skills are not owned by a runner. They live in a shared pool and are *symlinked*
into each runner's directory — so "enabled for Claude Code" is the presence of a
symlink, not metadata. Aviary deduplicates by canonical path and reports every
runner that links to an entry.

Select one and it opens beside the list: frontmatter, rendered markdown, the
symlink target, and a real token count from `tiktoken` rather than a byte
heuristic.

<br />

## Context, finally visible

The answer to *"why did the agent do that?"* is usually "something you couldn't
see was in its context." This is the screen that shows it: every instruction,
skill and MCP tool definition that loads, in resolution order, with its real
token cost against the window.

![Context](docs/images/context.png)

<br />

## Chat that runs the real thing

Agentic turns are driven by the runner's own CLI — `claude -p --output-format
stream-json`, `codex exec --json` — so tools, MCP, skills, permissions and
session resume all come for free. Edit a skill in Aviary and it applies on the
very next turn, because it is the same file.

![Chat](docs/images/chat.png)

<br />

## Everything is one keystroke away

`⌘K` searches the whole library, every context bundle, and every action.

![Command palette](docs/images/palette.png)

<br />

## Context Bundles

The idea that ties the app together. A bundle is a saved composition:

```
Bundle "Frontend Review"
  ├ prompt      review-checklist.md
  ├ skills      design-taste-frontend, web-design-guidelines
  ├ agents      Explore, code-reviewer
  ├ mcp         figma, playwright
  ├ memory      scope: ~/work/dashboard
  └ media       collection: "UI references / dark"
```

Attach one to a chat, or launch it into a terminal session.

<br />

## Aviary serves, it doesn't just show

Two local MCP servers expose the library back to your agents, so anything you
curate is reachable from inside Aviary *or* a bare terminal:

| Server | Tools |
|---|---|
| `aviary-library` | `search_library`, `get_skill`, `get_prompt`, `get_agent`, `list_bundles` |
| `aviary-media` | `search_media`, `get_media`, `list_collections` |

This is what makes an inspiration board useful rather than decorative — ask for
"that grainy teal gradient from my references" and the agent gets the file.

<br />

## Architecture

```
src-tauri/src/
├─ providers/      one file per runner — the blast radius for format drift
│  ├─ claude_code.rs
│  └─ codex.rs
├─ library.rs      index assembly, registered projects
├─ context.rs      resolves the instruction stack for (runner, cwd)
├─ mcp.rs          MCP server discovery across user/plugin/project configs
├─ media.rs        content-addressed media store, thumbnails, auto-tagging
├─ mcp_media.rs    JSON-RPC for the aviary-media MCP server
├─ bin/            aviary_media.rs — the MCP server binary (stdio)
├─ runner.rs       CLI supervisor, NDJSON → Tauri channel
├─ models.rs       model + effort discovery, per runner
├─ store.rs        SQLite — data.db (durable) and cache.db (disposable)
├─ writer.rs       atomic writes, snapshots, conflict detection
└─ tokens.rs       tiktoken (o200k_base)

src/
├─ views/          home · chat · library · projects · mcp · context · inspiration · settings
├─ components/     rail, title bar, shared screen parts
├─ lib/            api (the only IPC boundary), theme, motion, notify
└─ index.css       design tokens → shadcn token bridge
```

**Files are the source of truth.** The index is a disposable cache — delete it
and nothing is lost but a rescan. Writes are atomic, snapshotted to
`~/.aviary/history` beforehand, and refused if the file changed underneath you.

**Performance is a contract, not an aspiration.** No file I/O or parsing on the
UI thread. Every list virtualised. Live `backdrop-filter` only on small,
transient surfaces — card "glass" is a pre-rendered texture. Cold start under
400 ms, idle under 120 MB.

<br />

## Design system

Five colour modes, all driven by the same 24 semantic variables. The Figma file
is the source; `src/index.css` aliases shadcn's token names onto them, so every
shadcn component inherits the design system rather than being restyled.

| Mode | |
|---|---|
| **Dark** | near-black working surface, the default |
| **Light** | neutral daylight |
| **Aurora** | violet-cast dark |
| **Ember** | warm dark |
| **Paper** | warm cream light |

Icons are [HugeIcons](https://hugeicons.com). Type is Inter and Geist Mono.

<br />

## Getting started

```bash
# Requires Rust and Bun
cd aviary
bun install
bun run tauri dev
```

```bash
bunx tsc --noEmit                  # typecheck the frontend
cd src-tauri && cargo test --lib   # 28 tests, against your real machine
./scripts/make-dmg.sh              # universal DMG — see docs/RELEASING.md
```

<br />

## Status

**[`v0.1.0-alpha.1`](https://github.com/MedSlimane/Aviary/releases/tag/v0.1.0-alpha.1)** — universal macOS DMG. Ad-hoc signed, not notarised: right-click → Open on first launch.

Every surface below reads your real machine. **There is no mock data in the app.**

| | |
|---|---|
| Library — discovery, dedupe, editor with write safety | ✅ |
| Projects — auto-discovery, opt-in tracking | ✅ |
| MCP servers — discovery across user, plugin and project configs | ✅ |
| Chat — drives the real CLI, model and effort discovered | ✅ |
| Context — resolved instruction stack with real token costs | ✅ |
| Inspiration — content-addressed media board, auto-tagging | ✅ |
| `aviary-media` MCP server | ✅ |
| SQLite store + scan cache (101 ms → 1 ms) | ✅ |
| Auto-updater · notarisation · file watching | ⏳ P1 |
| Chat session persistence · permission approval UI | ⏳ P2 |
| MCP health checks · tool-definition token costs | ⏳ P3 |
| Context Bundles · `aviary-library` MCP server | ⏳ P4 |

The full plan is in **[`docs/ROADMAP.md`](docs/ROADMAP.md)**.

<br />

## Repository

```
aviary/        the app
ios/           SwiftUI companion prototype — designed, not shipped
assets/        brand, textures, generated floral photography, tokens
docs/          architecture, roadmap, release runbook, design spec
```

| Document | |
|---|---|
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | How it is put together, and why |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | What ships next, and what is deliberately not built |
| [`docs/RELEASING.md`](docs/RELEASING.md) | Build runbook — two non-obvious traps |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | Setup, and what gets a change rejected |
| [`CLAUDE.md`](CLAUDE.md) | Rules for agents working in this repo (`AGENTS.md` symlinks here) |
| [design spec](docs/superpowers/specs/2026-07-28-aviary-design.md) | Original rationale, risks and phasing |

<br />

---

<div align="center">
<sub><b>Aviary</b> is a placeholder name and mark.</sub>
</div>
