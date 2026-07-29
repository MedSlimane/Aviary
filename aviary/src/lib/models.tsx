import type { Runner } from "@/lib/api";
import { AnthropicWhite } from "@/components/ui/svgs/anthropicWhite";
import { AnthropicBlack } from "@/components/ui/svgs/anthropicBlack";
import { Openai } from "@/components/ui/svgs/openai";
import { OpenaiDark } from "@/components/ui/svgs/openaiDark";

/**
 * Models offered per runner.
 *
 * `id` is passed straight to the CLI's --model flag. A null id means "leave
 * the flag off entirely" so the runner keeps whatever default the user has
 * configured — Aviary should not silently pin a model it was never asked to.
 */
export type Model = {
  id: string | null;
  label: string;
  note: string;
  tag?: string;
};

export const MODELS: Record<Runner, Model[]> = {
  "claude-code": [
    { id: null, label: "Default", note: "Whatever your CLI is set to" },
    { id: "claude-opus-4-8", label: "Opus 4.8", note: "Deepest reasoning", tag: "Capable" },
    { id: "claude-sonnet-5", label: "Sonnet 5", note: "Balanced speed and depth", tag: "Balanced" },
    { id: "claude-fable-5", label: "Fable 5", note: "Newest of the Claude 5 family" },
    { id: "claude-haiku-4-5-20251001", label: "Haiku 4.5", note: "Fastest, cheapest", tag: "Fast" },
  ],
  codex: [
    { id: null, label: "Default", note: "Whatever your config.toml is set to" },
    { id: "gpt-5.6-sol", label: "GPT-5.6 Sol", note: "Your current Codex default" },
    { id: "o3", label: "o3", note: "Reasoning-heavy" },
  ],
};

/** The lab behind a runner, for branding. */
export function LabMark({
  runner,
  className,
  dark = true,
}: {
  runner: Runner;
  className?: string;
  dark?: boolean;
}) {
  if (runner === "claude-code") {
    return dark ? (
      <AnthropicWhite className={className} />
    ) : (
      <AnthropicBlack className={className} />
    );
  }
  return dark ? (
    <Openai className={className} />
  ) : (
    <OpenaiDark className={className} />
  );
}

export const RUNNER_LAB: Record<Runner, string> = {
  "claude-code": "Anthropic",
  codex: "OpenAI",
};
