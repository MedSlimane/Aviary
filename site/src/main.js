/**
 * Scroll reveal.
 *
 * IntersectionObserver rather than a scroll listener: a listener fires on every
 * frame and janks on mobile for no benefit. Elements reveal once and are then
 * unobserved.
 */
const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const targets = document.querySelectorAll(".reveal");

if (reduced) {
  targets.forEach((el) => el.classList.add("in"));
} else {
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        entry.target.classList.add("in");
        observer.unobserve(entry.target);
      }
    },
    { rootMargin: "0px 0px -12% 0px", threshold: 0.15 },
  );
  targets.forEach((el) => observer.observe(el));
}
