import { useEffect, useRef, useState } from "react";
import { imageUrl, type Item } from "../lib/api";
import { useT } from "../lib/i18n";
import { Icon } from "../ui/Icon";

/** Rendition the preview plays; the same one the grid shows at its largest, so it is often cached. */
const PLAY_EDGE = 1024;
/** Rendition of the slivers in the timeline. */
const SLIVER_EDGE = 512;
/** A Live Photo needs at least this many frames. */
export const MIN_LIVE_FRAMES = 3;

export type LiveDraft = { start: number; end: number; cover: number };

type Area = { top: number; bottom: number };

/** Plays the frames of a draft Live Photo over the photo, at the pace the movie will have. */
export function LivePreview({ frames, step, playing, area, onReady }: { frames: Item[]; step: number; playing: boolean; area: Area; onReady: (loaded: number) => void }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const cache = useRef(new Map<string, HTMLImageElement>());
  const key = frames.map((f) => f.id).join("\n");

  // Decode every frame ahead of time so playback never waits.
  useEffect(() => {
    let live = true;
    let loaded = 0;
    const report = () => live && onReady(loaded);
    for (const f of frames) {
      const known = cache.current.get(f.id);
      if (known?.complete) {
        loaded++;
        continue;
      }
      const img = new Image();
      img.decoding = "async";
      img.onload = img.onerror = () => {
        loaded++;
        report();
      };
      img.src = imageUrl("thumb", f.id, f.mtime, PLAY_EDGE);
      cache.current.set(f.id, img);
    }
    report();
    return () => {
      live = false;
    };
  }, [key]);

  useEffect(() => {
    const el = canvas.current;
    if (!playing || !el) return;
    const ratio = window.devicePixelRatio || 1;
    const began = performance.now();
    let raf = 0;
    let shown = -1;
    const draw = (now: number) => {
      raf = requestAnimationFrame(draw);
      if (!frames.length) return;
      // The first callback can carry a time from just before `began`.
      const at = Math.floor(Math.max(0, now - began) / 1000 / step) % frames.length;
      const img = cache.current.get(frames[at].id);
      const [w, h] = [el.clientWidth, el.clientHeight];
      if (at === shown && el.width === Math.round(w * ratio)) return;
      if (!img?.naturalWidth) return;
      shown = at;
      el.width = Math.round(w * ratio);
      el.height = Math.round(h * ratio);
      const ctx = el.getContext("2d")!;
      const scale = Math.min(el.width / img.naturalWidth, el.height / img.naturalHeight);
      const [dw, dh] = [img.naturalWidth * scale, img.naturalHeight * scale];
      ctx.fillStyle = "#000";
      ctx.fillRect(0, 0, el.width, el.height);
      ctx.imageSmoothingQuality = "high";
      ctx.drawImage(img, (el.width - dw) / 2, (el.height - dh) / 2, dw, dh);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, [playing, key, step]);

  return <canvas ref={canvas} className="live-layer" style={{ top: area.top, height: `calc(100% - ${area.top + area.bottom}px)`, display: playing ? undefined : "none" }} />;
}

type EditorProps = {
  frames: Item[];
  draft: LiveDraft;
  /** Seconds each frame is shown for. */
  step: number;
  playing: boolean;
  /** Frames of the range decoded so far. */
  ready: number;
  saving: boolean;
  onChange: (draft: LiveDraft) => void;
  /** A frame to look at while an end of the range is dragged; `null` when the drag ends. */
  onPeek: (index: number | null) => void;
  onPlay: () => void;
  onCancel: () => void;
  onSave: () => void;
};

/** Timeline of a burst for making a Live Photo: drag the ends to choose the frames, click one to make it the still. */
export function LiveEditor({ frames, draft, step, playing, ready, saving, onChange, onPeek, onPlay, onCancel, onSave }: EditorProps) {
  const t = useT();
  const track = useRef<HTMLDivElement>(null);
  const last = frames.length - 1;
  const count = draft.end - draft.start + 1;
  const loading = ready < count;

  const indexAt = (clientX: number) => {
    const box = track.current!.getBoundingClientRect();
    return Math.max(0, Math.min(last, Math.floor(((clientX - box.left) / box.width) * frames.length)));
  };

  const drag = (end: "start" | "end") => (e: React.PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const target = e.currentTarget as HTMLElement;
    target.setPointerCapture(e.pointerId);
    let now = draft;
    const move = (ev: PointerEvent) => {
      const i = indexAt(ev.clientX);
      const next = end === "start" ? { ...now, start: Math.min(i, now.end - MIN_LIVE_FRAMES + 1) } : { ...now, end: Math.max(i, now.start + MIN_LIVE_FRAMES - 1) };
      next.cover = Math.max(next.start, Math.min(next.end, next.cover));
      if (next.start === now.start && next.end === now.end) return;
      now = next;
      onChange(next);
      onPeek(end === "start" ? next.start : next.end);
    };
    const done = () => {
      target.removeEventListener("pointermove", move);
      target.removeEventListener("pointerup", done);
      target.removeEventListener("pointercancel", done);
      onPeek(null);
    };
    target.addEventListener("pointermove", move);
    target.addEventListener("pointerup", done);
    target.addEventListener("pointercancel", done);
  };

  const pick = (e: React.MouseEvent) => {
    const i = indexAt(e.clientX);
    if (i >= draft.start && i <= draft.end) onChange({ ...draft, cover: i });
  };

  const pct = (n: number) => `${(n / frames.length) * 100}%`;

  return (
    <div className="live-editor glass" role="group" aria-label={t.livePhoto}>
      <button className="live-play" disabled={loading} title={playing ? t.livePause : t.livePlay} aria-label={playing ? t.livePause : t.livePlay} onClick={onPlay}>
        {loading ? <span className="spinner" /> : <Icon name={playing ? "pause" : "play"} size={16} fill />}
      </button>
      <div className="live-track" ref={track} onClick={pick} title={t.liveTrackHint}>
        <div className="live-slivers">
          {frames.map((f, i) => (
            <i key={f.id} className={i < draft.start || i > draft.end ? "out" : ""} style={{ backgroundImage: `url("${imageUrl("thumb", f.id, f.mtime, SLIVER_EDGE)}")` }} />
          ))}
        </div>
        <div className="live-range" style={{ left: pct(draft.start), width: pct(count) }}>
          <span className="live-handle start" role="slider" aria-label={t.liveStart} aria-valuemin={1} aria-valuemax={frames.length} aria-valuenow={draft.start + 1} onPointerDown={drag("start")} onClick={(e) => e.stopPropagation()} />
          <span className="live-handle end" role="slider" aria-label={t.liveEnd} aria-valuemin={1} aria-valuemax={frames.length} aria-valuenow={draft.end + 1} onPointerDown={drag("end")} onClick={(e) => e.stopPropagation()} />
        </div>
        <div className="live-cover" style={{ left: pct(draft.cover + 0.5) }}>
          <b>{t.liveCover}</b>
        </div>
      </div>
      <span className="live-length">{t.liveLength(count, (count * step).toFixed(1))}</span>
      <button className="stack-strip-action" onClick={onCancel} disabled={saving}>
        {t.cancel}
      </button>
      <button className="stack-strip-action primary" onClick={onSave} disabled={saving}>
        {saving && <span className="spinner" />}
        {t.liveSave}
      </button>
    </div>
  );
}

/** The mark on a Live Photo in the viewer; pressing it plays the movie over the still. */
export function LiveMovie({ item, area }: { item: Item; area: Area }) {
  const t = useT();
  const [src, setSrc] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const [failed, setFailed] = useState(false);

  // The movie is small; it is fetched whole and played from memory.
  useEffect(() => {
    setPlaying(false);
    setFailed(false);
    setSrc(null);
    if (!item.live) return;
    let url: string | null = null;
    let live = true;
    fetch(imageUrl("live", item.live, item.mtime))
      .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(String(r.status)))))
      .then((data) => {
        if (!live) return;
        url = URL.createObjectURL(new Blob([data], { type: "video/mp4" }));
        setSrc(url);
      })
      .catch(() => live && setFailed(true));
    return () => {
      live = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, [item.live, item.mtime]);

  if (!item.live) return null;
  return (
    <>
      {playing && src && (
        <video className="live-layer" style={{ top: area.top, height: `calc(100% - ${area.top + area.bottom}px)` }} src={src} autoPlay muted playsInline onEnded={() => setPlaying(false)} onError={() => (setPlaying(false), setFailed(true))} />
      )}
      <button className={`live-mark glass ${playing ? "on" : ""}`} style={{ top: area.top + 10 }} disabled={!src || failed} title={failed ? t.liveCannotPlay : t.livePlay} onClick={() => setPlaying((v) => !v)}>
        <Icon name="live" size={14} />
        {t.liveBadge}
      </button>
    </>
  );
}
