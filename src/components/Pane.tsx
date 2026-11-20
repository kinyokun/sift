import { forwardRef, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from "react";
import { NO_LOOK, imageUrl, isNefPath, isRawPath, type CropBox, type Item, type Meta } from "../lib/api";
import { tiltOf } from "../lib/level";
import type { WheelMode } from "../lib/store";
import { ZOOM_PAST, ZOOM_REST_MS } from "../lib/wheelSize";
import { spin, turnPoint, turnRect, turnStyle, turnedSize } from "../lib/turn";
import { Overlay, type Crop } from "./Overlay";

export type View = { scale: number; tx: number; ty: number };
/** A user gesture expressed relative to the image, for mirroring to a linked pane. */
export type Nudge = { factor: number; dx: number; dy: number };

export type PaneHandle = {
  fit: () => void;
  /** Toggles between fit and one image pixel per device pixel. */
  toggleActual: () => void;
  /** Centres on a normalised image point at one image pixel per CSS pixel. */
  zoomTo: (nx: number, ny: number) => void;
  nudge: (n: Nudge) => void;
  isZoomed: () => boolean;
};

type Props = {
  item: Item;
  meta: Meta | null;
  active: boolean;
  dimmed: boolean;
  insets: { top: number; bottom: number };
  mobile: boolean;
  wheel: WheelMode;
  /** How far one notch of the wheel scrolls (`wheelNotch`). */
  notch: number;
  /** A double click leaves the photo instead of zooming into it. */
  onBack?: () => void;
  autoDevelop: boolean;
  /** Nikon's SDK is installed: NEF looks come from it. */
  nikon: boolean;
  /** Look chosen for this photo: a library look, `NO_LOOK`, or null for as shot. */
  look: string | null;
  /** Confirmed crop: only this part of the photo is shown. */
  frame: CropBox | null;
  /** Quarter turns clockwise the photo was given. */
  turn: number;
  onSwipeV?: (direction: 1 | -1) => void;
  showAf: boolean;
  /** Draw the camera's level over the photo. */
  showLevel?: boolean;
  /** Mark blown highlights and crushed shadows on the photo. */
  clip?: boolean;
  guides: number;
  flip: number;
  crop: Crop | null;
  cropRatio: number | null;
  onCrop: (crop: Crop) => void;
  onActivate: () => void;
  onNudge?: (n: Nudge) => void;
  onSwipe?: (direction: 1 | -1) => void;
  onZoom?: (percent: number, fit: boolean) => void;
  /** Ctrl + wheel or a pinch went on zooming out of a photo that already fits. */
  onZoomOut?: () => void;
  onStatus?: (status: PaneStatus) => void;
  /** The photo's picture has been on screen for a frame, or will never be. */
  onShown?: (id: string) => void;
};

export type PaneStatus = "developing" | "original" | "look" | "failed" | null;

/** The file and size a look is rendered from. Looks start from the RAW when
 * there is one, so they are not stacked on the in-camera look. */
export function lookSource(item: Item, opts: { mobile: boolean; nikon: boolean }): { path: string; edge: number } {
  const raw = item.paths.find(isRawPath);
  // Nikon's engine takes about a second at this size and far longer above it; exports are full size.
  if (opts.nikon && raw && isNefPath(raw)) return { path: raw, edge: NIKON_SCREEN_EDGE };
  return { path: raw ?? item.id, edge: opts.mobile ? MOBILE_FULL_EDGE : 0 };
}

/** Full-resolution URL for a photo, shared with the viewer's read-ahead. */
export function fullUrlOf(item: Item, meta: Meta | null, opts: { mobile: boolean; autoDevelop: boolean; nikon: boolean; look: string | null }): string {
  const raw = item.paths.find(isRawPath);
  const source = lookSource(item, opts);
  if (opts.look && opts.nikon && raw && isNefPath(raw)) return imageUrl("look", source.path, item.mtime, source.edge, opts.look);
  if (opts.look === NO_LOOK && raw) return imageUrl("orig", raw, item.mtime);
  if (opts.look && opts.look !== NO_LOOK) return imageUrl("look", source.path, item.mtime, source.edge, opts.look);
  if (meta?.raw && meta.low_res && opts.autoDevelop) return imageUrl("dev", item.id, item.mtime);
  return opts.mobile ? imageUrl("screen", item.id, item.mtime, MOBILE_FULL_EDGE) : imageUrl("full", item.id, item.mtime);
}

const SCREEN_EDGE = 2048;

/** The picture a photo is first shown with: quick to get, and what the viewer fetches ahead of a page turn. */
export const screenUrlOf = (item: Item) => imageUrl("screen", item.id, item.mtime, SCREEN_EDGE);
const MOBILE_FULL_EDGE = 4096;
const NIKON_SCREEN_EDGE = 2560;
const MAX_SCALE = 8;
const SWIPE_DISTANCE = 56;
const DOUBLE_TAP_MS = 300;
/** A second tap this far from the first is a new tap, not a double tap. */
const DOUBLE_TAP_SLOP = 36;
/** Two-finger trackpad travel that turns the page while the photo fits the pane. */
const WHEEL_SWIPE_DISTANCE = 70;
/** A trackpad scroll (and its momentum tail) ends after this much silence. */
const WHEEL_QUIET_MS = 160;
/** Wheel events further apart than this are separate notches; a trackpad sends one every frame. */
const WHEEL_NOTCH_MS = 40;

/** How far one notch of a mouse wheel scrolls, in the page's pixels. Windows does not speed
 * the wheel up and reports a notch as 100 pixels of the screen, so fewer on a scaled display;
 * macOS reports points, and a hundred of them is a fair page for a trackpad too. */
export function wheelNotch(platform: string | undefined): number {
  return platform === "macos" ? 100 : 100 / devicePixelRatio;
}
/** Glide after a flick: speed halves roughly every 225 ms and stops below this (px/ms). */
const GLIDE_DECAY_MS = 325;
const GLIDE_MIN_SPEED = 0.03;
const GLIDE_START_SPEED = 0.15;
/** Wait this long on a photo before fetching full resolution (and the clipping mask), so fast flipping stays light. */
const FULL_RES_DELAY_MS = 140;

function orientedSize(meta: Meta): { w: number; h: number } | null {
  let [w, h] = [meta.preview_width, meta.preview_height];
  if (meta.low_res && meta.exif.width && meta.exif.height) [w, h] = [meta.exif.width, meta.exif.height];
  if (!w || !h) return null;
  return meta.orientation >= 5 ? { w: h, h: w } : { w, h };
}

export const Pane = forwardRef<PaneHandle, Props>(function Pane(props, ref) {
  const { item, meta, insets } = props;
  const box = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  // The size of the pictures as they arrive (upright), and of the photo once turned.
  const [upright, setDims] = useState<{ w: number; h: number } | null>(null);
  const turn = props.turn;
  const fullDims = useMemo(() => upright && turnedSize(upright, turn), [upright, turn]);
  const frame = props.frame;
  // A straightened photo turns about its centre, confirmed or while its crop is being drawn.
  const angle = frame?.[4] ?? props.crop?.angle ?? 0;
  // Everything below works on the visible part of the photo: the whole frame, or its confirmed crop.
  const dims = useMemo(
    () => (fullDims && frame ? { w: fullDims.w * frame[2], h: fullDims.h * frame[3] } : fullDims),
    [fullDims, frame?.[0], frame?.[1], frame?.[2], frame?.[3]],
  );
  const [view, setViewState] = useState<View>({ scale: 1, tx: 0, ty: 0 });
  const [fitMode, setFitMode] = useState(true);
  // The photo whose full-size picture is wanted. Kept by photo, so the one after it never starts
  // out wanted for a render and fetches a picture nobody waited for.
  const [fullOf, setFullOf] = useState<string | null>(null);
  const wantFull = fullOf === item.id;
  // Which full-size image has arrived (or failed). Keyed by URL, so switching
  // photo or look can never leave a stale "ready" or "loading" behind.
  const [readyUrl, setReadyUrl] = useState<string | null>(null);
  const [failedUrl, setFailedUrl] = useState<string | null>(null);
  // `view` and `fitMode` in here run ahead of the painted state during a
  // gesture, so a render must not write the older state values back.
  const live = useRef({ view, dims, upright, fullDims, frame, turn, angle, size, fitMode, insets, id: item.id });
  Object.assign(live.current, { dims, upright, fullDims, frame, turn, angle, size, insets, id: item.id });

  const original = props.look === NO_LOOK && item.paths.some(isRawPath);
  const styled = !!props.look && props.look !== NO_LOOK;
  const develop = original || styled || (!!meta?.raw && meta.low_res && props.autoDevelop);
  const screenUrl = screenUrlOf(item);
  const fullUrl = fullUrlOf(item, meta, props);
  const fullReady = readyUrl === fullUrl;
  // The clipping warning is taken from what is on screen; a look still
  // rendering is not asked for a second time.
  const clipFrom = original || styled ? { ...lookSource(item, props), look: props.look! } : { path: item.id, edge: 0, look: undefined };
  const clipUrl = props.clip && wantFull && (fullReady || !(original || styled)) ? imageUrl("clip", clipFrom.path, item.mtime, clipFrom.edge, clipFrom.look) : null;

  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const measure = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const fitView = useCallback((d: { w: number; h: number }, s: { w: number; h: number }, ins: { top: number; bottom: number }): View => {
    const availH = Math.max(1, s.h - ins.top - ins.bottom);
    const scale = Math.min(s.w / d.w, availH / d.h);
    return { scale, tx: (s.w - d.w * scale) / 2, ty: ins.top + (availH - d.h * scale) / 2 };
  }, []);

  const clamp = useCallback(
    (v: View): View => {
      const { dims: d, size: s, insets: ins } = live.current;
      if (!d) return v;
      const fit = fitView(d, s, ins);
      const scale = Math.min(MAX_SCALE, Math.max(fit.scale, v.scale));
      const [iw, ih] = [d.w * scale, d.h * scale];
      const tx = iw <= s.w ? (s.w - iw) / 2 : Math.min(0, Math.max(s.w - iw, v.tx));
      const availH = s.h - ins.top - ins.bottom;
      const ty = ih <= availH ? ins.top + (availH - ih) / 2 : Math.min(ins.top, Math.max(s.h - ins.bottom - ih, v.ty));
      return { scale, tx, ty };
    },
    [fitView],
  );

  const report = (v: View, fit: boolean) => props.onZoom?.(Math.round(v.scale * devicePixelRatio * 100), fit);

  // Gestures fire several times per frame; the view is tracked in `live`
  // straight away and painted once per frame.
  const paint = useRef(0);
  const flush = useRef(() => {});
  flush.current = () => {
    paint.current = 0;
    const { view: v, fitMode: fit } = live.current;
    setViewState(v);
    setFitMode(fit);
    report(v, fit);
  };
  useEffect(() => () => cancelAnimationFrame(paint.current), []);

  /** Applies a view; user-driven changes are mirrored to a linked pane. */
  const apply = useCallback(
    (next: View, fromUser: boolean) => {
      const { view: old, dims: d, size: s, insets: ins } = live.current;
      if (!d) return;
      const v = clamp(next);
      const isFit = v.scale <= fitView(d, s, ins).scale * 1.001;
      if (!isFit) setFullOf(live.current.id);
      live.current.view = v;
      live.current.fitMode = isFit;
      if (!paint.current) paint.current = requestAnimationFrame(() => flush.current());
      if (fromUser && props.onNudge) {
        const centre = (q: View) => [(s.w / 2 - q.tx) / q.scale / d.w, (s.h / 2 - q.ty) / q.scale / d.h];
        const [a, b] = [centre(old), centre(v)];
        props.onNudge({ factor: v.scale / old.scale, dx: b[0] - a[0], dy: b[1] - a[1] });
      }
    },
    [clamp, fitView, props.onNudge, props.onZoom],
  );

  const zoomAt = useCallback(
    (cx: number, cy: number, scale: number, fromUser = true) => {
      const { view: v } = live.current;
      const k = Math.min(MAX_SCALE, scale) / v.scale;
      apply({ scale: v.scale * k, tx: cx - (cx - v.tx) * k, ty: cy - (cy - v.ty) * k }, fromUser);
    },
    [apply],
  );

  const centreOn = useCallback(
    (nx: number, ny: number, scale: number, fromUser: boolean) => {
      const { dims: d, size: s } = live.current;
      if (!d) return;
      apply({ scale, tx: s.w / 2 - nx * d.w * scale, ty: s.h / 2 - ny * d.h * scale }, fromUser);
    },
    [apply],
  );

  // New photo: reset to fit and take the size from metadata, before it is painted in the
  // shape and place of the photo before.
  useLayoutEffect(() => {
    setFitMode(true);
    live.current.fitMode = true;
    setDims(meta ? orientedSize(meta) : null);
    const timer = setTimeout(() => setFullOf(item.id), FULL_RES_DELAY_MS);
    return () => clearTimeout(timer);
  }, [item.id, meta]);

  // Keep a fitted image fitted when the pane or the chrome changes size.
  useLayoutEffect(() => {
    if (!dims || !size.w) return;
    if (live.current.fitMode) {
      const v = fitView(dims, size, insets);
      setViewState(v);
      live.current.view = v;
      report(v, true);
    } else {
      const v = clamp(live.current.view);
      setViewState(v);
      live.current.view = v;
    }
  }, [dims, size.w, size.h, insets.top, insets.bottom]);

  // A look is shown as soon as it is chosen, without waiting for the usual delay; one that
  // comes with the photo waits like any full-size picture.
  const chosen = useRef({ id: item.id, look: props.look });
  useEffect(() => {
    if (props.look && chosen.current.id === item.id && chosen.current.look !== props.look) setFullOf(item.id);
    chosen.current = { id: item.id, look: props.look };
  }, [item.id, props.look]);

  // The viewer turns the page only after this photo was shown: tell it once a frame with the picture is out.
  const shown = useRef(0);
  useEffect(() => () => cancelAnimationFrame(shown.current), []);
  const onPicture = () => {
    const id = item.id;
    cancelAnimationFrame(shown.current);
    shown.current = requestAnimationFrame(() => setTimeout(() => props.onShown?.(id)));
  };

  // What the pane is busy with, for the viewer's status pill.
  const busy: PaneStatus = !develop ? null : failedUrl === fullUrl ? "failed" : wantFull && !fullReady ? (styled ? "look" : original ? "original" : "developing") : null;
  useEffect(() => {
    props.onStatus?.(busy);
  }, [busy, item.id]);

  useImperativeHandle(
    ref,
    () => ({
      fit: () => {
        const { dims: d, size: s, insets: ins } = live.current;
        if (d) apply(fitView(d, s, ins), true);
      },
      toggleActual: () => {
        const { dims: d, size: s, insets: ins, fitMode: fit } = live.current;
        if (!d) return;
        if (fit) zoomAt(s.w / 2, s.h / 2, closeScale());
        else apply(fitView(d, s, ins), true);
      },
      zoomTo: (ux, uy) => {
        const { frame: f, turn: q, angle: a, fullDims: full } = live.current;
        const [tx, ty] = turnPoint(ux, uy, q);
        const [nx, ny] = full ? spin(tx, ty, a, full) : [tx, ty];
        const inside = (v: number, start: number, len: number) => Math.min(1, Math.max(0, (v - start) / len));
        centreOn(f ? inside(nx, f[0], f[2]) : nx, f ? inside(ny, f[1], f[3]) : ny, 1, false);
      },
      nudge: (n) => {
        const { view: v, dims: d, size: s } = live.current;
        if (!d) return;
        const scale = v.scale * n.factor;
        const cx = (s.w / 2 - v.tx) / v.scale / d.w + n.dx;
        const cy = (s.h / 2 - v.ty) / v.scale / d.h + n.dy;
        centreOn(cx, cy, scale, false);
      },
      isZoomed: () => !live.current.fitMode,
    }),
    [apply, centreOn, fitView, zoomAt],
  );

  const onZoomOut = useRef(props.onZoomOut);
  onZoomOut.current = props.onZoomOut;

  /** The wheel turning pages: when it last moved, which way, and how far since the last page. */
  const turning = useRef({ last: -Infinity, direction: 0, travel: 0 });

  // Wheel and trackpad gestures need non-passive listeners.
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const local = (e: { clientX: number; clientY: number }) => {
      const r = el.getBoundingClientRect();
      return [e.clientX - r.left, e.clientY - r.top];
    };
    // One two-finger scroll, from the first event to the end of its momentum.
    const scroll = { dx: 0, turned: false, startedFit: true, quiet: 0 };
    let pinching = false;
    // Zooming out of a photo that fits, after a rest, leaves it: see `onZoomOut`.
    const out = { turned: 0, last: 0, rested: true };
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      stopGlide();
      const { view: v, fitMode: fit } = live.current;
      const [x, y] = local(e);
      if (e.ctrlKey || e.metaKey) {
        if (e.timeStamp - out.last > ZOOM_REST_MS) out.rested = true;
        out.last = e.timeStamp;
        if (!pinching && fit && e.deltaY > 0 && onZoomOut.current) {
          if (!out.rested) return;
          out.turned += Math.min(30, e.deltaY * (e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 120 : 1));
          if (out.turned > ZOOM_PAST) {
            out.turned = 0;
            out.rested = false;
            onZoomOut.current();
          }
          return;
        }
        out.turned = 0;
        out.rested = false;
        // A trackpad pinch arrives here in Chromium, and as gesture events in WebKit; never as both.
        if (!pinching) zoomAt(x, y, v.scale * Math.exp(-e.deltaY * 0.012));
        return;
      }
      if (props.wheel === "zoom") return zoomAt(x, y, v.scale * Math.exp(-e.deltaY * 0.0022));
      if (props.wheel === "turn") {
        const delta = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
        if (!delta) return;
        const turn = turning.current;
        const direction = delta > 0 ? 1 : -1;
        // A notch of a wheel arrives on its own and turns a page, however little it scrolls
        // (macOS makes slow notches small). Otherwise a page is a notch of travel: in events
        // that follow each other closely (a trackpad, a wheel spinning freely), and in one
        // that stands for several notches (the browser adds up what it could not deliver).
        const alone = e.timeStamp - turn.last > WHEEL_NOTCH_MS || direction !== turn.direction;
        const travel = (alone ? 0 : turn.travel) + Math.abs(delta) * (e.deltaMode === 1 ? props.notch / 3 : e.deltaMode === 2 ? props.notch : 1);
        const pages = Math.max(alone ? 1 : 0, Math.floor(travel / props.notch));
        Object.assign(turn, { last: e.timeStamp, direction, travel: Math.max(0, travel - pages * props.notch) });
        for (let i = 0; i < pages; i++) props.onSwipe?.(direction);
        return;
      }
      if (!scroll.quiet) Object.assign(scroll, { dx: 0, turned: false, startedFit: fit });
      clearTimeout(scroll.quiet);
      scroll.quiet = window.setTimeout(() => (scroll.quiet = 0), WHEEL_QUIET_MS);
      if (!fit) {
        // A scroll that began as a page turn must not start panning the next photo.
        if (!scroll.turned) apply({ ...v, tx: v.tx - e.deltaX, ty: v.ty - e.deltaY }, true);
        return;
      }
      // While the photo fits, a sideways two-finger swipe turns the page, once per swipe.
      if (!scroll.startedFit || scroll.turned || Math.abs(e.deltaX) < Math.abs(e.deltaY)) return;
      scroll.dx += e.deltaX;
      if (Math.abs(scroll.dx) > WHEEL_SWIPE_DISTANCE) {
        scroll.turned = true;
        props.onSwipe?.(scroll.dx > 0 ? 1 : -1);
      }
    };
    // Safari/WKWebView report trackpad pinches as gesture events.
    let base = 1;
    let fromFit = false;
    const onGestureStart = (e: Event) => {
      e.preventDefault();
      stopGlide();
      pinching = true;
      base = live.current.view.scale;
      fromFit = live.current.fitMode;
    };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      const g = e as unknown as { scale: number; clientX: number; clientY: number };
      if (fromFit && g.scale < 0.75 && onZoomOut.current) {
        fromFit = false;
        return onZoomOut.current();
      }
      const [x, y] = local(g);
      zoomAt(x, y, base * g.scale);
    };
    const onGestureEnd = (e: Event) => {
      e.preventDefault();
      pinching = false;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    el.addEventListener("gesturestart", onGestureStart);
    el.addEventListener("gesturechange", onGestureChange);
    el.addEventListener("gestureend", onGestureEnd);
    return () => {
      clearTimeout(scroll.quiet);
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("gesturestart", onGestureStart);
      el.removeEventListener("gesturechange", onGestureChange);
      el.removeEventListener("gestureend", onGestureEnd);
    };
  }, [apply, zoomAt, props.wheel, props.notch, props.onSwipe]);

  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const gesture = useRef({
    startX: 0,
    startY: 0,
    moved: false,
    /** A second finger took part: the gesture can no longer end as a swipe or a tap. */
    multi: false,
    loupe: null as View | null,
    /** Finger distance and midpoint of the previous pinch frame. */
    pinch: 0,
    mid: [0, 0],
    lastTap: 0,
    tapX: 0,
    tapY: 0,
    /** Speed of the current drag in px/ms, smoothed, for the glide after a flick. */
    vx: 0,
    vy: 0,
    movedAt: 0,
    glide: 0,
    /** Touch double taps are recognised here; the browser's `dblclick` is for mice only. */
    touching: false,
  });

  const stopGlide = () => {
    cancelAnimationFrame(gesture.current.glide);
    gesture.current.glide = 0;
  };
  useEffect(() => stopGlide, []);

  /** Lets a flicked photo coast to a stop. */
  const glide = () => {
    const g = gesture.current;
    let last = performance.now();
    const tick = (now: number) => {
      const dt = Math.min(48, now - last);
      last = now;
      const before = live.current.view;
      apply({ ...before, tx: before.tx + g.vx * dt, ty: before.ty + g.vy * dt }, true);
      const after = live.current.view;
      // An edge stops the motion along its axis.
      if (after.tx === before.tx) g.vx = 0;
      if (after.ty === before.ty) g.vy = 0;
      const decay = Math.exp(-dt / GLIDE_DECAY_MS);
      g.vx *= decay;
      g.vy *= decay;
      g.glide = Math.hypot(g.vx, g.vy) > GLIDE_MIN_SPEED ? requestAnimationFrame(tick) : 0;
    };
    g.glide = requestAnimationFrame(tick);
  };

  /** The zoom level for checking focus: one image pixel per screen pixel,
   * or at least a clear step in when the photo is small. */
  const closeScale = () => {
    const { dims: d, size: s, insets: ins } = live.current;
    const fit = d ? fitView(d, s, ins).scale : 1;
    return Math.max(1 / devicePixelRatio, fit * 2);
  };

  const localPoint = (e: React.PointerEvent | React.MouseEvent) => {
    const r = box.current!.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  };

  const onPointerDown = (e: React.PointerEvent) => {
    props.onActivate();
    const g = gesture.current;
    stopGlide();
    // The first finger down starts from a clean slate, even if an earlier
    // finger's release was never reported.
    if (e.isPrimary) pointers.current.clear();
    g.touching = e.pointerType !== "mouse";
    box.current?.setPointerCapture(e.pointerId);
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pointers.current.size === 1) {
      g.startX = e.clientX;
      g.startY = e.clientY;
      g.moved = false;
      g.multi = false;
      g.vx = g.vy = 0;
      g.movedAt = performance.now();
    }
    if (e.button === 2 && e.pointerType === "mouse") {
      // Hold the right button for a temporary close look.
      const [x, y] = localPoint(e);
      g.loupe = live.current.view;
      zoomAt(x, y, Math.max(1, live.current.view.scale * 1.500), false);
    }
    if (pointers.current.size === 2) {
      const [a, b] = [...pointers.current.values()];
      const r = box.current!.getBoundingClientRect();
      g.multi = true;
      g.pinch = Math.hypot(a.x - b.x, a.y - b.y);
      g.mid = [(a.x + b.x) / 2 - r.left, (a.y + b.y) / 2 - r.top];
    }
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const prev = pointers.current.get(e.pointerId);
    if (!prev) return;
    const g = gesture.current;
    const { view: v, fitMode: fit } = live.current;
    pointers.current.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (Math.hypot(e.clientX - g.startX, e.clientY - g.startY) > 4) g.moved = true;
    if (pointers.current.size === 2) {
      const [a, b] = [...pointers.current.values()];
      const dist = Math.hypot(a.x - b.x, a.y - b.y);
      const r = box.current!.getBoundingClientRect();
      const mid = [(a.x + b.x) / 2 - r.left, (a.y + b.y) / 2 - r.top];
      const { dims: d, size: s, insets: ins } = live.current;
      if (g.pinch > 0 && d) {
        // The point that was between the fingers stays between them: zoom and drag in one step.
        const scale = Math.min(MAX_SCALE, Math.max(fitView(d, s, ins).scale, v.scale * (dist / g.pinch)));
        const k = scale / v.scale;
        apply({ scale, tx: mid[0] - (g.mid[0] - v.tx) * k, ty: mid[1] - (g.mid[1] - v.ty) * k }, true);
      }
      g.pinch = dist;
      g.mid = mid;
      return;
    }
    if (pointers.current.size > 2) return;
    const [dx, dy] = [e.clientX - prev.x, e.clientY - prev.y];
    const now = performance.now();
    const dt = Math.max(1, now - g.movedAt);
    g.movedAt = now;
    // Smooth the speed over the last few moves so one odd sample does not decide the glide.
    g.vx = g.vx * 0.5 + (dx / dt) * 0.5;
    g.vy = g.vy * 0.5 + (dy / dt) * 0.5;
    if (!fit) apply({ ...v, tx: v.tx + dx, ty: v.ty + dy }, !g.loupe);
  };

  const onPointerUp = (e: React.PointerEvent) => {
    const g = gesture.current;
    const wasSingle = pointers.current.size === 1;
    pointers.current.delete(e.pointerId);
    g.pinch = 0;
    if (g.loupe) {
      apply(g.loupe, false);
      g.loupe = null;
      return;
    }
    if (!wasSingle) return;
    if (g.multi) return;
    // A flick on a zoomed photo keeps moving for a moment (touch and pen; a mouse drag just stops).
    const flicked = performance.now() - g.movedAt < 60 && Math.hypot(g.vx, g.vy) > GLIDE_START_SPEED;
    if (!live.current.fitMode && e.pointerType !== "mouse" && g.moved && flicked) return glide();
    const [dx, dy] = [e.clientX - g.startX, e.clientY - g.startY];
    if (live.current.fitMode && e.pointerType !== "mouse" && Math.abs(dx) > SWIPE_DISTANCE && Math.abs(dx) > Math.abs(dy) * 1.500) {
      props.onSwipe?.(dx < 0 ? 1 : -1);
      return;
    }
    if (live.current.fitMode && e.pointerType !== "mouse" && Math.abs(dy) > SWIPE_DISTANCE && Math.abs(dy) > Math.abs(dx) * 1.500) {
      props.onSwipeV?.(dy < 0 ? 1 : -1);
      return;
    }
    if (!g.moved && e.pointerType !== "mouse") {
      const now = performance.now();
      const near = Math.hypot(e.clientX - g.tapX, e.clientY - g.tapY) < DOUBLE_TAP_SLOP;
      if (now - g.lastTap < DOUBLE_TAP_MS && near) {
        toggleAt(e);
        g.lastTap = 0;
      } else {
        g.lastTap = now;
        g.tapX = e.clientX;
        g.tapY = e.clientY;
      }
    }
  };

  const toggleAt = (e: React.PointerEvent | React.MouseEvent) => {
    const { dims: d, size: s, insets: ins, fitMode: fit } = live.current;
    if (!d) return;
    const [x, y] = localPoint(e);
    if (fit) zoomAt(x, y, closeScale());
    else apply(fitView(d, s, ins), true);
  };

  const onFullLoad = (e: React.SyntheticEvent<HTMLImageElement>, url: string) => {
    const img = e.currentTarget;
    setReadyUrl(url);
    const d = live.current.upright;
    // A developed RAW can differ slightly from the size the metadata promised.
    if (d && img.naturalWidth && (Math.abs(img.naturalWidth - d.w) > 2 || Math.abs(img.naturalHeight - d.h) > 2)) {
      const k = d.w / img.naturalWidth;
      const v = live.current.view;
      live.current.view = { ...v, scale: v.scale * k };
      setViewState(live.current.view);
      setDims({ w: img.naturalWidth, h: img.naturalHeight });
    }
  };

  // Focus areas are recorded for the whole upright frame; turn them with the photo and move them into the crop.
  const afBoxes = useMemo(() => {
    let boxes = meta?.af?.boxes ?? null;
    if (!boxes) return boxes;
    if (turn) boxes = boxes.map((b) => turnRect(b, turn));
    if (angle && fullDims)
      boxes = boxes.map((b) => {
        const [cx, cy] = spin(b.x + b.w / 2, b.y + b.h / 2, angle, fullDims);
        return { ...b, x: cx - b.w / 2, y: cy - b.h / 2 };
      });
    if (!frame) return boxes;
    return boxes
      .map((b) => ({ ...b, x: (b.x - frame[0]) / frame[2], y: (b.y - frame[1]) / frame[3], w: b.w / frame[2], h: b.h / frame[3] }))
      .filter((b) => b.x + b.w / 2 > 0 && b.x + b.w / 2 < 1 && b.y + b.h / 2 > 0 && b.y + b.h / 2 < 1);
  }, [meta, turn, angle, fullDims, frame?.[0], frame?.[1], frame?.[2], frame?.[3]]);

  // The photo shows the scene turned the other way from the camera, then by however far it is straightened.
  const tilt = props.showLevel ? tiltOf(meta) : null;
  const levelShown = tilt === null ? null : { horizon: angle - tilt, pitch: meta?.level?.pitch ?? null };

  return (
    <div
      ref={box}
      className={`pane ${props.active ? "active" : ""} ${props.dimmed ? "dimmed" : ""} ${fitMode ? "" : "zoomed"}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onLostPointerCapture={onPointerUp}
      onDoubleClick={(e) => !gesture.current.touching && (props.onBack ? props.onBack() : toggleAt(e))}
      onContextMenu={(e) => e.preventDefault()}
    >
      {dims && fullDims && upright ? (
        <div
          className="canvas"
          style={{ width: dims.w, height: dims.h, transform: `translate3d(${view.tx}px, ${view.ty}px, 0) scale(${view.scale})` }}
        >
          <div
            className="canvas-full"
            style={{
              width: fullDims.w,
              height: fullDims.h,
              left: -(frame?.[0] ?? 0) * fullDims.w,
              top: -(frame?.[1] ?? 0) * fullDims.h,
              transform: angle ? `rotate(${angle}deg)` : undefined,
            }}
          >
          <div style={turnStyle(turn, upright)}>
          <img className="tier" src={screenUrl} alt="" draggable={false} decoding="async" onLoad={onPicture} onError={onPicture} />
          {wantFull && (
            <img
              key={fullUrl}
              className={`tier full ${fullReady ? "ready" : ""}`}
              src={fullUrl}
              alt=""
              draggable={false}
              decoding="async"
              onLoad={(e) => onFullLoad(e, fullUrl)}
              onError={() => setFailedUrl(fullUrl)}
            />
          )}
          {clipUrl && <img key={clipUrl} className="tier clip" src={clipUrl} alt="" draggable={false} decoding="async" />}
          </div>
          </div>
        </div>
      ) : (
        <img
          className="placeholder"
          src={imageUrl("thumb", item.id, item.mtime, 512)}
          alt=""
          draggable={false}
          style={turn ? { transform: `rotate(${turn * 90}deg)` } : undefined}
          onLoad={onPicture}
          onError={onPicture}
        />
      )}
      {dims && (
        <Overlay
          width={dims.w * view.scale}
          height={dims.h * view.scale}
          pixelWidth={dims.w}
          pixelHeight={dims.h}
          style={{ transform: `translate3d(${view.tx}px, ${view.ty}px, 0)` }}
          af={props.showAf ? afBoxes : null}
          guides={props.guides}
          flip={props.flip}
          crop={props.crop}
          angle={angle}
          cropRatio={props.cropRatio}
          onCrop={props.onCrop}
          level={levelShown}
        />
      )}
    </div>
  );
});
