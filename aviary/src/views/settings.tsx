import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { homeDir } from "@tauri-apps/api/path";
import { scanLibrary, type LibrarySnapshot } from "@/lib/api";
import { useBoolPreference } from "@/lib/use-preference";
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

  // Persisted in SQLite, so a toggle survives relaunch. The previous version
  // held these in `useState` alone and silently reset on every window close.
  const [reducedMotion, setReducedMotion] = useBoolPreference("ui.reducedMotion");
  const [allowRiskyModes, setAllowRiskyModes] = useBoolPreference(
    "chat.allowRiskyPermissionModes",
  );

  const [library, setLibrary] = useState<LibrarySnapshot | null>(null);
  const [rebuilding, setRebuilding] = useState(false);

  useEffect(() => {
    scanLibrary()
      .then(setLibrary)
      .catch(() => setLibrary(null));
  }, []);

  // `index.css` already honours the OS setting; this lets the app opt in
  // independently of it.
  useEffect(() => {
    document.documentElement.dataset.reducedMotion = String(reducedMotion);
  }, [reducedMotion]);

  const rebuild = async () => {
    setRebuilding(true);
    try {
      const fresh = await scanLibrary(true);
      setLibrary(fresh);
      notify("Index rebuilt", {
        description: `${fresh.entries.length} entries in ${fresh.scannedMs}ms.`,
      });
    } catch (e) {
      notify("Rebuild failed", { description: String(e) });
    } finally {
      setRebuilding(false);
    }
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
                  active ? "border-violet" : "border-border hover:border-border-strong",
                )}
              >
                <div
                  className="h-14 w-full rounded-lg ring-1 ring-inset ring-glass-border"
                  style={{ backgroundImage: THEME_SWATCH[t] }}
                />
                <p className="px-0.5 text-[11px] font-medium">{THEMES[t].label}</p>
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
            <Switch checked={reducedMotion} onCheckedChange={setReducedMotion} />
          }
        />
      </Card>

      <Card title="RUNNERS">
        {library === null ? (
          <p className="text-[11px] text-tertiary">Checking…</p>
        ) : (
          library.runners.map((r) => (
            <Row
              key={r.runner}
              label={r.label}
              hint={r.root.replace(/^\/Users\/[^/]+/, "~")}
              control={
                <span
                  className={cn(
                    "rounded-full border px-2.5 py-1 text-[11px]",
                    r.detected
                      ? "border-border bg-elevated text-muted-foreground"
                      : "border-border text-tertiary",
                  )}
                >
                  {r.detected ? "detected" : "not found"}
                </span>
              }
            />
          ))
        )}
      </Card>

      <Card title="FILES & SAFETY">
        <Row
          label="Snapshot before every write"
          hint="Always on. The previous content is copied to ~/.aviary/history before an edit lands, and a write is refused if the file changed underneath you."
          control={
            <span className="rounded-full border border-border bg-elevated px-2.5 py-1 text-[11px] text-muted-foreground">
              always on
            </span>
          }
        />
        <Row
          label="Allow risky permission modes"
          hint="Shows dontAsk and bypassPermissions in the chat composer. Those let a runner execute commands without asking you first."
          control={
            <Switch checked={allowRiskyModes} onCheckedChange={setAllowRiskyModes} />
          }
        />
        <div className="space-y-2">
          <Label htmlFor="data-folder" className="text-[13px]">
            Data folder
          </Label>
          <Input id="data-folder" value="~/.aviary" readOnly className="font-mono text-xs" />
        </div>
      </Card>

      <Card title="PRIVACY">
        <Row
          label="Telemetry"
          hint="There is none. Aviary makes no network requests of its own — only the runners you launch talk to the outside world."
          control={
            <span className="rounded-full border border-border bg-elevated px-2.5 py-1 text-[11px] text-muted-foreground">
              none
            </span>
          }
        />
        <div className="flex gap-2 pt-1">
          <Button variant="outline" size="sm" disabled={rebuilding} onClick={rebuild}>
            {rebuilding ? "Rebuilding…" : "Rebuild index"}
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={async () => {
              try {
                await revealItemInDir(`${await homeDir()}/.aviary/`);
              } catch (e) {
                notify("Could not open the folder", { description: String(e) });
              }
            }}
          >
            Reveal data folder
          </Button>
        </div>
      </Card>
    </div>
  );
}
