# Contributing

Aviary is a single-developer project at alpha. Issues and PRs are welcome; this
file exists so a change has a fair chance of being merged rather than rewritten.

Start with [`CLAUDE.md`](CLAUDE.md) — it holds the rules that actually govern
this codebase, and applies to humans exactly as much as to agents.

---

## Setup

```bash
git clone git@github.com:MedSlimane/Aviary.git
cd Aviary/aviary
bun install
bun run tauri dev
```

Requires Rust (stable) and [Bun](https://bun.sh). macOS only for now.

---

## Before you open a PR

```bash
bunx tsc --noEmit                        # from aviary/
cd src-tauri && cargo test --lib         # 28 tests
```

Then **run the app and drive the flow you changed.** Compiling is not evidence.

---

## What gets a change rejected

These are not style preferences — each one has cost this project real time.

**Mock data in a shipped surface.** If a feature is not real, it does not ship.
Render an empty state that says what is missing instead. Sample data belongs in
tests and previews.

**Invented numbers.** If the code cannot know a value, the UI says so rather
than showing a plausible-looking figure. See how `context.rs` handles
unmeasurable layers.

**Writes that bypass `writer.rs`.** Every write to runner config is atomic,
snapshotted and conflict-checked. There is no second path.

**Weakened assertions to make CI pass.** The suite runs against the real machine
on purpose. If a test needs something CI lacks, make it *skip with a printed
reason* — do not lower the bar for everyone.

**Hardcoded model lists.** Models and effort levels are discovered from the CLI
at runtime.

**Comments that restate the code.** Explain *why*, or delete it.

---

## Tests

The suite deliberately reads your actual `~/.claude` and `~/.codex` rather than
fixtures. Fixtures would have hidden every interesting bug here — the symlink
dedupe, the plugin cache duplication, the per-runner config shapes.

So a test may find nothing on a given machine. Handle that by skipping loudly:

```rust
let Some(skill) = snap.entries.iter().find(|e| /* … */) else {
    eprintln!("skipped: no skills installed on this machine");
    return;
};
```

Tests that mutate durable state must clean up after themselves —
`media::tests::imports_dedupes_and_searches` imports a generated image, asserts
against it, then removes it so the contributor's own board is untouched.

---

## Commits

Imperative subject, then *why* in the body. Read `git log` for the register: the
messages explain the problem being solved and what was learned, not which files
moved.

```
fix: green CI, and sign the bundle so macOS does not call it damaged

`lipo` strips the linker's ad-hoc signature, so the universal bundle shipped
completely unsigned — macOS reports that as damaged rather than merely
untrusted, and no amount of right-click-Open recovers it.
```

---

## Scope

Check [`ROADMAP.md`](docs/ROADMAP.md) before building something large. The
ordering there is deliberate — Context Bundles are last because they compose
everything else, and building them earlier would mean composing mock data.

The non-goals are also listed there. Team features, sync servers, telemetry and
IDE ambitions are out of scope by decision, not by omission.
