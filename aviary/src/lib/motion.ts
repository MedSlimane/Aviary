import type { Transition } from "motion/react";

/**
 * Motion vocabulary for Aviary.
 *
 * Deliberately narrow: motion is used ONLY for direct interaction feedback —
 * hover, press, drag, toggle. Views and lists render instantly, because
 * entry animation on navigation taxes every single trip through the app.
 */

export const spring: Transition = {
  type: "spring",
  stiffness: 420,
  damping: 32,
  mass: 0.7,
};

/** Snappier spring for press feedback. */
export const springSnap: Transition = {
  type: "spring",
  stiffness: 620,
  damping: 26,
  mass: 0.5,
};

/** Shared-layout spring for selection pills sliding between options. */
export const springLayout: Transition = {
  type: "spring",
  stiffness: 520,
  damping: 38,
};

/** Press/hover feedback for cards and rows. */
export const pressable = {
  whileHover: { y: -1 },
  whileTap: { scale: 0.985, y: 0 },
  transition: springSnap,
} as const;

/** Press feedback for small controls, where a lift would look wrong. */
export const pressableFlat = {
  whileTap: { scale: 0.96 },
  transition: springSnap,
} as const;

/** Press feedback for icon buttons. */
export const pressableIcon = {
  whileHover: { scale: 1.06 },
  whileTap: { scale: 0.92 },
  transition: springSnap,
} as const;
