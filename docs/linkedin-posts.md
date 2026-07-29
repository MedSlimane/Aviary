# LinkedIn posts

Visuals live in Figma → **07 · LinkedIn**. Three slides at 1200×1200 in the
X-post layout: copy left, real app window bleeding off the right edge, abstract
gradient background with Aurora/Dusk/Tidal glows.

[Open in Figma](https://www.figma.com/design/odzFIgPkY0H65N8dM8buq8)

---

## Post 1 — carousel

**Slides:** `LI 1 · Hook` → `LI 2 · Library` → `LI 3 · Context`

> Tired of re-managing your skills, context and MCP servers every time you
> switch agents?
>
> I've been building **Aviary** — a local-first desktop app that keeps all of it
> in one window, shared across every agent you run.
>
> Claude Code and Codex read their behaviour from files scattered across your
> disk: `~/.claude/skills/`, `~/.codex/AGENTS.md`, per-project `CLAUDE.md`, MCP
> servers declared in three incompatible formats, memory directories, plugin
> caches. Same concepts, different places, no shared view.
>
> Aviary indexes all of it. Every skill, agent, prompt and command across both
> runners in one searchable library, grouped by the pack it ships with. A
> resolution-order view of exactly what loads before your first message, with
> its real token cost. Chat that drives the runner's own CLI, so an edit applies
> on the very next turn — same files, no sync layer.
>
> Two things I got wrong before I got them right:
>
> Skills aren't owned by a runner. They live in a shared pool and get symlinked
> into each one, so "enabled for Claude Code" is a symlink, not metadata. 20 of
> mine are shared across both.
>
> And my 45 personal skills were buried under 70 plugin skills I never wrote.
> Grouping by pack made my own work findable again.
>
> Early days — the library reads real data, chat and MCP are designed but not
> wired yet.
>
> Tauri, React, Rust.
>
> #AI #DeveloperTools #ClaudeCode #Rust

---

## Post 2 — skills and context, standalone

**Visual:** `LI 2 · Library`, or 2 + 3 as a two-image post

> Tired of not knowing what's actually in your agent's context?
>
> I've been building **Aviary** to make that visible.
>
> By the time you send your first message to Claude Code, roughly 17,000 tokens
> are already loaded — system prompt, global instructions, project ones, local
> overrides, every active skill, every MCP tool definition, whatever memory got
> picked up. That's 8.5% of a 200k window, spent before you asked for anything,
> and nowhere you can see it.
>
> So Aviary shows it. Every layer in resolution order, with per-layer cost
> against the window. Switch runner or directory and it recomputes.
>
> The same index powers the library — every skill, agent and prompt across
> Claude Code and Codex, deduplicated by real path, so a skill symlinked into
> both appears once and reports both.
>
> Files stay the source of truth. The index is a disposable cache; edits write
> to the files the CLIs already read.
>
> #AI #DeveloperTools #ClaudeCode #ContextEngineering

---

## Notes

**Voice.** Open with a single "tired of…" question, then "I've been building
Aviary" and describe it. No numbered lists — the two lessons in post 1 stay as
prose, since a list reads as a template.

**Why the hook names switching.** "Tired of managing context" is true but vague;
*"every time you switch agents"* is the sharper pain and the one the product
actually solves.

**Keep the claims honest.** The Library reads real data. Chat, MCP and
Inspiration are designed UI over sample data. Post 1 says so, and the README's
status table says the same — the two must not contradict each other.

**Length.** Post 1 runs ~200 words, past LinkedIn's truncation point. The "see
more" break lands just after the Aviary description, which is a reasonable
place for it.
