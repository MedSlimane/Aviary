import { toast } from "@/components/ui/toast";

/**
 * Thin wrapper over the Base UI toast manager so call sites read as
 * `notify("Saved", { description: "…" })` instead of `toast.add({ … })`.
 */
export function notify(title: string, options?: { description?: string }) {
  toast.add({ title, description: options?.description });
}
