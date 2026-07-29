import type { Runner } from "@/lib/api";
import { AnthropicWhite } from "@/components/ui/svgs/anthropicWhite";
import { AnthropicBlack } from "@/components/ui/svgs/anthropicBlack";
import { Openai } from "@/components/ui/svgs/openai";
import { OpenaiDark } from "@/components/ui/svgs/openaiDark";

export const RUNNER_LAB: Record<Runner, string> = {
  "claude-code": "Anthropic",
  codex: "OpenAI",
};

/** The lab behind a runner. Marks come from the svgl registry, not hand-drawn. */
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
  return dark ? <Openai className={className} /> : <OpenaiDark className={className} />;
}
