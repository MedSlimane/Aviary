import { useState } from "react";
import * as motionReact from "motion/react";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Button } from "@/components/ui/button";
import { notify } from "@/lib/notify";
import { THEMES, useTheme, type ThemeName } from "@/lib/theme";
import { PageHeader, SectionLabel } from "@/components/screen-parts";
import { cn } from "@/lib/utils";

const { motion } = motionReact;

const THEME_SWATCH: Record<ThemeName, string> = {
  dark: "linear-gradient(135deg, #0a0a0b, #232326)",
  light: "linear-gradient(135deg, #f4f4f5, #ffffff)",
  aurora: "linear-gradient(135deg, #0b0a12, #b49dff)",
  ember: "linear-gradient(135deg, #120c0a, #ffc9ac)",
  paper: "linear-gradient(135deg, #f5f1e8, #e2daca)",
};

function Card({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-3.5 rounded-[14px] border border-border bg-card p-5">
      <SectionLabel>{title}</SectionLabel>
      {children}
    </section>
  );
}

function Row({
  label,
  hint,
  control,
}: {
  label: string;
  hint: string;
  control: React.ReactNode;
}) {
  return (
    <div className="flex items-center gap-4">
      <div className="min-w-0 flex-1">
        <p className="text-[13px] font-medium">{label}</p>
        <p className="text-[11px] text-muted-foreground">{hint}</p>
      </div>
      {control}
    </div>
  );
}

export function SettingsView() {
  const { theme, setTheme } = useTheme();
  const [prefs, setPrefs] = useState({
    watch: true,
    snapshot: true,
    reducedMotion: false,
    telemetry: false,
  });

  const set = (key: keyof typeof prefs, label: string) => (v: boolean) => {
    setPrefs((p) => ({ ...p, [key]: v }));
    notify(`${label} ${v ? "on" : "off"}`);
  };

  return (
    <div className="flex flex-col gap-[18px] p-[26px]">
      <PageHeader
        title="Settings"
        subtitle="Local-first — nothing here leaves your machine"
      />

      <Card title="APPEARANCE">
        <div className="grid grid-cols-5 gap-3">
          {(Object.keys(THEMES) as ThemeName[]).map((t) => {
            const active = t === theme;
            return (
              <motion.button
                key={t}
                type="button"
                onClick={() => setTheme(t)}
                whileHover={{ y: -2 }}
                whileTap={{ scale: 0.97 }}
                transition={{ type: "spring", stiffness: 520, damping: 28 }}
                className={cn(
                  "av-hover-grad relative space-y-2 rounded-xl border p-2 text-left transition-colors",
                  active
                    ? "border-violet"
                    : "border-border hover:border-border-strong",
                )}
              >
                <div
                  className="h-14 w-full rounded-lg ring-1 ring-inset ring-white/[0.06]"
                  style={{ backgroundImage: THEME_SWATCH[t] }}
                />
                <p className="px-0.5 text-[11px] font-medium">
                  {THEMES[t].label}
                </p>
                {active && (
                  <motion.span
                    layoutId="theme-active"
                    className="absolute inset-0 rounded-xl ring-2 ring-inset ring-violet"
                    transition={{ type: "spring", stiffness: 480, damping: 34 }}
                  />
                )}
              </motion.button>
            );
          })}
        </div>
        <Row
          label="Reduce motion"
          hint="Disable springs and staggered reveals"
          control={
            <Switch
              checked={prefs.reducedMotion}
              onCheckedChange={set("reducedMotion", "Reduced motion")}
            />
          }
        />
      </Card>

      <Card title="RUNNERS">
        <Row
          label="Claude Code"
          hint="~/.claude"
          control={
            <span className="rounded-full border border-border bg-elevated px-2.5 py-1 text-[11px] text-muted-foreground">
              detected
            </span>
          }
        />
        <Row
          label="Codex"
          hint="~/.codex"
          control={
            <span className="rounded-full border border-border bg-elevated px-2.5 py-1 text-[11px] text-muted-foreground">
              detected
            </span>
          }
        />
      </Card>

      <Card title="FILES & SAFETY">
        <Row
          label="Watch for external changes"
          hint="Re-index when files change outside Aviary"
          control={
            <Switch
              checked={prefs.watch}
              onCheckedChange={set("watch", "File watching")}
            />
          }
        />
        <Row
          label="Snapshot before every write"
          hint="Keeps a copy in ~/.aviary/history so edits are reversible"
          control={
            <Switch
              checked={prefs.snapshot}
              onCheckedChange={set("snapshot", "Snapshots")}
            />
          }
        />
        <div className="space-y-2">
          <Label htmlFor="library-root" className="text-[13px]">
            Library root
          </Label>
          <Input
            id="library-root"
            defaultValue="~/.aviary"
            className="font-mono text-xs"
          />
        </div>
      </Card>

      <Card title="PRIVACY">
        <Row
          label="Anonymous telemetry"
          hint="Off by default. Never includes file contents."
          control={
            <Switch
              checked={prefs.telemetry}
              onCheckedChange={set("telemetry", "Telemetry")}
            />
          }
        />
        <div className="flex gap-2 pt-1">
          <Button
            variant="outline"
            size="sm"
            onClick={() => notify("Index rebuilt", { description: "1,284 entries in 82ms." })}
          >
            Rebuild index
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => notify("Opened ~/.aviary")}
          >
            Reveal data folder
          </Button>
        </div>
      </Card>
    </div>
  );
}
