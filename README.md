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
![Status](https://img.shields.io/badge/status-in%20development-A78BFA?style=flat-square)

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
src-tauri/
├─ providers/      trait Provider — one file per runner, the blast radius
│  ├─ claude_code.rs
│  └─ codex.rs
├─ library.rs      assembly, registered projects, settings
├─ indexer/        notify watcher → parse → SQLite FTS5          (next)
├─ runner/         CLI supervisor, NDJSON → Tauri channel        (planned)
└─ mcp_hub/        config read/write, health, aviary-* servers   (planned)

src/
├─ views/          home · chat · library · projects · mcp · context · inspiration · settings
├─ components/     rail, title bar, shared screen parts
├─ lib/            api, theme, motion, notify
└─ index.css       Figma tokens → shadcn tokens bridge
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
bun run build          # typecheck + build the frontend
cargo check            # from src-tauri/
cargo test -- --nocapture   # provider scan against your real machine
```

<br />

## Status

Honest state of things — the design is well ahead of the implementation.

| | |
|---|---|
| Design system + 9 screens in Figma | ✅ |
| Tauri shell, all 7 views, theming, ⌘K | ✅ |
| Real library discovery from disk | ✅ |
| Projects screen — auto-discovery, opt-in tracking | ✅ |
| Entry detail panel with markdown + tokens | ✅ |
| Indexer + FTS5 search | ⏳ next |
| Editor with write safety | ✅ |
| MCP config parsing | ✅ |
| MCP stdio health checks | ⏳ |
| Chat driving the real CLI | ⏳ |
| Context Bundles, `aviary-*` MCP servers | ⏳ |

Chat and Inspiration still render designed UI over sample data. Library,
Projects and MCP read your real machine.

<br />

## Repository

```
aviary/        the app
assets/        brand, textures, generated floral photography, tokens
docs/          design spec and images
```

The full design rationale — architecture, write-safety rules, performance
contract, risks and phasing — is in
[`docs/superpowers/specs/2026-07-28-aviary-design.md`](docs/superpowers/specs/2026-07-28-aviary-design.md).

<br />

---

<div align="center">
<sub><b>Aviary</b> is a placeholder name and mark.</sub>
</div>
