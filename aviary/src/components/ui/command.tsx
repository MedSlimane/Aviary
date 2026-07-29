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
          "top-[16%] w-[640px] max-w-[calc(100vw-3rem)] translate-y-0 overflow-hidden rounded-[18px]! border border-[rgba(255,255,255,0.13)] bg-[rgba(255,255,255,0.07)] p-0 shadow-[0px_24px_60px_-16px_rgba(0,0,0,0.5)] backdrop-blur-[24px]",
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
      className="flex w-full items-center gap-[12px] border-b border-[rgba(255,255,255,0.09)] py-[17px] pl-[18px] pr-[16px]"
    >
      <HugeiconsIcon
        icon={SearchIcon}
        strokeWidth={1.8}
        className="size-[17px] shrink-0 text-[rgba(255,255,255,0.55)]"
      />
      <CommandPrimitive.Input
        data-slot="command-input"
        className={cn(
          "min-w-0 flex-1 bg-transparent text-[16px] text-[rgba(255,255,255,0.95)] outline-hidden placeholder:text-[rgba(255,255,255,0.4)] disabled:cursor-not-allowed disabled:opacity-50",
          className
        )}
        {...props}
      />
      <kbd className="shrink-0 rounded-[5px] bg-[rgba(255,255,255,0.1)] px-[7px] py-[3px] font-mono text-[10px] text-[rgba(255,255,255,0.55)]">
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
        "no-scrollbar flex max-h-[380px] flex-col gap-[2px] overflow-x-hidden overflow-y-auto p-[8px] outline-none",
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
        "flex flex-col gap-[2px] overflow-hidden text-foreground **:[[cmdk-group-heading]]:pt-[10px] **:[[cmdk-group-heading]]:pb-[4px] **:[[cmdk-group-heading]]:pl-[12px] **:[[cmdk-group-heading]]:text-[10px] **:[[cmdk-group-heading]]:font-semibold **:[[cmdk-group-heading]]:tracking-[0.8px] **:[[cmdk-group-heading]]:text-[rgba(255,255,255,0.45)]",
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
        "group/command-item relative flex w-full cursor-default items-center gap-[12px] rounded-[10px] border border-transparent px-[12px] py-[9px] text-[13px] font-medium text-[rgba(255,255,255,0.88)] outline-hidden select-none in-data-[slot=dialog-content]:rounded-[10px]! data-[disabled=true]:pointer-events-none data-[disabled=true]:opacity-50 data-selected:border-[rgba(255,255,255,0.12)] data-selected:bg-[linear-gradient(90deg,rgba(167,139,250,0.34)_0%,rgba(125,212,252,0.15)_50%,rgba(94,235,212,0.04)_100%),linear-gradient(90deg,rgba(255,255,255,0.1)_0%,rgba(255,255,255,0.1)_100%)] data-selected:text-[rgba(255,255,255,0.96)] [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*=size-])]:size-[16px]",
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
        "ml-auto shrink-0 whitespace-nowrap font-mono text-[11px] font-normal text-[rgba(255,255,255,0.42)]",
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
        "flex w-full shrink-0 items-center gap-[18px] whitespace-pre bg-[rgba(255,255,255,0.04)] px-[18px] py-[11px] font-mono text-[11px] text-[rgba(255,255,255,0.45)]",
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
