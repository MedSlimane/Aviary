import type { Transition, Variants } from "motion/react";

/**
 * Motion vocabulary for Aviary.
 *
 * The design spec caps interactive-path animation at 300ms and requires
 * everything to respect prefers-reduced-motion. Springs are used for anything
 * the user directly manipulates; short eased tweens for incidental reveals.
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

export const easeOut: Transition = { duration: 0.18, ease: [0.22, 1, 0.36, 1] };

/** Press/hover feedback for any clickable surface. */
export const pressable = {
  whileHover: { y: -1 },
  whileTap: { scale: 0.985, y: 0 },
  transition: springSnap,
} as const;

/** Subtle press for small controls where lift would look wrong. */
export const pressableFlat = {
  whileTap: { scale: 0.96 },
  transition: springSnap,
} as const;

/** Staggered list container. */
export const listContainer: Variants = {
  hidden: {},
  show: {
    transition: { staggerChildren: 0.028, delayChildren: 0.02 },
  },
};

/** Individual row/card entering a staggered list. */
export const listItem: Variants = {
  hidden: { opacity: 0, y: 8 },
  show: { opacity: 1, y: 0, transition: { duration: 0.24, ease: [0.22, 1, 0.36, 1] } },
};

/** Whole-view transition when switching routes. */
export const viewTransition: Variants = {
  hidden: { opacity: 0, y: 6 },
  show: { opacity: 1, y: 0, transition: { duration: 0.2, ease: [0.22, 1, 0.36, 1] } },
  exit: { opacity: 0, y: -4, transition: { duration: 0.12 } },
};
