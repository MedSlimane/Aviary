# Aviary — Design Spec

**Date:** 2026-07-28
**Status:** Approved (design), pending implementation plan
**Codename:** Aviary *(placeholder — name and mark are provisional)*

**Figma:** [Aviary — Design System & Screens](https://www.figma.com/design/odzFIgPkY0H65N8dM8buq8)

| Page | Contents |
|---|---|
| 00 · Cover | Floral cover art + wordmark lockup |
| 01 · Foundations | 4 gradient paint styles, 24 color variables × 5 modes (swatch boards for Dark and Light), scale variables, 12-step type ramp, elevation styles |
| 02 · Components | 39 components and variant sets in labelled sections — Controls (Button, Chip, NavItem, Toggle, IconButton), Inputs (SearchField, Selector, Tab, Kbd), Rows (LibraryRow, ServerRow, ContextLayer, PaletteRow), Cards (BundleCard, PanelCard), Feedback (Banner ×3 tones, SectionLabel, SmallButton), Glass surfaces (ThinkingPill, GlassChip, SuggestionChip, RecordingPill, StopButton), plus 16 HugeIcons — all with TEXT / INSTANCE_SWAP properties |
| 03 · Screens | Library, Chat, Chat (empty), Chat (voice input), MCP Servers, Context, Inspiration, Command Palette, Skill Editor |
| 04 · Brand | Mark (solid / iridescent / on-light) + macOS app icon |
| 05 · Themes | The Library screen rendered in all five color modes |

**Color modes:** `Dark`, `Light`, `Aurora` (violet-cast dark), `Ember` (warm
dark), `Paper` (warm light). Every surface, border and text color is variable-
bound, so a screen re-themes by switching one mode on the frame.

**Assets:** `assets/` — dependency-free SVG textures, floral art, brand marks,
and `tokens.json`.

---

## 1. Vision

> Your agents live in many places. Aviary is the one place you shape them.

Coding agents — Claude Code, Codex, and whatever follows — read their behavior from
scattered files on disk: `~/.claude/skills/`, `~/.codex/AGENTS.md`, per-project
`CLAUDE.md`, MCP server blocks written in three incompatible config formats, memory
directories, plugin caches. The configuration surface is large, powerful, and
completely unmanaged.

Aviary is a fast, native-feeling desktop home for that surface — **and a place to
actually use it**, not merely curate it.

### 1.1 The unifying idea: Context Bundles

A **Context Bundle** is a saved composition:

```
Bundle "Frontend Review"
  ├ prompt      review-checklist.md
  ├ skills      design-taste-frontend, web-design-guidelines
  ├ agents      Explore, code-reviewer
  ├ mcp         figma, playwright
  ├ memory      scope: ~/work/dashboard
  └ media       collection: "UI references / dark"
```

A bundle can be **attached to a chat** inside Aviary, or **launched into a real CLI
session** in a terminal. Bundles are what turn five well-made tabs into one product.
Without them, Aviary is a file browser with good taste.

### 1.2 The inversion: Aviary serves, it doesn't just show

Aviary hosts two local MCP servers so that *any* agent — running inside Aviary or in
a bare terminal — can query the library:

| Server | Tools |
|---|---|
| `aviary-library` | `search_library`, `get_skill`, `get_prompt`, `get_agent`, `list_bundles` |
| `aviary-media` | `search_media`, `get_media`, `list_collections` |

This is what makes "AI-selected access to inspiration and media" real rather than
aspirational. The media board is not a gallery you scroll — it is a retrieval
surface your agents call into. Ask Claude Code for "that grainy teal gradient from
my references" and it gets the actual file.

### 1.3 Non-goals

- Not a team product. Single user, local-first, no accounts, no sync server.
- Not a replacement for the CLIs. Aviary orchestrates them; it does not fork them.
- Not a general-purpose IDE. Editing is scoped to config, prompt, and memory files.
- Not cross-platform at v1. macOS first; the Tauri stack keeps the door open.

---

## 2. Users and success criteria

**User:** one power user running multiple agent CLIs daily, with dozens of skills,
many MCP servers, and instruction files spread across personal and project scopes.

Aviary succeeds when:

1. Finding any prompt/skill/agent across every runner takes **under 3 seconds** from
   cold app launch, via `⌘K`.
2. The answer to *"why did the agent behave that way?"* is one click — the resolved
   context viewer shows the exact instruction stack in load order.
3. Adding an MCP server to three runners is one toggle grid, not three file edits.
4. Editing a skill in Aviary changes agent behavior on the **very next turn**, with
   no sync, export, or restart step.
5. The app is invisible in Activity Monitor: <120 MB idle, <400 ms cold start.

---

## 3. Information architecture

```
┌ Rail ─────┬─────────────────────────────────────────────┐
│ ✳ Aviary  │  ⌘K command palette (frosted glass overlay) │
│           │                                             │
│ ◇ Home    │   [ iridescent gradient hero strip ]        │
│ ✧ Chat    │                                             │
│ ◈ Library │   content — virtualized, dark dotted canvas │
│ ⬡ MCP     │                                             │
│ ◐ Context │                                             │
│ ✿ Inspire │                                             │
│ ⚙ Settings│                                             │
└───────────┴─────────────────────────────────────────────┘
```

### Home
Recents, currently running sessions, quick-launch bundles, and a health glance
(MCP servers down, files changed on disk outside Aviary, index staleness).

### Chat
Sessions across all runners in one list. Each session records which runner, model,
and bundle produced it. Two engines (§4.4): CLI-backed for agentic work, direct API
for lightweight asks.

### Library
Unified registry of **Prompts · Skills · Agents · Commands**, filterable by kind and
by runner. Split view: virtualized list, then editor.

- CodeMirror 6 editor with frontmatter validation against each runner's schema
- **Diff vs. disk** banner when the file changed underneath you
- Version history from the local snapshot store
- "Used by" backlinks — which bundles and which runners reference this entry

### MCP
Every MCP server discovered across every runner, in one grid.

- Rows are servers, columns are runners; cells are toggles
- Health via a real stdio `initialize` handshake, showing the tool count returned
- Env/args editor with secret masking, and a warning when a secret is stored in
  plaintext in a config file
- Install from a curated registry, writing correct syntax per runner

### Context
The instruction and memory hierarchy — global, user, project, local.

Its most valuable view: **"What's actually loaded."** Pick a runner and a working
directory; Aviary renders the resolved stack in load order, with an estimated token
cost per layer and a total. This is the debugging tool that does not currently exist
anywhere.

### Inspire
Media and link board. Drag in images, clips, and URLs; auto-tag by dominant color,
orientation, and detected content; organize into collections. Masonry grid with
GPU-friendly virtualization. Feeds `aviary-media`.

---

## 4. Architecture

**Tauri v2 · Rust core · React + TypeScript shell.**

Rust was chosen for the core because the expensive work — walking thousands of
files, watching for changes, parsing frontmatter, hashing media, running FTS — must
never touch the UI thread. React handles only presentation.

### 4.1 Module map

```
src-tauri/
├ providers/         trait Provider — one file per runner, the blast radius
│   ├ mod.rs         paths(), parse(), write(), capabilities(), mcp_config()
│   ├ claude_code.rs
│   └ codex.rs
├ indexer/           notify watcher → debounce → parse → SQLite FTS5
├ store/             SQLite: index, tags, collections, sessions, bundles
├ runner/
│   ├ cli.rs         process supervisor, NDJSON stream → Tauri channel
│   ├ api.rs         Anthropic / OpenAI clients for lightweight chats
│   └ permissions.rs tool-approval bridge
├ media/             import, thumbnails, perceptual-hash dedupe, EXIF
├ mcp_hub/           config read/write, stdio health check
└ mcp_serve/         hosts aviary-library and aviary-media

src/
├ routes/            TanStack Router
├ features/          library · chat · mcp · context · inspire
├ design/            tokens, motion primitives, glass + gradient surfaces
└ lib/               Tauri IPC hooks, TanStack Query cache
```

### 4.2 Data model — files are truth

**Rule: the SQLite database is a disposable cache.** Deleting it must cost nothing
but a re-index. Every user-visible entity is a real file on disk, in the format its
runner already expects.

The index stores: path, runner, kind, name, description, frontmatter blob, content
hash, mtime, FTS tokens, plus Aviary-only metadata (tags, favorites, bundle
membership) keyed by **content-addressed identity** so metadata survives a file
move.

Aviary-only data that has no home in a runner's format lives in
`~/.aviary/` — bundles, collections, tags, media, snapshots.

### 4.3 Write safety

Writing to live agent configuration is the highest-risk thing this app does.

1. **Atomic writes** — write to a temp file in the same directory, `fsync`, rename.
2. **Snapshot before every write** — the prior content is copied to
   `~/.aviary/history/<hash>/<timestamp>` before the new content lands.
3. **Conflict detection** — a write is refused if the on-disk content hash no longer
   matches what the editor loaded. The user sees a diff and chooses.
4. **Never delete without confirmation**, and deletes move to `~/.aviary/trash`.

### 4.4 The two chat engines

| | CLI engine (default) | API engine |
|---|---|---|
| Invocation | `claude -p --output-format stream-json --verbose`, `codex exec --json` | Anthropic / OpenAI SDK |
| Gets | tools, MCP, skills, permissions, session resume — free | nothing but the model |
| Library edits apply | immediately, same files | not applicable |
| Latency | process spawn (~hundreds of ms) | ~200 ms |
| Used for | all agentic work | quick no-tool questions |

Both engines emit into the same normalized event stream, so the renderer does not
branch on engine:

```
SessionEvent = Started | Thinking | Text(delta) | ToolCall | ToolResult
             | PermissionRequest | TokenUsage | Finished | Error
```

**Permissions.** `stream-json` requires an explicit permission strategy. v1 ships
`plan` and `acceptEdits` modes plus a real approval UI driven by
`PermissionRequest` events. Aviary will not ship
`--dangerously-skip-permissions` as a default, and any path that bypasses approvals
must be an explicit, per-session, clearly-labeled user choice.

### 4.5 Performance contract

These are requirements, verified in P0, not aspirations:

- No file I/O, parsing, or hashing on the UI thread. Ever.
- Every list and grid virtualized. Library opens 5,000 entries in **<100 ms**.
- Live `backdrop-filter` is permitted **only** on small, transient surfaces —
  command palette, chat composer, status pills. Card and hero "glass" is a
  **pre-rendered gradient texture** plus a 1 px inner stroke. This is precisely why
  the asset pack exists: to buy the look without paying for runtime blur.
- Cold start **<400 ms**; idle RSS **<120 MB**; 120 fps scroll on ProMotion.
- Media thumbnails decoded off-thread and cached at fixed sizes.

---

## 5. Visual design

### 5.1 Direction

A quiet, neutral chrome carrying **iridescent gradient art** as card headers, hero
strips, empty states, and modal tops. The gradients are soft, heavily blurred, and
finely grained — pastel violet, blue, peach, coral, teal, pale gold. The working
surface underneath stays near-black with a barely-visible dot grid, so dense lists
and code remain readable across long sessions.

The vibe appears in **moments** — the palette, a card header, an empty state, the
agent "thinking" pills — never behind body text.

### 5.2 Surfaces

| Surface | Treatment |
|---|---|
| Canvas | near-black + tileable dot grid |
| Panel | flat elevated neutral, 1 px subtle border |
| Card header | pre-rendered gradient texture strip + grain |
| Hero / empty state | full gradient art |
| Palette, composer, pills | true frosted glass, live `backdrop-filter` |
| Modal | neutral body, gradient cap |

### 5.3 Asset pack

Generated as dependency-free SVG (layered gradients + `feTurbulence` grain):

- Four gradient families — **aurora** (violet/blue), **ember** (peach/coral),
  **tidal** (teal/gold), **dusk** (indigo/magenta) — each at hero and card sizes
- Tileable grain overlays (fine, coarse), dot grid, ambient mesh background
- Brand: mark (solid + gradient), wordmark with outlined letterforms, macOS icon
- `tokens.json` — gradient stops, neutral dark ramp, light ramp, radius scale, blur
  scale, shadows

### 5.4 Motion

Restrained and physical. Spring-based transitions on route change; gradient hero
strips parallax subtly on scroll; the "thinking" pills breathe. Everything respects
`prefers-reduced-motion`. No animation longer than 300 ms on an interactive path.

---

## 6. Risks

| Risk | Mitigation |
|---|---|
| Writing to live agent config corrupts a working setup | Atomic writes, snapshot-before-write, conflict detection, trash instead of delete (§4.3) |
| CLI tool-permission prompts block or hang a chat turn | Explicit permission modes + approval UI; timeouts surface as `Error` events, never silent hangs (§4.4) |
| Runner config formats drift between releases | All format knowledge confined to one file per runner behind the `Provider` trait; schema-version detection with a clear "unsupported format" state |
| Glass/gradient aesthetic tanks scroll performance | Pre-rendered textures instead of runtime blur; live blur restricted to small transient surfaces; 120 fps verified in P0 (§4.5) |
| Scope — five subsystems in one app | Strict phasing (§7); only P0 receives a detailed implementation plan |
| Secrets in MCP configs shown or logged | Masked in UI by default, never written to the index, redacted from all logs |

---

## 7. Phasing

| Phase | Scope | Estimate |
|---|---|---|
| **P0 Foundation** | Tauri shell, design system + tokens, `Provider` trait with Claude Code + Codex adapters, indexer + FTS5, Library browse/search/edit with write safety | ~2 weeks |
| **P1 MCP + Context** | Server grid, stdio health checks, per-runner toggles, env editor, resolved-context viewer with token costs | ~1.5 weeks |
| **P2 Chat** | CLI streaming engine, session resume, permission UI, normalized event stream, API lite path | ~2.5 weeks |
| **P3 Inspire** | Media import, masonry board, collections, auto-tagging, `aviary-media` MCP server | ~1.5 weeks |
| **P4 Bundles** | Context Bundles, attach-to-chat, launch-into-CLI, `aviary-library` MCP server, motion polish | ~1.5 weeks |

Only **P0** receives a detailed implementation plan at this time. Later phases
remain scoped intent and will be re-brainstormed as they are approached, since
earlier phases will change what the right answer is.

### 7.1 P0 definition of done

- App launches on macOS in <400 ms with the design system applied
- Claude Code and Codex skills, prompts, agents, and commands are discovered,
  indexed, and searchable via `⌘K`
- A skill can be edited and saved, with snapshot and conflict detection working
- Deleting `~/.aviary/index.db` causes a re-index and no data loss
- 5,000-entry library opens in <100 ms, verified with a generated fixture set

---

## 8. Open questions

Deferred deliberately; none block P0.

1. Should bundles be shareable as a single portable file? (P4)
2. Is a Windows/Linux build ever wanted, or is macOS-only acceptable forever?
3. Should Aviary manage *project-local* `.claude/` directories across many repos,
   or only the user-global scope? (Affects P1 context resolution scope.)
4. Final name and mark — Aviary is a placeholder.
