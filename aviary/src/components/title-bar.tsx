import { HugeiconsIcon } from "@hugeicons/react";
import { Search01Icon, Settings01Icon } from "@hugeicons/core-free-icons";
import { Kbd } from "@/components/ui/kbd";
import { THEMES, useTheme, type ThemeName } from "@/lib/theme";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

export function TitleBar({ onOpenPalette }: { onOpenPalette: () => void }) {
  const { theme, setTheme } = useTheme();

  return (
    <header
      data-tauri-drag-region
      className="flex h-[46px] shrink-0 items-center gap-4 border-b border-border bg-card px-4"
    >
      {/* Reserved for the native macOS traffic lights (titleBarStyle: Overlay) */}
      <div className="w-[68px] shrink-0" />

      <div className="flex flex-1 justify-center">
        <button
          type="button"
          onClick={onOpenPalette}
          className="av-hover-grad flex h-[34px] w-[360px] items-center gap-2 rounded-[10px] border border-border bg-elevated px-3 text-left text-[13px] text-tertiary transition-colors hover:border-border-strong"
        >
          <HugeiconsIcon icon={Search01Icon} size={15} strokeWidth={1.5} />
          <span className="flex-1 truncate">
            Search prompts, skills, agents…
          </span>
          <Kbd className="text-[10px]">⌘K</Kbd>
        </button>
      </div>

      <div className="flex shrink-0 items-center gap-3">
        <DropdownMenu>
          <DropdownMenuTrigger
            aria-label="Theme"
            className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-hover hover:text-foreground"
          >
            <HugeiconsIcon icon={Settings01Icon} size={16} strokeWidth={1.5} />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-40">
            <DropdownMenuRadioGroup
              value={theme}
              onValueChange={(v) => setTheme(v as ThemeName)}
            >
              <DropdownMenuLabel>Theme</DropdownMenuLabel>
              {Object.entries(THEMES).map(([id, t]) => (
                <DropdownMenuRadioItem key={id} value={id}>
                  {t.label}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>

        <div
          className="size-[22px] rounded-full"
          style={{
            backgroundImage:
              "linear-gradient(135deg, var(--av-accent-violet), var(--av-accent-peach))",
          }}
        />
      </div>
    </header>
  );
}
