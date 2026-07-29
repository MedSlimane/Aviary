import { useEffect, useState } from "react";
import { AppRail, NAV_ITEMS, type RouteId } from "@/components/app-rail";
import { TitleBar } from "@/components/title-bar";
import { ThemeProvider, THEMES, useTheme, type ThemeName } from "@/lib/theme";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
  CommandShortcut,
  CommandFooter,
} from "@/components/ui/command";
import { Toaster } from "@/components/ui/toast";
import { ErrorBoundary } from "@/components/error-boundary";
import { notify } from "@/lib/notify";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  SparklesIcon,
  Layers01Icon,
  PaintBoardIcon,
  ArrowRight01Icon,
} from "@hugeicons/core-free-icons";
import { useLibrary } from "@/lib/use-library";
import { HomeView } from "@/views/home";
import { ChatView } from "@/views/chat";
import { LibraryView } from "@/views/library";
import { ProjectsView } from "@/views/projects";
import { McpView } from "@/views/mcp";
import { ContextView } from "@/views/context";
import { InspirationView } from "@/views/inspiration";
import { SettingsView } from "@/views/settings";

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
  const { data } = useLibrary();

  // Palette searches the real library; cmdk filters, so a slice keeps the
  // list responsive without hiding matches the user typed toward.
  const libraryItems = (data?.entries ?? []).slice(0, 200);

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
      case "projects":
        return <ProjectsView />;
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

  // Chat and Library manage their own scrolling internally — Library has two
  // independently scrolling columns, so the shell must not scroll around it.
  const ownsScroll =
    route === "chat" || route === "library" || route === "projects";

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-background text-foreground">
      <TitleBar onOpenPalette={() => setPaletteOpen(true)} />

      <div className="flex min-h-0 flex-1">
        <AppRail active={route} onNavigate={setRoute} />
        <main className={cnMain(route)} key="main">
          {ownsScroll ? (
            <div className="h-full">{renderView()}</div>
          ) : (
            <div className="h-full overflow-y-auto">{renderView()}</div>
          )}
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

          <CommandGroup heading="GO TO">
            {NAV_ITEMS.map((item) => (
              <CommandItem
                key={item.id}
                value={`go ${item.label}`}
                onSelect={() => go(item.id)}
              >
                <HugeiconsIcon icon={item.icon} strokeWidth={1.5} />
                <span className="min-w-0 flex-1 truncate">{item.label}</span>
                <CommandShortcut>↵</CommandShortcut>
              </CommandItem>
            ))}
            <CommandItem value="go Settings" onSelect={() => go("settings")}>
              <HugeiconsIcon icon={ArrowRight01Icon} strokeWidth={1.5} />
              <span className="min-w-0 flex-1 truncate">Settings</span>
              <CommandShortcut>↵</CommandShortcut>
            </CommandItem>
          </CommandGroup>

          <CommandSeparator />

          <CommandGroup heading="LIBRARY">
            {libraryItems.map((l) => (
              <CommandItem
                key={l.id}
                value={`${l.name} ${l.description} ${l.group ?? ""}`}
                onSelect={() => {
                  go("library");
                  notify(l.name, {
                    description: l.path.replace(/^\/Users\/[^/]+/, "~"),
                  });
                }}
              >
                <HugeiconsIcon icon={SparklesIcon} strokeWidth={1.5} />
                <span className="min-w-0 flex-1 truncate">{l.name}</span>
                <CommandShortcut>{l.group ?? l.kind}</CommandShortcut>
              </CommandItem>
            ))}
          </CommandGroup>

          <CommandSeparator />

          <CommandGroup heading="BUNDLES">
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
                <HugeiconsIcon icon={Layers01Icon} strokeWidth={1.5} />
                <span className="min-w-0 flex-1 truncate">{b}</span>
                <CommandShortcut>⌘↵</CommandShortcut>
              </CommandItem>
            ))}
          </CommandGroup>

          <CommandSeparator />

          <CommandGroup heading="THEME">
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
                <HugeiconsIcon icon={PaintBoardIcon} strokeWidth={1.5} />
                <span className="min-w-0 flex-1 truncate">{THEMES[t].label}</span>
              </CommandItem>
            ))}
          </CommandGroup>
        </CommandList>
        <CommandFooter>
          <span>{"↵  open"}</span>
          <span>{"↑↓  navigate"}</span>
          <span>{"⌘↵  launch bundle"}</span>
          <span className="ml-auto">{"esc  close"}</span>
        </CommandFooter>
      </CommandDialog>
    </div>
  );
}

function cnMain(route: RouteId) {
  // The shell never scrolls; each view decides how its own content scrolls.
  return route === "chat"
    ? "min-w-0 flex-1 overflow-hidden"
    : "av-canvas-dots min-w-0 flex-1 overflow-hidden";
}

export default function App() {
  return (
    <ErrorBoundary>
      <ThemeProvider>
        <Toaster>
          <Shell />
        </Toaster>
      </ThemeProvider>
    </ErrorBoundary>
  );
}
