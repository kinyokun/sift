import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { NO_LOOK, imageUrl, isRawPath, type Item, type StackKind } from "../lib/api";
import { useT } from "../lib/i18n";
import { actionFor, isTyping, keyOf, ratingOf } from "../lib/shortcuts";
import { useShallow } from "zustand/react/shallow";
import { flagOf, folderName, framesOf, lookOf, useStore, visibleEntries, type Entry, type FlagFilter, type SortBy } from "../lib/store";
import { IconButton, Menu, Segmented, Stars, type MenuItem } from "../ui/controls";
import { Icon } from "../ui/Icon";
import { useWheelSize } from "../lib/wheelSize";
import { chooseFolder, importPhotos } from "./Welcome";
import { MAX_PANES } from "./Compare";

const GAP = 6;
const THUMB_EDGE = 512;
/** Smallest cell size of the slider and Ctrl + wheel; the largest is two cells to a row. */
const THUMB_MIN = 110;
/** Thumbnails for cells wider than this many device pixels are asked for at twice the size. */
const THUMB_SHARP = 600;
const OVERSCAN_ROWS = 4;
const CELL_INSET = 5;
/** A new cell waits this long before asking for its thumbnail, so rows flung past cost nothing. */
const SETTLE_MS = 60;
/** A press held this long on a touch screen starts selecting. */
const HOLD_MS = 420;
/** A drag shorter than this is still a click. */
const DRAG_SLOP = 6;
/** How long a click waits to see whether it is a double click. */
const DOUBLE_CLICK_MS = 320;
/** Dragging a selection this close to the top or bottom scrolls the grid. */
const SCROLL_EDGE = 56;

/** Width/height of thumbnails already seen, so badges sit on the picture itself. */
const ratios = new Map<string, number>();

type Box = { x0: number; y0: number; x1: number; y1: number };

