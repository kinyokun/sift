import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { NO_LOOK, api, fileName, imageUrl, isRawPath, type CropBox, type Item, type Meta } from "../lib/api";
import { useT } from "../lib/i18n";
import { actionFor, isTyping, ratingOf } from "../lib/shortcuts";
import { cropOf, flagOf, lookOf, useStore } from "../lib/store";
import { IconButton, Menu, Stars, type MenuItem } from "../ui/controls";
import { Icon } from "../ui/Icon";
import { CROP_RATIOS, GUIDE_MODES, ROTATABLE_GUIDES, centeredCrop, type Crop } from "./Overlay";
import { Histogram } from "./Histogram";
import { LookWall } from "./LookWall";
import { LiveEditor, LiveMovie, LivePreview, MIN_LIVE_FRAMES, type LiveDraft } from "./Live";
import { StackStrip } from "./StackStrip";
import { Pane, fullUrlOf, lookSource, type Nudge, type PaneHandle, type PaneStatus } from "./Pane";

export function formatShutter(s: number): string {
  return s >= 1 ? `${Number(s.toFixed(1))}s` : `1/${Math.round(1 / s)}s`;
}

export function exposureLine(meta: Meta): string {
  const x = meta.exif;
  return [
    x.focal ? `${Math.round(x.focal)}mm` : "",
    x.aperture ? `f/${Number(x.aperture.toFixed(1))}` : "",
    x.shutter ? formatShutter(x.shutter) : "",
    x.iso ? `ISO ${x.iso}` : "",
    x.bias ? `${x.bias > 0 ? "+" : ""}${Number(x.bias.toFixed(1))} EV` : "",
  ]
    .filter(Boolean)
    .join("  ");
}

type InfoProps = {
  item: Item;
  meta: Meta | null;
  docked: boolean;
  crop: CropBox | null;
  /** What the histogram is taken from; `null` while a look is still rendering. */
  histogram: { path: string; look: string | null; edge: number } | null;
  /** Keeps the in-camera recipe as a look of its own; absent when the photo has none to keep. */
  onSaveLook?: () => void;
};

function InfoPanel({ item, meta, docked, crop, histogram, onSaveLook }: InfoProps) {
  const t = useT();
  const x = meta?.exif;
  const camera = x ? [x.make?.split(" ")[0], x.model].filter((v, i, a) => v && !(i === 0 && a[1]?.toUpperCase().includes(v.toUpperCase()))).join(" ") : "";
  const look = meta?.look;
  // Only worth a mention when the sensor is not full frame.
  const equivalent = x?.focal35 && x.focal && Math.abs(x.focal35 - x.focal) > 1 ? t.equiv(x.focal35) : "";
  const shooting = [t.programs[x?.program ?? 0], t.meterings[x?.metering ?? 0], x?.flash ? t.flashFired : ""].filter(Boolean).join("  ·  ");
  const rows: [string, React.ReactNode][] = [
    [t.file, [...item.paths, ...(item.live ? [item.live] : [])].map((p) => p.split(/[\\/]/).pop()).join("  +  ")],
    [t.camera, camera],
    [t.lens, x?.lens],
    [t.exposure, meta ? [exposureLine(meta), equivalent].filter(Boolean).join("  ·  ") : ""],
    [t.shooting, shooting],
    [t.whiteBalance, x?.manual_wb == null ? "" : x.manual_wb ? t.wbManual : t.wbAuto],
    [
      t.look,
      look && (
        <>
          {look.name}
          <em className={look.custom ? "recipe" : ""}>{look.custom ? t.lookRecipe : t.lookBuiltIn}</em>
          {onSaveLook && (
            <button className="link save-look" onClick={onSaveLook}>
              {t.lookSave}
            </button>
          )}
        </>
      ),
    ],
    [t.afMode, meta?.af?.mode],
    [t.taken, x?.datetime],
    [t.dimensions, x?.width && x.height ? `${x.width} × ${x.height}` : ""],
    [t.fileSize, `${(item.size / 1048576).toFixed(1)} MB`],
    [
      t.cropBefore,
      crop && (
        <div className="before-crop">
          <img src={imageUrl("thumb", item.id, item.mtime, 512)} alt="" draggable={false} />
          <i style={{ left: `${crop[0] * 100}%`, top: `${crop[1] * 100}%`, width: `${crop[2] * 100}%`, height: `${crop[3] * 100}%` }} />
        </div>
      ),
    ],
  ];
  return (
    <aside className={docked ? "info-side" : "info-panel glass"}>
      <Histogram request={histogram} crop={crop} />
      {rows
        .filter(([, v]) => v)
        .map(([k, v]) => (
          <div key={k}>
            <small>{k}</small>
            <span>{v}</span>
          </div>
        ))}
    </aside>
  );
}

