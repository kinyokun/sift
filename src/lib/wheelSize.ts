import { useEffect, useRef, type RefObject } from "react";

type Options = {
  get: () => number;
  /** Gets the new size and the event that asked for it. */
  set: (size: number, at: { clientX: number; clientY: number }) => void;
  min: number;
  max: number;
  /** Called when the zoom goes on past `max`: the next thing up takes over. */
  past?: (at: { clientX: number; clientY: number }) => void;
  on?: boolean;
};

/** The wheel has to rest this long at the end before turning on counts as going past it. */
export const ZOOM_REST_MS = 180;
/** How far a wheel turns at the end before it goes past it: about two notches. */
export const ZOOM_PAST = 50;

/**
 * Ctrl/⌘ + wheel over `ref` (which is also what a trackpad pinch sends in
 * Chromium; WebKit sends gesture events) resizes what it shows instead of
 * zooming the page: `set` gets the new size, between `min` and `max`.
 */
export function useWheelSize(ref: RefObject<HTMLElement | null>, options: Options) {
  const { min, max, on = true } = options;
  // The callbacks are read when the wheel turns, so they need not be stable.
  const latest = useRef(options);
  latest.current = options;
  useEffect(() => {
    const get = () => latest.current.get();
    const set: Options["set"] = (size, at) => latest.current.set(size, at);
    const hasPast = () => !!latest.current.past;
    const past: NonNullable<Options["past"]> = (at) => latest.current.past?.(at);
    const el = ref.current;
    if (!el || !on) return;
    const clamp = (size: number) => Math.round(Math.min(max, Math.max(min, size)));
    // Small steps (a slow pinch) add up here until they move the size by a pixel.
    let pending = 0;
    // A spin that arrives at the end stops there; turning on after a rest goes past it.
    let last = 0;
    let rested = true;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      if (e.timeStamp - last > ZOOM_REST_MS) rested = true;
      last = e.timeStamp;
      // Lines or pages rather than pixels: a mouse wheel on some systems.
      const delta = e.deltaY * (e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 120 : 1);
      // One notch of a wheel is about an eighth; a pinch arrives as many small steps.
      pending += Math.max(-30, Math.min(30, delta));
      const now = clamp(get());
      const next = clamp(now * Math.exp(-pending * 0.004));
      if (next !== now) {
        pending = 0;
        rested = false;
        set(next, e);
      } else if (next === min && pending > 0) {
        pending = 0;
      } else if (next === max && pending < 0) {
        if (!hasPast() || !rested) pending = 0;
        else if (pending < -ZOOM_PAST) {
          pending = 0;
          rested = false;
          past(e);
        }
      }
    };
    // Safari/WKWebView report trackpad pinches as gesture events.
    type Gesture = Event & { scale: number; clientX: number; clientY: number };
    let base = 0;
    let fromEnd = false;
    let done = false;
    const onGestureStart = (e: Event) => {
      e.preventDefault();
      base = clamp(get());
      fromEnd = base === max;
      done = false;
    };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      if (done) return;
      const g = e as Gesture;
      const next = clamp(base * g.scale);
      if (next !== clamp(get())) set(next, g);
      // Only a pinch that began at the end goes past it.
      if (hasPast() && fromEnd && g.scale > 1.25) {
        done = true;
        past(g);
      }
    };
    const onGestureEnd = (e: Event) => e.preventDefault();
    el.addEventListener("wheel", onWheel, { passive: false });
    el.addEventListener("gesturestart", onGestureStart);
    el.addEventListener("gesturechange", onGestureChange);
    el.addEventListener("gestureend", onGestureEnd);
    return () => {
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("gesturestart", onGestureStart);
      el.removeEventListener("gesturechange", onGestureChange);
      el.removeEventListener("gestureend", onGestureEnd);
    };
  }, [ref, min, max, on]);
}
