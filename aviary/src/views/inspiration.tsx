import { useState } from "react";
import * as motionReact from "motion/react";
import { Button } from "@/components/ui/button";
import { notify } from "@/lib/notify";
import { PageHeader, Segmented } from "@/components/screen-parts";
import { cn } from "@/lib/utils";

const { motion, AnimatePresence } = motionReact;

const COLLECTIONS = ["All", "Gradients", "Type", "Motion", "Dark UI"] as const;
type Collection = (typeof COLLECTIONS)[number];

const ART: Record<string, string> = {
  aurora: "linear-gradient(105deg, #2b2140, #5b4b9e 35%, #7c8fe0 70%, #bfd9f2)",
  tidal: "linear-gradient(95deg, #10312f, #2e6e66 35%, #7fc9c0 70%, #eaf7ef)",
  ember: "linear-gradient(115deg, #3a1d22, #8e4a48 35%, #d98a6b 70%, #fce3c8)",
  dusk: "linear-gradient(120deg, #160b2e, #43156b 35%, #9b2b84 70%, #f0a0b4)",
};

type Tile = {
  id: string;
  art: keyof typeof ART;
  h: number;
  tags: Collection[];
};

const TILES: Tile[] = [
  { id: "a", art: "aurora", h: 196, tags: ["Gradients"] },
  { id: "b", art: "dusk", h: 148, tags: ["Dark UI"] },
  { id: "c", art: "tidal", h: 112, tags: ["Gradients", "Motion"] },
  { id: "d", art: "ember", h: 150, tags: ["Gradients"] },
  { id: "e", art: "aurora", h: 182, tags: ["Type"] },
  { id: "f", art: "tidal", h: 224, tags: ["Motion"] },
  { id: "g", art: "ember", h: 128, tags: ["Type"] },
  { id: "h", art: "dusk", h: 104, tags: ["Dark UI"] },
  { id: "i", art: "dusk", h: 168, tags: ["Dark UI", "Gradients"] },
  { id: "j", art: "tidal", h: 152, tags: ["Motion"] },
  { id: "k", art: "ember", h: 116, tags: ["Type"] },
];

export function InspirationView() {
  const [collection, setCollection] = useState<Collection>("All");

  const visible =
    collection === "All"
      ? TILES
      : TILES.filter((t) => t.tags.includes(collection));

  // Distribute across 4 masonry columns
  const columns: Tile[][] = [[], [], [], []];
  visible.forEach((t, i) => columns[i % 4].push(t));

  return (
    <div className="flex flex-col gap-[18px] p-[26px]">
      <PageHeader
        title="Inspiration"
        subtitle={`${visible.length} items · ${COLLECTIONS.length - 1} collections`}
        action={
          <Button
            size="sm"
            className="rounded-full"
            onClick={() =>
              notify("Import media", {
                description: "Drop images, clips or links to add them.",
              })
            }
          >
            Import
          </Button>
        }
      />

      <Segmented
        options={COLLECTIONS}
        value={collection}
        onChange={setCollection}
        layoutId="inspiration-collection"
      />

      {/* aviary-media callout */}
      <motion.div
        whileHover={{ y: -1 }}
        transition={{ type: "spring", stiffness: 480, damping: 30 }}
        className="flex items-center gap-3.5 overflow-hidden rounded-[14px] border border-border bg-card p-1 pr-4"
      >
        <div
          className="h-12 w-[120px] shrink-0 rounded-[11px]"
          style={{ backgroundImage: ART.ember }}
        />
        <div className="min-w-0 flex-1 space-y-0.5">
          <p className="truncate text-[13px] font-medium">
            Your agents can search this library
          </p>
          <p className="truncate font-mono text-[11px] text-tertiary">
            aviary-media › search_media("grainy teal gradient")
          </p>
        </div>
      </motion.div>

      {/* Masonry */}
      <div className="flex gap-3">
        {columns.map((col, ci) => (
          <div key={ci} className="flex flex-1 flex-col gap-3">
            <AnimatePresence mode="popLayout">
              {col.map((t) => (
                <motion.button
                  key={t.id}
                  layout
                  type="button"
                  initial={{ opacity: 0, scale: 0.94 }}
                  animate={{ opacity: 1, scale: 1 }}
                  exit={{ opacity: 0, scale: 0.94 }}
                  whileHover={{ scale: 1.015 }}
                  whileTap={{ scale: 0.985 }}
                  transition={{ type: "spring", stiffness: 400, damping: 32 }}
                  onClick={() =>
                    notify("Copied reference", {
                      description: "Path copied — paste it into any agent.",
                    })
                  }
                  className={cn(
                    "w-full shrink-0 rounded-xl ring-1 ring-inset ring-white/[0.06]",
                  )}
                  style={{ height: t.h, backgroundImage: ART[t.art] }}
                />
              ))}
            </AnimatePresence>
          </div>
        ))}
      </div>
    </div>
  );
}
