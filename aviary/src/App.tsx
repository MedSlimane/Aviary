import { useEffect, useState } from "react";
import * as motionReact from "motion/react";
import { AppRail, NAV_ITEMS, type RouteId } from "@/components/app-rail";
import { TitleBar } from "@/components/title-bar";
import { ThemeProvider, THEMES, useTheme, type ThemeName } from "@/lib/theme";
import { viewTransition } from "@/lib/motion";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
  CommandShortcut,
} from "@/components/ui/command";
import { Toaster } from "@/components/ui/toast";
import { notify } from "@/lib/notify";
import { HomeView } from "@/views/home";
import { ChatView } from "@/views/chat";
import { LibraryView } from "@/views/library";
import { McpView } from "@/views/mcp";
import { ContextView } from "@/views/context";
import { InspirationView } from "@/views/inspiration";
import { SettingsView } from "@/views/settings";

const { motion, AnimatePresence } = motionReact;

const LIBRARY_ITEMS = [
  { name: "design-taste-frontend", meta: "Skill · Claude Code" },
  { name: "systematic-debugging", meta: "Skill · Claude Code" },
  { name: "Explore", meta: "Agent · Claude Code" },
  { name: "brandkit", meta: "Skill · Codex" },
  { name: "/verify", meta: "Command · Claude Code" },
];

const BUNDLE_ITEMS = [
  "Frontend Review",
  "Deep Research",
  "Design Exploration",
  "Repo Triage",
];

function Shell() {
  const [route, setRoute] = useState<RouteId>("home");
  const [paletteOpen, setPaletteOpen] = useState(false);
  const { setTheme } = useTheme();

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "k" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        setPaletteOpen((o) => !o);
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  const go = (r: RouteId) => {
    setRoute(r);
    setPaletteOpen(false);
  };

  const renderView = () => {
    switch (route) {
      case "home":
        return <HomeView onNavigate={setRoute} />;
      case "chat":
        return <ChatView />;
      case "library":
        return <LibraryView />;
      case "mcp":
        return <McpView />;
      case "context":
        return <ContextView />;
      case "inspiration":
        return <InspirationView />;
      case "settings":
        return <SettingsView />;
    }
  };

  // Chat owns its own full-height layout; other views scroll normally.
  const isChat = route === "chat";

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-background text-foreground">
      <TitleBar onOpenPalette={() => setPaletteOpen(true)} />

      <div className="flex min-h-0 flex-1">
        <AppRail active={route} onNavigate={setRoute} />
        <main
          className={cnMain(isChat)}
          key="main"
        >
          <AnimatePresence mode="wait">
            <motion.div
              key={route}
              variants={viewTransition}
              initial="hidden"
              animate="show"
              exit="exit"
              className={isChat ? "h-full" : undefined}
            >
              {renderView()}
            </motion.div>
          </AnimatePresence>
        </main>
      </div>

      <CommandDialog
        open={paletteOpen}
        onOpenChange={setPaletteOpen}
        title="Command palette"
        description="Search across your library, bundles and actions"
      >
        <CommandInput placeholder="Search prompts, skills, agents…" />
        <CommandList>
          <CommandEmpty>No results found.</CommandEmpty>

          <CommandGroup heading="Go to">
            {NAV_ITEMS.map((item) => (
              <CommandItem
                key={item.id}
                value={`go ${item.label}`}
                onSelect={() => go(item.id)}
              >
                {item.label}
              </CommandItem>
            ))}
            <CommandItem value="go Settings" onSelect={() => go("settings")}>
              Settings
            </CommandItem>
          </CommandGroup>

          <CommandSeparator />

          <CommandGroup heading="Library">
            {LIBRARY_ITEMS.map((l) => (
              <CommandItem
                key={l.name}
                value={l.name}
                onSelect={() => {
                  go("library");
                  notify(`Opened ${l.name}`);
                }}
              >
                <span className="flex-1">{l.name}</span>
                <span className="text-[11px] text-tertiary">{l.meta}</span>
              </CommandItem>
            ))}
          </CommandGroup>

          <CommandSeparator />

          <CommandGroup heading="Bundles">
            {BUNDLE_ITEMS.map((b) => (
              <CommandItem
                key={b}
                value={`bundle ${b}`}
                onSelect={() => {
                  go("chat");
                  notify(`Launched ${b}`, {
                    description: "Attached to a new Claude Code session.",
                  });
                }}
              >
                <span className="flex-1">{b}</span>
                <CommandShortcut>⌘↵</CommandShortcut>
              </CommandItem>
            ))}
          </CommandGroup>

          <CommandSeparator />

          <CommandGroup heading="Theme">
            {(Object.keys(THEMES) as ThemeName[]).map((t) => (
              <CommandItem
                key={t}
                value={`theme ${THEMES[t].label}`}
                onSelect={() => {
                  setTheme(t);
                  setPaletteOpen(false);
                  notify(`Theme: ${THEMES[t].label}`);
                }}
              >
                {THEMES[t].label}
              </CommandItem>
            ))}
          </CommandGroup>
        </CommandList>
      </CommandDialog>
    </div>
  );
}

function cnMain(isChat: boolean) {
  return isChat
    ? "min-w-0 flex-1 overflow-hidden"
    : "av-canvas-dots min-w-0 flex-1 overflow-auto";
}

export default function App() {
  return (
    <ThemeProvider>
      <Toaster>
        <Shell />
      </Toaster>
    </ThemeProvider>
  );
}
