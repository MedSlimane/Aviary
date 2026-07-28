import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { notify } from "@/lib/notify";
import {
  PageHeader,
  StaggerList,
  StaggerRow,
  StatusDot,
} from "@/components/screen-parts";

type Server = {
  name: string;
  command: string;
  tools: string;
  status: "ok" | "warn" | "error";
  claude: boolean;
  codex: boolean;
};

const INITIAL: Server[] = [
  { name: "figma", command: "npx -y figma-developer-mcp", tools: "14 tools", status: "ok", claude: true, codex: true },
  { name: "playwright", command: "npx @playwright/mcp@latest", tools: "21 tools", status: "ok", claude: true, codex: false },
  { name: "github", command: "docker run ghcr.io/github/github-mcp-server", tools: "33 tools", status: "ok", claude: true, codex: true },
  { name: "sqlite", command: "uvx mcp-server-sqlite --db ~/data.db", tools: "6 tools", status: "ok", claude: false, codex: true },
  { name: "memory", command: "npx -y @modelcontextprotocol/server-memory", tools: "9 tools", status: "ok", claude: true, codex: true },
  { name: "notion", command: "npx -y @notionhq/notion-mcp-server", tools: "auth expired", status: "warn", claude: true, codex: false },
];

export function McpView() {
  const [servers, setServers] = useState(INITIAL);

  const toggle = (name: string, runner: "claude" | "codex") => {
    setServers((prev) =>
      prev.map((s) => {
        if (s.name !== name) return s;
        const next = { ...s, [runner]: !s[runner] };
        const label = runner === "claude" ? "Claude Code" : "Codex";
        notify(`${s.name} ${next[runner] ? "enabled" : "disabled"} for ${label}`, {
          description: next[runner]
            ? "Config written — active on next session."
            : "Removed from the runner's MCP config.",
        });
        return next;
      }),
    );
  };

  const healthy = servers.filter((s) => s.status === "ok").length;

  return (
    <div className="flex flex-col gap-[18px] p-[26px]">
      <PageHeader
        title="MCP Servers"
        subtitle={`${servers.length} servers · ${healthy} healthy · ${servers.length - healthy} needs attention`}
        action={
          <Button
            size="sm"
            className="rounded-full"
            onClick={() =>
              notify("Add server", { description: "Pick from the registry or paste a command." })
            }
          >
            Add server
          </Button>
        }
      />

      {/* Column headers, aligned to the toggle columns below */}
      <div className="flex items-center gap-3 px-3.5">
        <div className="flex-1" />
        <span className="w-[52px] text-center text-[9px] font-semibold tracking-[0.08em] text-tertiary">
          CLAUDE
        </span>
        <span className="w-[52px] text-center text-[9px] font-semibold tracking-[0.08em] text-tertiary">
          CODEX
        </span>
        <span className="w-[86px]" />
      </div>

      <StaggerList className="space-y-1.5">
        {servers.map((s) => (
          <StaggerRow
            key={s.name}
            interactive={false}
            className="flex items-center gap-3 rounded-[10px] border border-border bg-card px-3.5 py-2.5 transition-colors hover:border-border-strong"
          >
            <StatusDot status={s.status} />
            <div className="min-w-0 flex-1 space-y-0.5">
              <p className="truncate text-[13px] font-medium">{s.name}</p>
              <p className="truncate font-mono text-[11px] text-tertiary">
                {s.command}
              </p>
            </div>
            <div className="flex w-[52px] justify-center">
              <Switch
                checked={s.claude}
                onCheckedChange={() => toggle(s.name, "claude")}
                aria-label={`${s.name} for Claude Code`}
              />
            </div>
            <div className="flex w-[52px] justify-center">
              <Switch
                checked={s.codex}
                onCheckedChange={() => toggle(s.name, "codex")}
                aria-label={`${s.name} for Codex`}
              />
            </div>
            <span className="w-[86px] text-right text-[11px] text-muted-foreground">
              {s.tools}
            </span>
          </StaggerRow>
        ))}
      </StaggerList>
    </div>
  );
}
