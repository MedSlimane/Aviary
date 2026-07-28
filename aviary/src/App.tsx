import { useEffect, useState } from "react";
import { AppRail, NAV_ITEMS, type RouteId } from "@/components/app-rail";
import { TitleBar } from "@/components/title-bar";
import { ThemeProvider } from "@/lib/theme";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Empty, EmptyDescription, EmptyTitle } from "@/components/ui/empty";
import { LibraryView } from "@/views/library";

function PlaceholderView({ route }: { route: RouteId }) {
  const item = NAV_ITEMS.find((n) => n.id === route);
  const label = item?.label ?? "Settings";
  return (
    <Empty className="flex-1">
      <EmptyTitle>{label}</EmptyTitle>
      <EmptyDescription>
        This surface is designed in Figma and not built yet.
      </EmptyDescription>
    </Empty>
  );
}

function Shell() {
  const [route, setRoute] = useState<RouteId>("library");
  const [paletteOpen, setPaletteOpen] = useState(false);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "k" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        setPaletteOpen((open) => !open);
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-background text-foreground">
      <TitleBar onOpenPalette={() => setPaletteOpen(true)} />

      <div className="flex min-h-0 flex-1">
        <AppRail active={route} onNavigate={setRoute} />
        <main className="av-canvas-dots min-w-0 flex-1 overflow-auto">
          {route === "library" ? (
            <LibraryView />
          ) : (
            <PlaceholderView route={route} />
          )}
        </main>
      </div>

      <CommandDialog open={paletteOpen} onOpenChange={setPaletteOpen}>
        <CommandInput placeholder="Search prompts, skills, agents…" />
        <CommandList>
          <CommandEmpty>No results found.</CommandEmpty>
          <CommandGroup heading="Go to">
            {NAV_ITEMS.map((item) => (
              <CommandItem
                key={item.id}
                value={item.label}
                onSelect={() => {
                  setRoute(item.id);
                  setPaletteOpen(false);
                }}
              >
                {item.label}
              </CommandItem>
            ))}
          </CommandGroup>
        </CommandList>
      </CommandDialog>
    </div>
  );
}

export default function App() {
  return (
    <ThemeProvider>
      <Shell />
    </ThemeProvider>
  );
}
