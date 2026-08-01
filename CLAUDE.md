# Aviary

A local-first macOS app for managing what coding agents read — skills, prompts,
subagents, MCP servers, instruction files and media across Claude Code and Codex.

**Tauri v2 · Rust core · React + TypeScript shell.** The app lives in `aviary/`.

Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) before changing anything
structural. [`docs/ROADMAP.md`](docs/ROADMAP.md) says what is deliberately not
built yet.

---

## Commands

```bash
cd aviary
bun install
bun run tauri dev            # run the app

bunx tsc --noEmit            # typecheck the frontend
cd src-tauri && cargo test --lib      # 28 tests, run against your real machine
cd src-tauri && cargo build --bins    # app + aviary-media

./scripts/make-dmg.sh        # universal DMG (see docs/RELEASING.md first)
```

---

## The rules that matter

### 1. Never ship mock data

This is the one that gets violated by accident. A screen with convincing fake
data is worse than an absent screen, because it spends trust the working parts
have to earn back — and it hides the fact that the feature was never built.

The alpha shipped only after Home, Settings and Inspiration were converted from
hardcoded arrays to real queries. If you cannot make a surface real, leave it
out, or render an empty state that says what is missing.

Sample data is fine in `#Preview` blocks and tests. Nowhere else.

### 2. Never invent a number

`context.rs` exists to answer *"why did the agent behave that way?"* — its whole
value is being true. Anything whose cost cannot be read from disk is reported
with `measured: false` and **no figure**, never a plausible guess.

Same discipline everywhere: if the code cannot know it, the UI says so. Do not
add a fallback that looks like data.

### 3. All writes to runner config go through `writer.rs`

Atomic write, snapshot to `~/.aviary/history` first, refuse on hash mismatch.
There is no second way to write. Corrupting a working agent setup is the worst
thing this app can do.

### 4. Nothing expensive on the UI thread

Filesystem walks, tokenising, hashing, subprocesses — all `spawn_blocking`.

### 5. `src/lib/api.ts` is the only IPC boundary

Every `invoke` gets a typed wrapper there that normalises Rust's `snake_case`
into `camelCase`. If you are adding a command and not editing that file, stop.

### 6. Two databases, and the split is load-bearing

`data.db` is durable (preferences, projects, media, collections). `cache.db` is
disposable (scan snapshots, token counts, thumbnails). Deleting `cache.db` must
never lose anything. Do not put user data in it, and do not put derived data in
`data.db`.

---

## What is easy to get wrong here

**Skills are symlinks.** They live in a shared pool and are linked into each
runner's directory, so "enabled for Claude Code" is the *presence of a symlink*,
not a flag. Entries dedupe by canonical path. `find` without `-L` will disagree
with Aviary's counts; Aviary is right.

**Tests run against the real machine on purpose.** Fixtures would have hidden
the symlink dedupe, the plugin cache duplication, and the per-runner config
shapes. When a test needs something the machine may not have, make it **skip
with a printed reason** — CI has no `~/.claude`, and an empty result there is
correct rather than broken. Never weaken an assertion to make CI pass.

**Model and effort levels are discovered, not hardcoded.** Parsed from the CLI's
`--help` and probed at runtime. Adding a hardcoded model list is a regression.

**Media is content-addressed.** sha256 of the bytes is the identity. Re-import
is a no-op, not a duplicate.

**`lipo` strips code signatures.** A universal build arrives unsigned unless
re-signed, and macOS calls an unsigned bundle *damaged* rather than merely
untrusted. `scripts/make-dmg.sh` handles this; do not bypass it.

---

## Style

Match the surrounding code — it is consistent, so this is easy.

**Comments explain *why*, never *what*.** The codebase's comments carry the
findings that justify a design: why dedupe is by canonical path, why skills count
frontmatter only, why the databases are split. A comment restating the line below
it is noise; delete it. If a decision was non-obvious, say what you learned.

Rust: modules carry a `//!` header explaining their reason to exist. Prefer
returning `Result<_, String>` for Tauri commands.

TypeScript: no `any`. Views own their data fetching with `useState`/`useEffect`
and the existing `Skeleton` + `notify` patterns — copy `views/mcp.tsx`.

Commits: imperative subject, then *why* in the body. Look at `git log` — they
explain the problem being solved, not the files touched.

---

## Verifying

A change is not done because it compiles. Run the app and drive the affected
flow, or write a test that exercises real files. `cargo test --lib` and
`bunx tsc --noEmit` are the floor, not the bar.

For UI, launch with `bun run tauri dev` and look at it. Do not synthesise clicks
into the user's desktop — automation lands in whatever window is frontmost, which
may be their browser.
