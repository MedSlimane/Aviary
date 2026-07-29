import * as React from "react"
import { Command as CommandPrimitive } from "cmdk"

import { cn } from "@/lib/utils"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { HugeiconsIcon } from "@hugeicons/react"
import { SearchIcon } from "@hugeicons/core-free-icons"

function Command({
  className,
  ...props
}: React.ComponentProps<typeof CommandPrimitive>) {
  return (
    <CommandPrimitive
      data-slot="command"
      className={cn(
        "flex size-full flex-col overflow-hidden rounded-[18px]! bg-transparent text-foreground",
        className
      )}
      {...props}
    />
  )
}

function CommandDialog({
  title = "Command Palette",
  description = "Search for a command to run...",
  children,
  className,
  showCloseButton = false,
  ...props
}: Omit<React.ComponentProps<typeof Dialog>, "children"> & {
  title?: string
  description?: string
  className?: string
  showCloseButton?: boolean
  children: React.ReactNode
}) {
  return (
    <Dialog {...props}>
      <DialogHeader className="sr-only">
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>
      <DialogContent
        className={cn(
          // DialogContent ships `sm:max-w-sm` (384px). That responsive-prefixed
          // cap outranks an unprefixed width, so it must be cleared at the same
          // breakpoint or the palette silently stays narrow.
          "top-[16%] w-[clamp(520px,44vw,920px)] max-w-[calc(100vw-3rem)] translate-y-0 overflow-hidden rounded-[18px]! border border-glass-border bg-glass p-0 shadow-[0px_24px_60px_-16px_rgba(0,0,0,0.5)] backdrop-blur-[24px] sm:max-w-[calc(100vw-3rem)]",
          className
        )}
        showCloseButton={showCloseButton}
      >
        {/* cmdk's Input/List/Item read context from the Command root — without
            this wrapper they render nothing and the dialog appears empty. */}
        <Command>{children}</Command>
      </DialogContent>
    </Dialog>
  )
}

function CommandInput({
  className,
  ...props
}: React.ComponentProps<typeof CommandPrimitive.Input>) {
  return (
    <div
      data-slot="command-input-wrapper"
      className="flex w-full items-center gap-[12px] border-b border-glass-border py-[17px] pl-[18px] pr-[16px]"
    >
      <HugeiconsIcon
        icon={SearchIcon}
        strokeWidth={1.8}
        className="size-[17px] shrink-0 text-on-glass-3"
      />
      <CommandPrimitive.Input
        data-slot="command-input"
        className={cn(
          "min-w-0 flex-1 bg-transparent text-[16px] text-on-glass outline-hidden placeholder:text-on-glass-3 disabled:cursor-not-allowed disabled:opacity-50",
          className
        )}
        {...props}
      />
      <kbd className="shrink-0 rounded-[5px] bg-glass-hover px-[7px] py-[3px] font-mono text-[10px] text-on-glass-3">
        esc
      </kbd>
    </div>
  )
}

function CommandList({
  className,
  ...props
}: React.ComponentProps<typeof CommandPrimitive.List>) {
  return (
    <CommandPrimitive.List
      data-slot="command-list"
      className={cn(
        "no-scrollbar flex max-h-[clamp(280px,46vh,560px)] flex-col gap-[2px] overflow-x-hidden overflow-y-auto p-[8px] outline-none",
        className
      )}
      {...props}
    />
  )
}

function CommandEmpty({
  className,
  ...props
}: React.ComponentProps<typeof CommandPrimitive.Empty>) {
  return (
    <CommandPrimitive.Empty
      data-slot="command-empty"
      className={cn("py-6 text-center text-sm", className)}
      {...props}
    />
  )
}

function CommandGroup({
  className,
  ...props
}: React.ComponentProps<typeof CommandPrimitive.Group>) {
  return (
    <CommandPrimitive.Group
      data-slot="command-group"
      className={cn(
        "flex flex-col gap-[2px] overflow-hidden text-foreground **:[[cmdk-group-heading]]:pt-[10px] **:[[cmdk-group-heading]]:pb-[4px] **:[[cmdk-group-heading]]:pl-[12px] **:[[cmdk-group-heading]]:text-[10px] **:[[cmdk-group-heading]]:font-semibold **:[[cmdk-group-heading]]:tracking-[0.8px] **:[[cmdk-group-heading]]:text-on-glass-3",
        className
      )}
      {...props}
    />
  )
}

function CommandSeparator({
  className,
  ...props
}: React.ComponentProps<typeof CommandPrimitive.Separator>) {
  return (
    <CommandPrimitive.Separator
      data-slot="command-separator"
      className={cn("hidden", className)}
      {...props}
    />
  )
}

function CommandItem({
  className,
  children,
  ...props
}: React.ComponentProps<typeof CommandPrimitive.Item>) {
  return (
    <CommandPrimitive.Item
      data-slot="command-item"
      className={cn(
        "group/command-item relative flex w-full cursor-default items-center gap-[12px] rounded-[10px] border border-transparent px-[12px] py-[9px] text-[13px] font-medium text-on-glass-2 outline-hidden select-none in-data-[slot=dialog-content]:rounded-[10px]! data-[disabled=true]:pointer-events-none data-[disabled=true]:opacity-50 data-selected:text-on-glass [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*=size-])]:size-[16px]",
        className
      )}
      {...props}
    >
      {children}
    </CommandPrimitive.Item>
  )
}

function CommandShortcut({
  className,
  ...props
}: React.ComponentProps<"span">) {
  return (
    <span
      data-slot="command-shortcut"
      className={cn(
        "ml-auto shrink-0 whitespace-nowrap font-mono text-[11px] font-normal text-on-glass-3",
        className
      )}
      {...props}
    />
  )
}

function CommandFooter({
  className,
  ...props
}: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="command-footer"
      className={cn(
        "flex w-full shrink-0 items-center gap-[18px] whitespace-pre bg-glass px-[18px] py-[11px] font-mono text-[11px] text-on-glass-3",
        className
      )}
      {...props}
    />
  )
}

export {
  Command,
  CommandDialog,
  CommandFooter,
  CommandInput,
  CommandList,
  CommandEmpty,
  CommandGroup,
  CommandItem,
  CommandShortcut,
  CommandSeparator,
}
