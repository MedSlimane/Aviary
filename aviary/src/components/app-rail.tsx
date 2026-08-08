import { HugeiconsIcon } from "@hugeicons/react";
import {
  BubbleChatIcon,
  Home01Icon,
  Layers01Icon,
  ServerStack01Icon,
  Folder01Icon,
  Brain02Icon,
  Album02Icon,
  PackageIcon,
  Settings01Icon,
} from "@hugeicons/core-free-icons";
import * as motionReact from "motion/react";
import { cn } from "@/lib/utils";
import { pressableFlat } from "@/lib/motion";

const { motion } = motionReact;

export const NAV_ITEMS = [
  { id: "home", label: "Home", icon: Home01Icon },
  { id: "chat", label: "Chat", icon: BubbleChatIcon },
  { id: "library", label: "Library", icon: Layers01Icon },
  { id: "projects", label: "Projects", icon: Folder01Icon },
  { id: "bundles", label: "Bundles", icon: PackageIcon },
  { id: "mcp", label: "MCP Servers", icon: ServerStack01Icon },
  { id: "context", label: "Context", icon: Brain02Icon },
  { id: "inspiration", label: "Inspiration", icon: Album02Icon },
] as const;

export type RouteId = (typeof NAV_ITEMS)[number]["id"] | "settings";

function AviaryMark({ className }: { className?: string }) {
  // Six-point spark — the placeholder Aviary mark
  const petal =
    "M64 59C58.9 54.3 57.4 46.5 60.2 34.3L64 14C67.8 29.2 70.2 43.2 68.7 49.5C67.8 53.7 66.1 57 64 59Z";
  return (
    <svg viewBox="0 0 128 128" className={className} aria-hidden="true">
      {[0, 60, 120, 180, 240, 300].map((angle) => (
        <path
          key={angle}
          d={petal}
          fill="currentColor"
          transform={`rotate(${angle} 64 64)`}
        />
      ))}
      <circle cx="64" cy="64" r="7.2" fill="currentColor" />
    </svg>
  );
}

type AppRailProps = {
  active: RouteId;
  onNavigate: (id: RouteId) => void;
};

export function AppRail({ active, onNavigate }: AppRailProps) {
  return (
    <nav className="flex w-[228px] shrink-0 flex-col gap-[3px] border-r border-border bg-card px-3 py-3.5">
      <div className="flex items-center gap-2.5 px-2 pb-4 pt-1">
        <AviaryMark className="size-5 text-violet" />
        <span className="text-sm font-semibold tracking-tight">Aviary</span>
      </div>

      {NAV_ITEMS.map((item) => {
        const isActive = item.id === active;
        return (
          <motion.button
            key={item.id}
            type="button"
            onClick={() => onNavigate(item.id)}
            aria-current={isActive ? "page" : undefined}
            {...pressableFlat}
            className={cn(
              "group relative flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-[13px] font-medium transition-colors",
              isActive
                ? "av-selected-wash bg-selected text-foreground ring-1 ring-inset ring-glass-border"
                : "av-hover-grad text-muted-foreground hover:text-foreground",
            )}
          >
            <HugeiconsIcon
              icon={item.icon}
              size={16}
              strokeWidth={1.5}
              className="shrink-0"
            />
            <span className="truncate">{item.label}</span>
          </motion.button>
        );
      })}

      <div className="flex-1" />

      <motion.button
        type="button"
        onClick={() => onNavigate("settings")}
        {...pressableFlat}
        className={cn(
          "flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-[13px] font-medium transition-colors",
          active === "settings"
            ? "av-selected-wash bg-selected text-foreground ring-1 ring-inset ring-glass-border"
            : "av-hover-grad text-muted-foreground hover:text-foreground",
        )}
      >
        <HugeiconsIcon
          icon={Settings01Icon}
          size={16}
          strokeWidth={1.5}
          className="shrink-0"
        />
        <span className="truncate">Settings</span>
      </motion.button>
    </nav>
  );
}
