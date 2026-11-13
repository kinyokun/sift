import { useEffect, type RefObject } from "react";

/**
 * Ctrl/⌘ + wheel over `ref` (which is also what a trackpad pinch sends)
 * resizes what it shows instead of zooming the page: `set` gets the new size,
 * between `min` and `max`.
 */
export function useWheelSize(ref: RefObject<HTMLElement | null>, get: () => number, set: (size: number) => void, min: number, max: number, on = true) {
  useEffect(() => {
    const el = ref.current;
    if (!el || !on) return;
    // Small steps (a slow pinch) add up here until they move the size by a pixel.
    let pending = 0;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      // Lines or pages rather than pixels: a mouse wheel on some systems.
      const delta = e.deltaY * (e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 120 : 1);
      // One notch of a wheel is about an eighth; a pinch arrives as many small steps.
      pending += Math.max(-30, Math.min(30, delta));
      const next = Math.round(Math.min(max, Math.max(min, get() * Math.exp(-pending * 0.004))));
      if (next !== get()) {
        pending = 0;
        set(next);
      } else if ((next === min && pending > 0) || (next === max && pending < 0)) {
        pending = 0;
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
    // `get` and `set` are read when the wheel turns, so they need not be stable.
  }, [ref, min, max, on]);
}