/** Frames one Live Photo holds at most; the backend refuses more. */
const MAX_LIVE_FRAMES = 600;

export function Viewer() {
  const t = useT();
  const viewer = useStore((s) => s.viewer)!;
  const items = useStore((s) => s.items);
  const metas = useStore((s) => s.metas);
  const ratings = useStore((s) => s.ratings);
  const flags = useStore((s) => s.flags);
  const looks = useStore((s) => s.looks);
  const library = useStore((s) => s.library);
  const crops = useStore((s) => s.crops);
  const settings = useStore((s) => s.settings);
  const info = useStore((s) => s.info);
  const stacks = useStore((s) => s.stacks);
  const stackOf = useStore((s) => s.stackOf);
  const mobile = !!info?.mobile;
  const byId = useMemo(() => new Map(items.map((it) => [it.id, it])), [items]);

  const [showAf, setShowAf] = useState(true);
  const [guides, setGuides] = useState(0);
  const [flip, setFlip] = useState(0);
  const [cropRatio, setCropRatio] = useState<string | null>(null);
  const [crop, setCrop] = useState<Crop | null>(null);
  const [eye, setEye] = useState(false);
  const [compare, setCompare] = useState(false);
  const [link, setLink] = useState(true);
  const [active, setActive] = useState<0 | 1>(0);
  const [rightIndex, setRightIndex] = useState(0);
  const [floatingInfo, setFloatingInfo] = useState(false);
  const [immersive, setImmersive] = useState(false);
  const [wall, setWall] = useState(false);
  const [zoom, setZoom] = useState({ percent: 0, fit: true });
  const [status, setStatus] = useState<PaneStatus>(null);
  const [live, setLive] = useState<LiveDraft | null>(null);
  const liveRef = useRef(live);
  liveRef.current = live;
  const peeking = useRef(false);
  const [liveStep, setLiveStep] = useState(0.1);
  const [liveReady, setLiveReady] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [saving, setSaving] = useState(false);
  // On desktop the info sidebar stays open across photos and sessions.
  const showInfo = mobile ? floatingInfo : settings.infoPinned;
  const toggleInfo = () => (mobile ? setFloatingInfo((v) => !v) : useStore.getState().setSettings({ infoPinned: !settings.infoPinned }));
  const warm = useRef<HTMLImageElement[]>([]);
  const [insets, setInsets] = useState({ top: 0, bottom: 0 });

  const panes = [useRef<PaneHandle>(null), useRef<PaneHandle>(null)];
  const topBar = useRef<HTMLElement>(null);
  const dock = useRef<HTMLDivElement>(null);
  const strip = useRef<HTMLDivElement>(null);

  const leftItem = byId.get(viewer.ids[viewer.index]);
  const rightItem = compare ? byId.get(viewer.ids[rightIndex]) : undefined;
  const current = active === 1 && rightItem ? rightItem : leftItem;
  const currentMeta = current ? metas[current.id] ?? null : null;

  // Metadata for what is on screen, and warm the neighbours.
  useEffect(() => {
    const st = useStore.getState();
    if (leftItem) st.loadMeta(leftItem.id);
    if (rightItem) st.loadMeta(rightItem.id);
    warm.current = [];
    for (const d of [1, -1, 2]) {
      const next = byId.get(viewer.ids[viewer.index + d]);
      if (!next) continue;
      const img = new Image();
      img.src = imageUrl("screen", next.id, next.mtime, 2048);
      warm.current.push(img);
      // Decode the next photo at full size ahead of time so stepping forward is instant.
      st.loadMeta(next.id).then((meta) => {
        const now = useStore.getState();
        if (d !== 1 || now.viewer?.ids[now.viewer.index] !== leftItem?.id) return;
        const full = new Image();
        full.decoding = "async";
        full.src = fullUrlOf(next, meta, { mobile, autoDevelop: settings.autoDevelop, nikon: !!now.info?.nikon_sdk, look: lookOf(now.looks, next) });
        full.decode?.().catch(() => {});
        warm.current.push(full);
      });
    }
  }, [leftItem?.id, rightItem?.id]);

  // The frames of the stack the photo on screen is in, as far as the viewer holds them.
  const inViewer = useMemo(() => new Set(viewer.ids), [viewer.ids]);
  const stackAt = current && settings.stacks ? stackOf[current.id] : undefined;
  const stack = stackAt ? stacks[stackAt[0]] : undefined;
  const stackFrames = useMemo(() => {
    const frames = (stack?.ids ?? []).filter((id) => inViewer.has(id)).map((id) => byId.get(id)!).filter(Boolean);
    return frames.length > 1 ? frames : [];
  }, [stack, inViewer, byId]);
  const showStrip = stackFrames.length > 0 && !crop && !immersive;

  useLayoutEffect(() => {
    const measure = () => {
      const band = strip.current ? strip.current.offsetHeight + 10 : 0;
      setInsets(immersive ? { top: 0, bottom: 0 } : { top: topBar.current?.offsetHeight ?? 0, bottom: (dock.current?.offsetHeight ?? 0) + 20 + band });
    };
    measure();
    const ro = new ResizeObserver(measure);
    if (topBar.current) ro.observe(topBar.current);
    if (dock.current) ro.observe(dock.current);
    return () => ro.disconnect();
  }, [immersive, compare, showInfo, showStrip, !!live]);

  // Eye zoom follows the photo: stepping through a burst keeps the eyes centred.
  const focusEyes = useCallback(async (item: Item, pane: PaneHandle | null, announce: boolean) => {
    const st = useStore.getState();
    const meta = await st.loadMeta(item.id);
    if (!meta || !pane) return;
    const boxes = meta.af?.boxes ?? [];
    const target = await api.locateEyes(item.id, boxes).catch(() => null);
    const focused = boxes.find((b) => b.focused) ?? boxes[0];
    // Let the pane take its size from the freshly loaded metadata first.
    requestAnimationFrame(() => {
      if (target) pane.zoomTo(target.x, target.y);
      else if (focused) {
        pane.zoomTo(focused.x + focused.w / 2, focused.y + focused.h / 2);
        if (announce) st.notify("noFace");
      } else if (announce) st.notify("noFaceNoAf");
    });
  }, []);

  useEffect(() => {
    if (eye && leftItem) focusEyes(leftItem, panes[0].current, true);
  }, [eye, leftItem?.id]);
  useEffect(() => {
    if (eye && rightItem) focusEyes(rightItem, panes[1].current, false);
  }, [eye, rightItem?.id]);

  // An unconfirmed crop belongs to the photo it was started on.
  useEffect(() => {
    setCrop(null);
    setCropRatio(null);
  }, [current?.id]);

  const toggleEye = () => {
    if (eye) panes.forEach((p) => p.current?.fit());
    setEye(!eye);
  };

  const step = useCallback(
    (delta: number) => {
      if (compare && active === 1) setRightIndex((i) => Math.min(viewer.ids.length - 1, Math.max(0, i + delta)));
      else useStore.getState().setViewerIndex(viewer.index + delta);
    },
    [compare, active, viewer.index, viewer.ids.length],
  );

  /** Shows another photo in the pane that has the keyboard. */
  const show = useCallback(
    (index: number) => {
      if (index < 0 || index >= viewer.ids.length) return;
      if (compare && active === 1) setRightIndex(index);
      else useStore.getState().setViewerIndex(index);
    },
    [compare, active, viewer.ids.length],
  );

  /** Leaves the stack on screen for the one after or before it; a photo on its own is a stack of one. */
  const stepStack = (delta: number) => {
    const ids = viewer.ids;
    const key = (i: number) => (settings.stacks ? stackOf[ids[i]]?.[0] : undefined) ?? `one:${i}`;
    const from = compare && active === 1 ? rightIndex : viewer.index;
    let i = from + delta;
    while (i >= 0 && i < ids.length && key(i) === key(from)) i += delta;
    // Going back lands on the first frame of the stack before.
    while (delta < 0 && i > 0 && key(i - 1) === key(i)) i--;
    show(i);
  };

  // Making a Live Photo from the burst on screen. A file that is a second copy
  // of an exposure is not a frame of it. Indices are into `liveSource`.
  const copies = stack?.copies;
  const liveSource = useMemo(() => stackFrames.filter((f) => !copies?.[f.id]), [stackFrames, copies]);
  const liveFrames = live ? liveSource.slice(live.start, live.end + 1) : [];
  const startLive = () => {
    if (!current) return;
    const origin = copies?.[current.id] ?? current.id;
    const at = Math.max(0, liveSource.findIndex((f) => f.id === origin));
    if (origin !== current.id) show(viewer.ids.indexOf(origin));
    // A very long burst starts as the stretch around the frame on screen.
    const start = Math.max(0, Math.min(at - MAX_LIVE_FRAMES / 2, liveSource.length - MAX_LIVE_FRAMES));
    setLive({ start, end: Math.min(liveSource.length, start + MAX_LIVE_FRAMES) - 1, cover: at });
    setLiveReady(0);
    setPlaying(false);
    setCompare(false);
    api
      .livePhotoPace(liveSource.map((f) => f.id))
      .then(setLiveStep)
      .catch(() => {});
  };
  const endLive = () => {
    setLive(null);
    setPlaying(false);
  };
  const changeLive = (next: LiveDraft) => {
    if (next.end - next.start >= MAX_LIVE_FRAMES) return;
    liveRef.current = next;
    setLive(next);
    if (!peeking.current && next.cover !== live?.cover) show(viewer.ids.indexOf(liveSource[next.cover].id));
  };
  /** While an end of the range is dragged the photo shows that end; afterwards the still again. */
  const peek = (index: number | null) => {
    peeking.current = index !== null;
    const frame = liveSource[index ?? liveRef.current?.cover ?? 0];
    if (frame) show(viewer.ids.indexOf(frame.id));
  };
  const saveLive = async () => {
    if (!live || saving) return;
    setSaving(true);
    setPlaying(false);
    try {
      const saved = await api.saveLivePhoto(liveFrames.map((f) => f.id), live.cover - live.start);
      endLive();
      useStore.getState().notify(`livePhotoSaved:${fileName(saved)}`);
      useStore.getState().rescan();
    } catch (e) {
      useStore.getState().notify(`livePhotoFailed:${e}`);
    } finally {
      setSaving(false);
    }
  };
  // Stepping to another frame of the burst makes it the still; leaving the burst ends the editing.
  const currentId = current?.id;
  useEffect(() => {
    const draft = liveRef.current;
    if (!draft || peeking.current || !currentId) return;
    const at = liveSource.findIndex((f) => f.id === (copies?.[currentId] ?? currentId));
    if (at < 0) return endLive();
    if (at !== draft.cover) setLive({ start: Math.min(draft.start, at), end: Math.max(draft.end, at), cover: at });
  }, [currentId, liveSource, copies]);

  const toggleCompare = () => {
    if (!compare) setRightIndex(Math.min(viewer.ids.length - 1, viewer.index + 1));
    setActive(0);
    setCompare(!compare);
  };

  /** Starts (or, with `null`, abandons) a crop on the current photo. Nothing
   * changes until it is confirmed. Editing always shows the whole frame. */
  const chooseCrop = (id: string | null) => {
    setCropRatio(id);
    const meta = current ? metas[current.id] : null;
    if (!id || !meta || !current) return setCrop(null);
    const swap = meta.orientation >= 5;
    const [w, h] = swap ? [meta.preview_height, meta.preview_width] : [meta.preview_width, meta.preview_height];
    const saved = cropOf(crops, current);
    const keep = id === "free" || (saved && !cropRatio);
    setCrop(saved && keep ? { x: saved[0], y: saved[1], w: saved[2], h: saved[3] } : centeredCrop(w || 3, h || 2, CROP_RATIOS.find((r) => r.id === id)!.ratio));
  };

  const confirmCrop = () => {
    if (!current || !crop) return;
    const whole = crop.w > 0.999 && crop.h > 0.999;
    useStore.getState().setCrop([current.id], whole ? null : [crop.x, crop.y, crop.w, crop.h]);
    chooseCrop(null);
  };

  const resetCrop = () => {
    if (current) useStore.getState().setCrop([current.id], null);
    chooseCrop(null);
  };

  /** Steps through: as shot, original (RAW only), then every imported look. */
  const cycleLook = (delta: number) => {
    if (!current) return;
    const options: (string | null)[] = [null, ...(current.paths.some(isRawPath) ? [NO_LOOK] : []), ...library.map((l) => l.id)];
    const at = Math.max(0, options.indexOf(lookOf(looks, current)));
    useStore.getState().setLook([current.id], options[(at + delta + options.length) % options.length]);
  };

  const cycleGuides = (delta: number) => setGuides((g) => (g + delta + GUIDE_MODES) % GUIDE_MODES);
  const rotateGuides = () => setFlip((f) => (f + 1) % 4);

  /** After marking a photo, optionally move on (not while comparing). */
  const advance = () => {
    if (settings.autoAdvance && !compare) step(1);
  };

  const setFlag = (value: number) => {
    if (!current) return;
    const clear = flagOf(flags, current) === value;
    useStore.getState().flag([current.id], clear ? 0 : value);
    if (!clear) advance();
  };

  const setRating = (n: number) => {
    if (!current) return;
    useStore.getState().rate([current.id], n);
    if (n > 0) advance();
  };

  /** Sets the look of the current photo; it is remembered and used on export. */
  const chooseLook = (look: string | null) => {
    if (!current) return;
    if (look === NO_LOOK && !current.paths.some(isRawPath)) return useStore.getState().notify("originalNeedsRaw");
    useStore.getState().setLook([current.id], look);
  };

  const toggleOriginal = () => chooseLook(current && lookOf(looks, current) === NO_LOOK ? null : NO_LOOK);

  const importLooks = async () => {
    const picked = await open({ multiple: true, filters: [{ name: "Looks", extensions: ["cube", "CUBE", "3dl", "3DL", "png", "PNG", "np3", "NP3"] }] });
    const sources = Array.isArray(picked) ? picked : picked ? [picked] : [];
    if (!sources.length) return;
    try {
      const before = library.length;
      const next = await api.importLooks(sources);
      useStore.getState().setLibrary(next);
      useStore.getState().notify(`lookImported:${Math.max(next.length - before, sources.length)}`);
    } catch (e) {
      useStore.getState().notify(String(e));
    }
  };

  /** Saves the recipe the current photo was shot with to the look library. */
  const saveCameraLook = async () => {
    if (!current) return;
    try {
      const id = await api.saveCameraLook(current.paths.find(isRawPath) ?? current.id);
      const next = await api.listLooks();
      useStore.getState().setLibrary(next);
      useStore.getState().notify(`lookSaved:${next.find((l) => l.id === id)?.name ?? id}`);
    } catch (e) {
      useStore.getState().notify(String(e));
    }
  };

  const toggleClipping = () => useStore.getState().setSettings({ clipping: !settings.clipping });

  const toggleAf = () => {
    if (!showAf && !currentMeta?.af) useStore.getState().notify("noFocusData");
    setShowAf(!showAf);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const st = useStore.getState();
      if (st.sheet || isTyping(e) || !current) return;
      if (wall) {
        // The wall takes the keyboard: its own key or Escape closes it.
        if (e.key === "Escape" || actionFor(st.settings.keys, e) === "wall") {
          e.preventDefault();
          setWall(false);
        }
        return;
      }
      if (e.key === "Enter" && crop) {
        e.preventDefault();
        return confirmCrop();
      }
      if (e.key === "Escape") {
        if (crop) chooseCrop(null);
        else if (live) endLive();
        else if (immersive) setImmersive(false);
        else if (compare) setCompare(false);
        else st.closeViewer();
        return;
      }
      const action = actionFor(st.settings.keys, e);
      if (!action) {
        // Arrow keys always step, unless they were assigned to something.
        if (e.metaKey || e.ctrlKey || e.altKey) return;
        if (e.key === "ArrowRight") return step(1);
        if (e.key === "ArrowLeft") return step(-1);
        if (e.key === "?") return st.setSheet("help");
        return;
      }
      e.preventDefault();
      const stars = ratingOf(action);
      if (stars !== null) return setRating(stars);
      switch (action) {
        case "next":
          return step(1);
        case "prev":
          return step(-1);
        case "stackNext":
          return stepStack(1);
        case "stackPrev":
          return stepStack(-1);
        case "pick":
          return setFlag(1);
        case "reject":
          return setFlag(-1);
        case "unflag":
          return st.flag([current.id], 0);
        case "eye":
          return toggleEye();
        case "zoom":
          return panes[active].current?.toggleActual();
        case "af":
          return toggleAf();
        case "original":
          return toggleOriginal();
        case "lookNext":
          return cycleLook(1);
        case "lookPrev":
          return cycleLook(-1);
        case "wall":
          return setWall(true);
        case "clipping":
          return toggleClipping();
        case "guides":
          return cycleGuides(1);
        case "guidesPrev":
          return cycleGuides(-1);
        case "flip":
          return rotateGuides();
        case "crop":
          return chooseCrop(cropRatio ? null : current && cropOf(crops, current) ? "free" : "original");
        case "compare":
          return toggleCompare();
        case "side":
          if (compare) setActive((a) => (a === 0 ? 1 : 0));
          return;
        case "link":
          return setLink((v) => !v);
        case "info":
          return toggleInfo();
        case "immersive":
          return setImmersive((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  if (!leftItem || !current) return null;

  const relay = (from: 0 | 1) => (n: Nudge) => {
    if (compare && link) panes[from === 0 ? 1 : 0].current?.nudge(n);
  };
  const wheelZoom = settings.wheelZoom ?? info?.platform !== "macos";
  const sdk = !!info?.nikon_sdk;
  const paneProps = {
    insets,
    mobile,
    wheelZoom,
    // Zooming out of a photo that fits goes back to the grid, as zooming into the grid came here.
    onZoomOut: crop || live || mobile ? undefined : () => useStore.getState().closeViewer(),
    autoDevelop: settings.autoDevelop,
    nikon: sdk,
    showAf,
    clip: settings.clipping,
    guides,
    flip,
    crop,
    cropRatio: cropRatio ? CROP_RATIOS.find((r) => r.id === cropRatio)!.ratio : null,
    onCrop: setCrop,
  };

  const guideItems: MenuItem[] = [
    ...t.guideNames.map((label, i) => ({ label, checked: guides === i, onSelect: () => setGuides(i) })),
    ...(ROTATABLE_GUIDES.includes(guides) ? (["divider", { label: t.guidesRotate, onSelect: rotateGuides }] as MenuItem[]) : []),
  ];
  const savedCrop = cropOf(crops, current);
  const cropItems: MenuItem[] = [
    { label: t.guideNames[0], checked: !cropRatio, onSelect: () => chooseCrop(null) },
    ...(savedCrop ? [{ label: t.cropReset, onSelect: resetCrop } as MenuItem] : []),
    "divider",
    ...CROP_RATIOS.map((r) => ({
      label: r.id === "original" ? t.cropOriginal : r.id === "free" ? t.cropFree : r.id,
      checked: cropRatio === r.id,
      onSelect: () => chooseCrop(r.id),
    })),
  ];
  const flag = flagOf(flags, current);
  const position = compare && active === 1 ? rightIndex : viewer.index;
  const docked = showInfo && !mobile && !immersive;
  const look = currentMeta?.look;
  const currentLook = lookOf(looks, current);
  const chosenLook = library.find((l) => l.id === currentLook);
  // The histogram follows what is on screen; a look still rendering is not asked for twice.
  const lookFrom = lookSource(current, { mobile, nikon: sdk });
  const histogramOf = !currentLook ? { path: current.id, look: null, edge: 0 } : status ? null : { ...lookFrom, look: currentLook };
  // Looks go by their name alone: who renders them is not part of what they are called.
  const lookName = currentLook === NO_LOOK ? t.original : chosenLook?.name;
  const pill =
    status === "developing"
      ? t.developing
      : status === "original"
        ? t.originalLoading
        : status === "look"
          ? t.lookRendering
          : status === "failed"
            ? t.developFailed
            : (lookName ?? "");
  // A recipe that only lives in this photo can be kept, unless the library already has it by name.
  const saveLook = look?.custom && currentMeta?.look_recordable && !library.some((l) => l.nikon && l.name === look.name) ? saveCameraLook : undefined;
  const lookItems: MenuItem[] = [
    { label: t.lookAsShot, checked: !currentLook, onSelect: () => chooseLook(null) },
    { label: t.original, checked: currentLook === NO_LOOK, onSelect: () => chooseLook(NO_LOOK) },
    ...(library.length ? ["divider" as const] : []),
    ...library.map((l) => ({
      label: l.name,
      checked: currentLook === l.id,
      onSelect: () => chooseLook(l.id),
    })),
    "divider",
    ...(library.length ? [{ label: t.lookWall, icon: "wall", onSelect: () => setWall(true) } as MenuItem] : []),
    { label: t.lookImport, icon: "plus", onSelect: importLooks },
  ];

  return (
    <div className={`viewer ${immersive ? "immersive" : ""} ${compare ? "comparing" : ""} ${docked ? "with-info" : ""}`}>
      <div className="viewer-main">
        <div className="stage">
          <Pane
            ref={panes[0]}
            key="left"
            item={leftItem}
            meta={metas[leftItem.id] ?? null}
            active={active === 0}
            dimmed={flagOf(flags, leftItem) === -1}
            look={lookOf(looks, leftItem)}
            frame={crop ? null : cropOf(crops, leftItem)}
            onSwipeV={cycleLook}
            {...paneProps}
            crop={active === 0 ? crop : null}
            onActivate={() => setActive(0)}
            onNudge={relay(0)}
            onSwipe={step}
            onZoom={(percent, fit) => active === 0 && setZoom({ percent, fit })}
            onStatus={setStatus}
          />
          {rightItem && (
            <Pane
              ref={panes[1]}
              key="right"
              item={rightItem}
              meta={metas[rightItem.id] ?? null}
              active={active === 1}
              dimmed={flagOf(flags, rightItem) === -1}
              look={lookOf(looks, rightItem)}
              frame={crop ? null : cropOf(crops, rightItem)}
              onSwipeV={cycleLook}
              {...paneProps}
              crop={active === 1 ? crop : null}
              onActivate={() => setActive(1)}
              onNudge={relay(1)}
              onSwipe={step}
              onZoom={(percent, fit) => active === 1 && setZoom({ percent, fit })}
            />
          )}
        </div>

        {live && <LivePreview frames={liveFrames} step={liveStep} playing={playing} area={insets} onReady={setLiveReady} />}
        {!live && !compare && !crop && !immersive && leftItem.live && <LiveMovie item={leftItem} area={insets} />}

        {pill && <div className={`status-pill glass ${!status ? "quiet" : ""}`}>{pill}</div>}

        {!mobile && !immersive && (
          <>
            <button className="edge prev" aria-label={t.actions.prev} disabled={position === 0} onClick={() => step(-1)}>
              <Icon name="back" size={22} />
            </button>
            <button className="edge next" aria-label={t.actions.next} disabled={position >= viewer.ids.length - 1} onClick={() => step(1)}>
              <Icon name="forward" size={22} />
            </button>
          </>
        )}

        <div className="dock glass" ref={dock}>
          {showStrip && stack && (
            <div className="stack-strip-holder" ref={strip}>
              {live ? (
                <LiveEditor
                  frames={liveSource}
                  draft={live}
                  step={liveStep}
                  playing={playing}
                  ready={liveReady}
                  saving={saving}
                  onChange={changeLive}
                  onPeek={peek}
                  onPlay={() => setPlaying((v) => !v)}
                  onCancel={endLive}
                  onSave={saveLive}
                />
              ) : (
                <StackStrip
                  kind={stack.kind}
                  frames={stackFrames}
                  current={current.id}
                  onChoose={(id) => show(viewer.ids.indexOf(id))}
                  onLivePhoto={stack.kind === "burst" && liveSource.length >= MIN_LIVE_FRAMES && info?.live_photo && !mobile ? startLive : undefined}
                />
              )}
            </div>
          )}
          {crop && (
            <div className="crop-bar glass">
              <button onClick={() => chooseCrop(null)}>{t.cancel}</button>
              {savedCrop && <button onClick={resetCrop}>{t.cropReset}</button>}
              <button className="primary" onClick={confirmCrop}>
                {t.cropConfirm}
              </button>
            </div>
          )}
          <div className="dock-group">
            <Stars value={ratings[current.id] || 0} onChange={setRating} size={20} />
          </div>
          <div className="dock-group">
            <IconButton icon="check" label={t.pick} active={flag === 1} className="pick" onClick={() => setFlag(1)} />
            <IconButton icon="x" label={t.reject} active={flag === -1} className="reject" onClick={() => setFlag(-1)} />
          </div>
          <div className="dock-group">
            <IconButton icon="focus" label={t.focusPoints} active={showAf && !!currentMeta?.af} onClick={toggleAf} />
            <IconButton icon="eye" label={t.eyeZoom} active={eye} onClick={toggleEye} />
            <Menu label={t.looks} up className={currentLook ? "on" : ""} items={lookItems} button={<Icon name="look" />} />
            <Menu label={t.guides} up className={guides ? "on" : ""} items={guideItems} button={<Icon name="guides" />} />
            <Menu label={t.crop} up className={cropRatio || savedCrop ? "on" : ""} items={cropItems} button={<Icon name="crop" />} />
            <IconButton icon="clip" label={t.clipping} active={settings.clipping} onClick={toggleClipping} />
            <IconButton icon="compare" label={t.compare} active={compare} onClick={toggleCompare} />
            {compare && <IconButton icon="link" label={t.link} active={link} onClick={() => setLink(!link)} />}
          </div>
        </div>
      </div>

      {docked && <InfoPanel item={current} meta={currentMeta} docked crop={savedCrop} histogram={histogramOf} onSaveLook={saveLook} />}
      {showInfo && mobile && !immersive && (
        <InfoPanel item={current} meta={currentMeta} docked={false} crop={savedCrop} histogram={histogramOf} onSaveLook={saveLook} />
      )}
      {wall && (
        <LookWall
          item={current}
          source={lookFrom}
          looks={library}
          current={currentLook}
          onChoose={(id) => {
            chooseLook(id);
            setWall(false);
          }}
          onClose={() => setWall(false)}
        />
      )}

      <header className="viewer-bar" ref={topBar} data-tauri-drag-region>
        <IconButton icon="back" label={t.back} onClick={() => useStore.getState().closeViewer()} className="on-dark" />
        <div className="viewer-title" data-tauri-drag-region>
          <strong>{current.name}</strong>
          <span>
            {position + 1} / {viewer.ids.length}
            {current.raw && <em>{current.paired ? "RAW+JPG" : "RAW"}</em>}
            {current.live && <em>LIVE</em>}
            {look?.custom && <em className="recipe">{look.name}</em>}
            {savedCrop && <em>{t.cropped}</em>}
          </span>
        </div>
        <div className="viewer-bar-end">
          {zoom.percent > 0 && (
            <button className="zoom-pill" onClick={() => panes[active].current?.toggleActual()}>
              {zoom.percent}%
            </button>
          )}
          <IconButton icon="info" label={t.info} active={showInfo} onClick={toggleInfo} className="on-dark" />
        </div>
      </header>
    </div>
  );
}