const Cell = memo(function Cell(props: {
  /** The grid cell's id: the photo's own, or the first frame of a folded stack. */
  id: string;
  /** The photo shown. */
  item: Item;
  /** Folded stack: how many frames it holds. Opened stack: this frame's place in it. */
  stack: StackKind | null;
  stackKey: string;
  stackOpen: boolean;
  stackIndex: number;
  stackTotal: number;
  stackLabel: string;
  rating: number;
  flag: number;
  /** Name of the look chosen for this photo, if any. */
  look: string;
  selected: boolean;
  /** The photo the keyboard acts on when nothing is selected. */
  cursor: boolean;
  size: number;
  left: number;
  top: number;
  checkLabel: string;
  onPress: (id: string, shown: string, e: React.MouseEvent) => void;
  onCheck: (id: string, e: React.MouseEvent) => void;
  onOpen: (id: string, shown: string, e: React.MouseEvent) => void;
  /** Opens or folds the stack. */
  onStack: (key: string) => void;
  /** Touch screens: a long press. */
  onHold?: (id: string) => void;
}) {
  const { id, item, rating, flag, selected, size } = props;
  const folded = !!props.stack && !props.stackOpen;
  const seen = ratios.has(item.id);
  const [ratio, setRatio] = useState(ratios.get(item.id) ?? 0);
  const [asked, setAsked] = useState(seen);
  const [ready, setReady] = useState(seen);
  const hold = useRef<{ timer: ReturnType<typeof setTimeout>; x: number; y: number; fired: boolean } | null>(null);
  useEffect(() => {
    if (asked) return;
    const timer = setTimeout(() => setAsked(true), SETTLE_MS);
    return () => clearTimeout(timer);
  }, [asked]);
  useEffect(() => () => clearTimeout(hold.current?.timer), []);

  const inner = size - CELL_INSET * 2;
  const frame = ratio ? { width: ratio >= 1 ? inner : inner * ratio, height: ratio >= 1 ? inner / ratio : inner } : { width: inner, height: inner };
  const { onHold } = props;
  const touch = onHold && {
    onPointerDown: (e: React.PointerEvent) => {
      clearTimeout(hold.current?.timer);
      const state = { timer: setTimeout(() => ((state.fired = true), onHold(id)), HOLD_MS), x: e.clientX, y: e.clientY, fired: false };
      hold.current = state;
    },
    onPointerMove: (e: React.PointerEvent) => {
      const h = hold.current;
      if (h && Math.hypot(e.clientX - h.x, e.clientY - h.y) > 8) clearTimeout(h.timer);
    },
    onPointerUp: () => clearTimeout(hold.current?.timer),
    onPointerCancel: () => clearTimeout(hold.current?.timer),
    onContextMenu: (e: React.MouseEvent) => e.preventDefault(),
  };
  return (
    <div
      className={`cell ${selected ? "selected" : ""} ${props.cursor && !selected ? "cursor" : ""} ${flag === -1 ? "rejected" : ""} ${folded ? "stacked" : ""}`}
      style={{ width: size, height: size, transform: `translate(${props.left}px, ${props.top}px)` }}
      onClick={(e) => {
        // The click that ends a long press has already been acted on.
        if (hold.current?.fired) return void (hold.current = null);
        props.onPress(id, item.id, e);
      }}
      onDoubleClick={(e) => props.onOpen(id, item.id, e)}
      {...touch}
    >
      <div className={`cell-frame ${ready ? "" : "pending"}`} style={frame}>
        {asked && (
          <img
            src={imageUrl("thumb", item.id, item.mtime, size * devicePixelRatio > THUMB_SHARP ? THUMB_EDGE * 2 : THUMB_EDGE)}
            alt=""
            decoding="async"
            draggable={false}
            onLoad={(e) => {
              const r = e.currentTarget.naturalWidth / Math.max(1, e.currentTarget.naturalHeight);
              ratios.set(item.id, r);
              setRatio(r);
              setReady(true);
            }}
            onError={() => setReady(true)}
          />
        )}
        <div className="cell-badges">
          {rating > 0 && (
            <span className="cell-stars">
              {rating}
              <Icon name="star" size={10} fill />
            </span>
          )}
          {item.raw && <span className="cell-raw">{item.paired ? "RAW+J" : "RAW"}</span>}
          {item.live && (
            <span className="cell-live">
              <Icon name="live" size={10} />
              LIVE
            </span>
          )}
          {props.look && (
            <span className="cell-look" title={props.look}>
              <Icon name="look" size={11} />
            </span>
          )}
        </div>
        {props.stack && (
          <button
            className={`cell-stack ${props.stack}`}
            title={props.stackLabel}
            aria-label={props.stackLabel}
            tabIndex={-1}
            onClick={(e) => {
              e.stopPropagation();
              props.onStack(props.stackKey);
            }}
            onDoubleClick={(e) => e.stopPropagation()}
          >
            <Icon name={props.stack === "pixelshift" ? "shift" : "stack"} size={12} />
            {folded ? props.stackTotal : `${props.stackIndex}/${props.stackTotal}`}
          </button>
        )}
        {flag === 1 && (
          <span className="cell-flag pick">
            <Icon name="check" size={12} />
          </span>
        )}
        {flag === -1 && (
          <span className="cell-flag reject">
            <Icon name="x" size={12} />
          </span>
        )}
      </div>
      <button
        className="cell-check"
        role="checkbox"
        aria-checked={selected}
        aria-label={props.checkLabel}
        title={props.checkLabel}
        tabIndex={-1}
        onClick={(e) => {
          e.stopPropagation();
          props.onCheck(id, e);
        }}
        onDoubleClick={(e) => e.stopPropagation()}
      >
        {selected && <Icon name="check" size={13} />}
      </button>
    </div>
  );
});

