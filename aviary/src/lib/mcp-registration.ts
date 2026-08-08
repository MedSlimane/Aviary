import type { McpRegistration } from "@/lib/api";

function shellWord(value: string) {
  return /^[A-Za-z0-9_./:@%+=,-]+$/.test(value)
    ? value
    : `'${value.split("'").join(`'"'"'`)}'`;
}

/** Formats the semantic descriptor as the Claude CLI's stdio registration. */
export function claudeMcpRegistrationCommand(registration: McpRegistration) {
  return [
    "claude",
    "mcp",
    "add",
    registration.name,
    "--",
    registration.command,
    ...registration.args,
  ]
    .map(shellWord)
    .join(" ");
}
