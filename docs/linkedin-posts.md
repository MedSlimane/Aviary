# LinkedIn posts

Visuals live in Figma → **07 · LinkedIn**. Three slides at 1200×1200, built in
the X-post layout: copy left, app window bleeding off the right edge, abstract
gradient background.

[Open in Figma](https://www.figma.com/design/odzFIgPkY0H65N8dM8buq8)

---

## Post 1 — the carousel

**Slides:** `LI 1 · Hook` → `LI 2 · Library` → `LI 3 · Context`

> My agent config was spread across six files I never opened.
>
> `~/.claude/skills/`, `~/.codex/AGENTS.md`, a per-project `CLAUDE.md`, MCP
> servers declared in three incompatible formats, plus a memory directory I'd
> genuinely forgotten about.
>
> Every one of them shapes how Claude Code and Codex behave. None of them were
> anywhere I could see.
>
> So I've been building Aviary — a local-first desktop app that puts all of it
> in one window.
>
> Three things I learned building it:
>
> **1. Skills aren't owned by a runner.** They sit in a shared pool and get
> symlinked into each runner's directory. "Enabled for Claude Code" isn't
> metadata you store — it's whether a symlink exists. Once I understood that,
> the whole data model got simpler: dedupe by real path, collect the runners
> that link to it. 20 of my skills are shared across both.
>
> **2. Nobody can see their own context.** By the time you type your first
> message, ~17k tokens are already loaded — system prompt, instruction files,
> active skills, MCP tool definitions, memory. That's 8.5% of the window gone
> before you've asked anything. So I built the screen that shows it, in
> resolution order, with real costs.
>
> **3. Files have to stay the source of truth.** The index is a disposable
> cache. Delete it, lose nothing but a rescan. Aviary edits the same files the
> CLIs read, so a change applies on the very next turn — no sync, no export,
> no drift.
>
> Still early. The library is real; chat and MCP are designed but not wired.
>
> Built with Tauri, React and Rust. Happy to go deeper on any of it.
>
> #AI #DeveloperTools #ClaudeCode #Rust #LocalFirst

---

## Post 2 — skills and context, standalone

**Visual:** `LI 2 · Library` (or 2 + 3 as a two-image post)

> "Why did the agent do that?"
>
> Nine times out of ten the answer is: something you couldn't see was in its
> context.
>
> Before you type a single word to Claude Code, you've typically spent ~17k
> tokens — the system prompt, your global `CLAUDE.md`, the project one, local
> overrides, every active skill, every MCP tool definition, and whatever memory
> files got picked up.
>
> That's 8.5% of a 200k window, gone, invisible.
>
> I've been building Aviary to make that surface visible and editable:
>
> → every skill, agent and prompt across runners in one index, grouped by the
> pack it ships with
> → the exact resolution order of what loads, with per-layer token cost
> → edits write to the same files the CLIs read, so they apply on the next turn
>
> The part that surprised me: my 45 personal skills were buried under ~70 plugin
> skills I'd never authored. Grouping by pack made my own work findable again —
> a five-line change that mattered more than any feature I'd planned.
>
> #AI #DeveloperTools #ClaudeCode #ContextEngineering

---

## Notes

- Slide 1 headline was originally *"You can finally see what your agent is
  doing"* — replaced, it under-sold the product as a log viewer rather than a
  home for everything.
- Keep the claim honest: the Library reads real data; Chat, MCP and Inspiration
  are designed UI over sample data. The status table in the README says the
  same, so the two don't contradict each other.
- Both posts lead with the problem, not the tool. The config-scattered-across-
  six-files opening is the part people recognise in their own setup.