export function Library() {
  const t = useT();
  const s = useStore(
    useShallow((st) => ({
      info: st.info,
      folder: st.folder,
      loading: st.loading,
      items: st.items,
      ratings: st.ratings,
      dates: st.dates,
      flags: st.flags,
      looks: st.looks,
      library: st.library,
      filter: st.filter,
      minStars: st.minStars,
      stacks: st.stacks,
      stackOf: st.stackOf,
      expanded: st.expanded,
      stacking: st.settings.stacks,
      selection: st.selection,
      cursor: st.cursor,
      thumb: st.settings.thumb,
      sortBy: st.settings.sortBy,
      sortDesc: st.settings.sortDesc,
      viewing: !!st.viewer,
    })),
  );
  const act = useStore.getState();
  // The cells of the grid; a folded stack is one of them.
  const items: Entry[] = useMemo(
    () => visibleEntries({ ...s, settings: { sortBy: s.sortBy, sortDesc: s.sortDesc, stacks: s.stacking } }),
    // Capture times only matter when sorting by them.
    [s.items, s.filter, s.minStars, s.flags, s.ratings, s.sortBy, s.sortDesc, s.sortBy === "time" ? s.dates : null, s.stacks, s.expanded, s.stacking],
  );
  const photoCount = useMemo(() => items.reduce((n, e) => n + e.members.length, 0), [items]);
  /** The photos an action on these cells reaches: every frame of a folded stack. */
  const frames = useCallback((ids: string[]) => framesOf(items, ids), [items]);
  const mobile = !!s.info?.mobile;
  const scroller = useRef<HTMLDivElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const header = useRef<HTMLElement>(null);
  const [box, setBox] = useState({ width: 0, height: 0, head: 0 });
  /** First row under the toolbar; the grid re-renders when it changes, not on every scroll event. */
  const [row, setRow] = useState(0);
  /** Touch screens: taps toggle photos instead of opening them. */
  const [selecting, setSelecting] = useState(false);
  const [marquee, setMarquee] = useState<Box | null>(null);
  /** Where a range starts, and what was selected before it. */
  const anchor = useRef<{ id: string | null; base: string[] }>({ id: null, base: [] });
  /** Pending "select just this one" after a click inside a selection. */
  const collapse = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(collapse.current), []);
  /** Set while the click that ends a drag is still to come. */
  const dragged = useRef(false);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    // The grid scrolls underneath the translucent toolbar.
    const measure = () => setBox({ width: el.clientWidth, height: el.clientHeight, head: header.current?.offsetHeight ?? 0 });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    if (header.current) ro.observe(header.current);
    return () => ro.disconnect();
  }, []);

  // Columns fill the width exactly; the slider sets the target cell size.
  const pad = mobile ? 4 : 14;
  const inner = Math.max(0, box.width - pad * 2);
  // The largest cells are two to a row; zooming on from there opens the photo.
  const thumbMax = Math.max(THUMB_MIN, Math.floor((inner - GAP) / 2));
  const target = mobile ? Math.min(s.thumb, 132) : Math.min(s.thumb, thumbMax);
  const cols = Math.max(mobile ? 3 : 2, Math.floor((inner + GAP) / (target + GAP)));
  const size = Math.max(40, (inner - GAP * (cols - 1)) / cols);
  const pitch = size + GAP;
  const rows = Math.ceil(items.length / cols);
  const first = Math.max(0, row - OVERSCAN_ROWS);
  const last = Math.min(rows, row + Math.ceil(box.height / pitch) + 1 + OVERSCAN_ROWS);
  const shown = items.slice(first * cols, last * cols);

  const rowAt = useCallback((top: number) => Math.max(0, Math.floor((top - pad) / pitch)), [pad, pitch]);
  useLayoutEffect(() => setRow(rowAt(scroller.current?.scrollTop ?? 0)), [rowAt, items]);

  const selected = useMemo(() => new Set(s.selection), [s.selection]);
  const indexOf = useCallback((id: string | null) => (id ? items.findIndex((it) => it.id === id) : -1), [items]);

  // A new folder, filter or order starts ranges afresh.
  useEffect(() => {
    anchor.current = { id: null, base: [] };
  }, [s.folder, s.filter, s.minStars]);
  useEffect(() => setSelecting(false), [s.folder]);
  /** The cell under a point of the window, and how far down it the point is. */
  const cellAt = (at: { clientX: number; clientY: number }) => {
    const el = scroller.current;
    if (!el) return null;
    const r = el.getBoundingClientRect();
    const col = Math.floor((at.clientX - r.left - pad) / pitch);
    const y = at.clientY - r.top + el.scrollTop - box.head - pad;
    const index = Math.floor(y / pitch) * cols + col;
    if (col < 0 || col >= cols || y < 0 || index >= items.length) return null;
    return { index, part: (y % pitch) / pitch, y: at.clientY - r.top };
  };
  /** The cell the wheel is resizing the grid around, to be put back under the pointer. */
  const held = useRef<{ index: number; part: number; y: number } | null>(null);
  const settled = useRef(false);
  useWheelSize(scroller, {
    get: () => useStore.getState().settings.thumb,
    set: (thumb, at) => {
      held.current = cellAt(at);
      act.setSettings({ thumb });
    },
    min: THUMB_MIN,
    max: thumbMax,
    past: (at) => {
      const entry = items[cellAt(at)?.index ?? indexOf(s.cursor)] ?? items[row * cols];
      if (!entry) return;
      act.select(useStore.getState().selection, entry.id);
      act.openViewer(entry.item.id);
    },
    on: !mobile,
  });
  useLayoutEffect(() => {
    const el = scroller.current;
    const at = held.current;
    held.current = null;
    if (!el || !at) return;
    el.scrollTop = box.head + pad + (Math.floor(at.index / cols) + at.part) * pitch - at.y;
    settled.current = true;
  }, [cols, pitch]);
  // A turn of the wheel that left the rows as they were holds nothing.
  useEffect(() => void (held.current = null));

  /** Adds or removes one photo; ranges then start from it. */
  const toggle = useCallback((id: string) => {
    const st = useStore.getState();
    const next = st.selection.includes(id) ? st.selection.filter((x) => x !== id) : [...st.selection, id];
    anchor.current = { id, base: next };
    st.select(next, id);
  }, []);

  /** Selects everything from the anchor to `id`, on top of what was selected before the range began. */
  const extendTo = useCallback(
    (id: string) => {
      const st = useStore.getState();
      const to = indexOf(id);
      if (to < 0) return;
      let from = indexOf(anchor.current.id);
      if (from < 0) {
        from = indexOf(st.cursor);
        anchor.current = { id: from < 0 ? id : st.cursor, base: [] };
      }
      if (from < 0) from = to;
      const now = new Set(st.selection);
      const range = items.slice(Math.min(from, to), Math.max(from, to) + 1).map((it) => it.id);
      const ids = new Set([...anchor.current.base.filter((x) => now.has(x)), ...range]);
      st.select([...ids], id);
    },
    [items, indexOf],
  );

  const onPress = useCallback(
    (id: string, shown: string, e: React.MouseEvent) => {
      if (dragged.current) return;
      clearTimeout(collapse.current);
      const st = useStore.getState();
      if (st.info?.mobile) return selecting ? toggle(id) : st.openViewer(shown);
      if (e.shiftKey) return extendTo(id);
      if (e.metaKey || e.ctrlKey) return toggle(id);
      const only = () => {
        anchor.current = { id, base: [] };
        useStore.getState().select([id]);
      };
      // A click on one of several selected photos may be the first half of a
      // double click that opens it; the selection is kept until that is ruled out.
      if (st.selection.length > 1 && st.selection.includes(id)) collapse.current = setTimeout(only, DOUBLE_CLICK_MS);
      else only();
    },
    [selecting, toggle, extendTo],
  );
  const onCheck = useCallback((id: string, e: React.MouseEvent) => (e.shiftKey ? extendTo(id) : toggle(id)), [toggle, extendTo]);
  // A folded stack opens in the viewer on the frame it shows; its other frames follow there.
  const onOpen = useCallback((id: string, shown: string, e: React.MouseEvent) => {
    if (e.shiftKey || e.metaKey || e.ctrlKey) return;
    clearTimeout(collapse.current);
    const st = useStore.getState();
    st.select(st.selection, id);
    st.openViewer(shown);
  }, []);
  const onHold = useCallback((id: string) => {
    setSelecting(true);
    anchor.current = { id, base: [id] };
    useStore.getState().select([id]);
  }, []);
  const onStack = useCallback((key: string) => useStore.getState().toggleStack(key), []);

  const clearSelection = useCallback(() => {
    const st = useStore.getState();
    anchor.current = { id: null, base: [] };
    setSelecting(false);
    st.select([], st.cursor);
  }, []);
  const selectAll = useCallback(() => {
    const st = useStore.getState();
    st.select(items.map((it) => it.id), st.cursor);
  }, [items]);

  // Dragging across the grid selects the photos the rectangle touches.
  const geometry = useRef({ cols, pitch, size, pad, items });
  geometry.current = { cols, pitch, size, pad, items };
  const onPointerDown = (e: React.PointerEvent) => {
    const el = scroller.current;
    const grid = gridRef.current;
    if (!el || !grid || e.pointerType !== "mouse" || e.button !== 0) return;
    const target = e.target as HTMLElement;
    if (target.closest("button")) return;
    const frame = el.getBoundingClientRect();
    // The scrollbar belongs to the scroller too.
    if (e.clientX > frame.left + el.clientWidth) return;
    const origin = grid.getBoundingClientRect();
    const start = { x: e.clientX - origin.left, y: e.clientY - origin.top };
    const additive = e.shiftKey || e.metaKey || e.ctrlKey;
    const base = additive ? useStore.getState().selection : [];
    const onCell = !!target.closest(".cell");
    let at = { x: e.clientX, y: e.clientY };
    let active = false;
    let raf = 0;
    let nearest: string | null = null;

    const update = () => {
      const g = geometry.current;
      const r = grid.getBoundingClientRect();
      const x = Math.min(r.width, Math.max(0, at.x - r.left));
      const y = Math.min(r.height, Math.max(0, at.y - r.top));
      const area = { x0: Math.min(start.x, x), y0: Math.min(start.y, y), x1: Math.max(start.x, x), y1: Math.max(start.y, y) };
      setMarquee(area);
      const span = (lo: number, hi: number, count: number) => [Math.max(0, Math.ceil((lo - g.pad - g.size) / g.pitch)), Math.min(count - 1, Math.floor((hi - g.pad) / g.pitch))];
      const [c0, c1] = span(area.x0, area.x1, g.cols);
      const [r0, r1] = span(area.y0, area.y1, Math.ceil(g.items.length / g.cols));
      const hits: string[] = [];
      let best = Infinity;
      nearest = null;
      for (let rr = r0; rr <= r1; rr++) {
        for (let c = c0; c <= c1; c++) {
          const item = g.items[rr * g.cols + c];
          if (!item) continue;
          hits.push(item.id);
          const d = Math.hypot(g.pad + c * g.pitch + g.size / 2 - x, g.pad + rr * g.pitch + g.size / 2 - y);
          if (d < best) [best, nearest] = [d, item.id];
        }
      }
      const st = useStore.getState();
      const ids = [...new Set([...base, ...hits])];
      if (ids.length !== st.selection.length || ids.some((id, i) => st.selection[i] !== id)) st.select(ids, st.cursor);
    };
    const step = () => {
      const view = el.getBoundingClientRect();
      const top = view.top + (header.current?.offsetHeight ?? 0);
      const over = at.y < top + SCROLL_EDGE ? at.y - (top + SCROLL_EDGE) : at.y > view.bottom - SCROLL_EDGE ? at.y - (view.bottom - SCROLL_EDGE) : 0;
      if (over) {
        const before = el.scrollTop;
        el.scrollTop += Math.sign(over) * Math.min(28, 4 + Math.abs(over) / 3);
        if (el.scrollTop !== before) update();
      }
      raf = requestAnimationFrame(step);
    };
    const move = (ev: PointerEvent) => {
      at = { x: ev.clientX, y: ev.clientY };
      if (!active) {
        if (Math.hypot(at.x - e.clientX, at.y - e.clientY) < DRAG_SLOP) return;
        active = true;
        dragged.current = true;
        raf = requestAnimationFrame(step);
      }
      update();
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      cancelAnimationFrame(raf);
      setMarquee(null);
      const st = useStore.getState();
      if (active) {
        // The click that follows this release is part of the drag.
        setTimeout(() => (dragged.current = false), 0);
        anchor.current = { id: nearest, base: st.selection };
        if (nearest) st.select(st.selection, nearest);
      } else if (!onCell && !additive && st.selection.length) clearSelection();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  };

  // Keep the cursor cell on screen.
  useEffect(() => {
    const i = indexOf(s.cursor);
    const el = scroller.current;
    // The wheel has just put the grid where the pointer is.
    if (settled.current) return void (settled.current = false);
    if (i < 0 || !el || dragged.current) return;
    const top = box.head + Math.floor(i / cols) * pitch;
    if (top < el.scrollTop + box.head) el.scrollTop = top - box.head;
    else if (top + pitch + pad * 2 > el.scrollTop + el.clientHeight) el.scrollTop = top + pitch + pad * 2 - el.clientHeight;
  }, [s.cursor, cols, pitch, indexOf, pad, box.head]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const st = useStore.getState();
      if (st.viewer || st.sheet || isTyping(e)) return;
      const mod = e.metaKey || e.ctrlKey;
      if (mod && keyOf(e) === "A") {
        e.preventDefault();
        return selectAll();
      }
      if (e.key === "Escape" && (st.selection.length > 1 || selecting)) {
        e.preventDefault();
        setSelecting(false);
        anchor.current = { id: null, base: [] };
        return st.select(st.cursor && !selecting ? [st.cursor] : [], st.cursor);
      }
      if (mod || !items.length) return;
      const at = indexOf(st.cursor);
      const move = (delta: number) => {
        e.preventDefault();
        const next = items[Math.min(items.length - 1, Math.max(0, at < 0 ? 0 : at + delta))].id;
        if (e.shiftKey && at >= 0) return extendTo(next);
        anchor.current = { id: next, base: [] };
        st.select([next]);
      };
      const cells = st.selection.length ? st.selection : st.cursor ? [st.cursor] : [];
      const members = frames(cells);
      const targets = members.map((m) => m.id);
      switch (e.key) {
        case "ArrowRight":
          return move(1);
        case "ArrowLeft":
          return move(-1);
        case "ArrowDown":
          return move(cols);
        case "ArrowUp":
          return move(-cols);
        case "Enter":
          if (at >= 0) {
            e.preventDefault();
            st.openViewer(items[at].item.id);
          }
          return;
      }
      if (!targets.length) return;
      const action = actionFor(st.settings.keys, e);
      const stars = ratingOf(action);
      if (stars !== null) return void st.rate(targets, stars);
      const allFlagged = (v: number) => members.every((m) => flagOf(st.flags, m) === v);
      if (action === "pick") st.flag(targets, allFlagged(1) ? 0 : 1);
      else if (action === "reject") st.flag(targets, allFlagged(-1) ? 0 : -1);
      else if (action === "unflag") st.flag(targets, 0);
      else if (action === "next") move(1);
      else if (action === "prev") move(-1);
      else if (action === "compare" && targets.length > 1 && targets.length <= MAX_PANES) st.openViewer(targets[0], targets);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [items, cols, indexOf, extendTo, selectAll, selecting, frames]);

  const counts = useMemo(() => {
    let picked = 0;
    let rejected = 0;
    for (const it of s.items) {
      const f = flagOf(s.flags, it);
      if (f === 1) picked++;
      else if (f === -1) rejected++;
    }
    return { picked, rejected };
  }, [s.items, s.flags]);

  const filters: { value: FlagFilter; label: React.ReactNode }[] = [
    { value: "all", label: t.filterAll },
    { value: "picked", label: <>{t.filterPicked}{counts.picked > 0 && <i>{counts.picked}</i>}</> },
    { value: "unflagged", label: t.filterUnflagged },
    { value: "rejected", label: <>{t.filterRejected}{counts.rejected > 0 && <i>{counts.rejected}</i>}</> },
  ];

  const starItems: MenuItem[] = [0, 1, 2, 3, 4, 5].map((n) => ({
    label: n === 0 ? t.anyStars : t.starsAtLeast(n),
    checked: s.minStars === n,
    onSelect: () => act.setMinStars(n),
  }));

  const sorts: [SortBy, string][] = [
    ["name", t.sortName],
    ["time", t.sortTime],
    ["rating", t.sortRating],
    ["size", t.sortSize],
  ];
  const sortItems: MenuItem[] = [
    ...sorts.map(([by, label]) => ({ label, checked: s.sortBy === by, onSelect: () => act.setSettings({ sortBy: by }) })),
    "divider",
    { label: t.sortAsc, checked: !s.sortDesc, onSelect: () => act.setSettings({ sortDesc: false }) },
    { label: t.sortDesc, checked: s.sortDesc, onSelect: () => act.setSettings({ sortDesc: true }) },
  ];

  /** Sets one look on every selected photo; "original" only where there is a RAW. */
  const applyLook = (look: string | null) => {
    const picked = frames(s.selection.length ? s.selection : s.cursor ? [s.cursor] : []);
    const ids = (look === NO_LOOK ? picked.filter((it) => it.paths.some(isRawPath)) : picked).map((it) => it.id);
    if (!picked.length) return act.notify("selectFirst");
    if (!ids.length) return act.notify("originalNeedsRaw");
    act.setLook(ids, look);
    act.notify(`lookApplied:${ids.length}`);
  };
  const lookNames = useMemo(() => new Map(s.library.map((l) => [l.id, l.name])), [s.library]);
  const lookItems: MenuItem[] = [
    { label: t.lookAsShot, onSelect: () => applyLook(null) },
    { label: t.original, onSelect: () => applyLook(NO_LOOK) },
    ...(s.library.length ? ["divider" as const] : []),
    ...s.library.map((l) => ({ label: l.name, onSelect: () => applyLook(l.id) })),
  ];

  const switchFolder = async () => {
    try {
      const dir = mobile ? await importPhotos() : await chooseFolder();
      if (dir) await act.openFolder(dir);
    } catch (e) {
      act.notify(String(e));
    }
  };

  const moreItems: MenuItem[] = [
    { label: mobile ? t.importPhotos : t.openFolder, icon: mobile ? "plus" : "folder", onSelect: switchFolder },
    ...(mobile ? [{ label: t.selectPhotos, icon: "check", onSelect: () => setSelecting(true) } as MenuItem] : []),
    ...(mobile ? [] : [{ label: t.findRaw, icon: "raw", onSelect: () => act.setSheet("findraw") } as MenuItem]),
    "divider",
    ...(s.stacks.length ? [{ label: t.stackPhotos, checked: s.stacking, onSelect: () => act.setSettings({ stacks: !s.stacking }) } as MenuItem, "divider" as const] : []),
    { label: t.settings, icon: "gear", onSelect: () => act.setSheet("settings") },
    { label: t.help, icon: "help", onSelect: () => act.setSheet("help") },
  ];

  const lookLabel = (id: string | null) => (!id ? "" : id === NO_LOOK ? t.original : (lookNames.get(id) ?? id));

  // The bar for acting on several photos at once.
  const chosen = useMemo(() => frames(s.selection).map((m) => m.id), [frames, s.selection]);
  // One folded stack is already several photos.
  // The bar floats above everything, so it would cover the viewer's dock.
  const bar = !s.viewing && (mobile ? selecting : s.selection.length > 1);
  const sharedRating = chosen.length && chosen.every((id) => (s.ratings[id] || 0) === (s.ratings[chosen[0]] || 0)) ? s.ratings[chosen[0]] || 0 : 0;
  const flagAll = (value: number) => chosen.length && act.flag(chosen, value);

  return (
    <div className="library">
      <header className="toolbar glass" ref={header} data-tauri-drag-region>
        <div className="toolbar-side">
          <IconButton icon="back" label={t.back} onClick={act.closeFolder} />
          <div className="toolbar-title" data-tauri-drag-region>
            <strong>{s.folder ? folderName(s.folder) : ""}</strong>
            <span>{s.loading ? t.readingFolder : t.photos(photoCount)}</span>
          </div>
        </div>
        <Segmented className="toolbar-filter" value={s.filter} options={filters} onChange={act.setFilter} />
        <div className="toolbar-side end">
          {!mobile && (
            <input
              className="size-slider"
              type="range"
              min={THUMB_MIN}
              max={thumbMax}
              step={2}
              value={Math.min(s.thumb, thumbMax)}
              title={`${t.size} · ${t.pinchOrCtrl}`}
              aria-label={t.size}
              onChange={(e) => act.setSettings({ thumb: Number(e.target.value) })}
            />
          )}
          <Menu label={t.sort} items={sortItems} button={<Icon name="sort" />} />
          <Menu label={t.stars} items={starItems} button={<><Icon name="star" fill={s.minStars > 0} />{s.minStars > 0 && <b>{s.minStars}</b>}</>} />
          <Menu label={t.lookApply} items={lookItems} button={<Icon name="look" />} />
          <IconButton icon="transfer" label={t.transfer} onClick={() => act.setSheet("transfer")} />
          <IconButton icon="export" label={t.export} onClick={() => act.setSheet("export")} />
          <Menu label={t.more} items={moreItems} button={<Icon name="more" />} />
        </div>
      </header>

      <div className="grid-scroller" ref={scroller} onScroll={(e) => setRow(rowAt(e.currentTarget.scrollTop))} onPointerDown={mobile ? undefined : onPointerDown}>
        <div
          className={`grid ${bar ? "selecting" : ""}`}
          ref={gridRef}
          style={{ height: rows * pitch - GAP + pad * 2 + (bar ? 76 : 0), marginTop: box.head }}
        >
          {shown.map((entry, k) => {
            const i = first * cols + k;
            const { item, stack } = entry;
            return (
              <Cell
                key={entry.id}
                id={entry.id}
                item={item}
                stack={stack?.kind ?? null}
                stackKey={stack?.key ?? ""}
                stackOpen={!!stack?.open}
                stackIndex={stack?.index ?? 0}
                stackTotal={stack?.total ?? 0}
                stackLabel={!stack ? "" : `${stack.kind === "pixelshift" ? t.pixelShift : t.burst} · ${stack.open ? t.stackFold : t.stackOpen(stack.total)}`}
                rating={s.ratings[item.id] || 0}
                flag={flagOf(s.flags, item)}
                look={lookLabel(lookOf(s.looks, item))}
                selected={selected.has(entry.id)}
                cursor={entry.id === s.cursor}
                size={size}
                left={pad + (i % cols) * pitch}
                top={pad + Math.floor(i / cols) * pitch}
                checkLabel={t.selectPhoto}
                onPress={onPress}
                onCheck={onCheck}
                onOpen={onOpen}
                onStack={onStack}
                onHold={mobile ? onHold : undefined}
              />
            );
          })}
          {marquee && <div className="marquee" style={{ left: marquee.x0, top: marquee.y0, width: marquee.x1 - marquee.x0, height: marquee.y1 - marquee.y0 }} />}
        </div>
        {s.loading && (
          <div className="empty">
            <span className="spinner" />
            <p>{t.readingFolder}</p>
          </div>
        )}
        {!s.loading && items.length === 0 && (
          <div className="empty">
            <Icon name="folder" size={44} />
            <p>{s.items.length ? t.emptyFilter : t.emptyFolder}</p>
          </div>
        )}
      </div>

      {bar && (
        <div className="selection-bar glass" role="toolbar" aria-label={t.selectedCount(chosen.length)}>
          <strong>{t.selectedCount(chosen.length)}</strong>
          <Stars value={sharedRating} size={17} onChange={(n) => chosen.length && act.rate(chosen, n)} />
          <span className="selection-flags">
            <button className="flag-button pick" title={t.pick} aria-label={t.pick} onClick={() => flagAll(1)}>
              <Icon name="check" size={15} />
            </button>
            <button className="flag-button reject" title={t.reject} aria-label={t.reject} onClick={() => flagAll(-1)}>
              <Icon name="x" size={15} />
            </button>
            <button className="flag-button" title={t.actions.unflag} aria-label={t.actions.unflag} onClick={() => flagAll(0)}>
              <Icon name="flagOff" size={15} />
            </button>
          </span>
          <Menu label={t.lookApply} items={lookItems} button={<Icon name="look" />} up />
          {s.info?.save_to_photos && <IconButton icon="save" label={t.saveToPhotos} onClick={() => act.saveToPhotos(frames(s.selection))} />}
          {chosen.length > 1 && chosen.length <= MAX_PANES && (
            <IconButton icon="compare" label={t.compareSelected} onClick={() => act.openViewer(chosen[0], chosen)} />
          )}
          <button className="text-button select-all" onClick={selectAll}>
            {t.selectAll}
          </button>
          <button className="text-button strong" onClick={clearSelection}>
            {mobile ? t.done : t.deselect}
          </button>
        </div>
      )}
    </div>
  );
}
