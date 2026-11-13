import { useEffect, useRef } from "react";
import { imageUrl, type Item, type StackKind } from "../lib/api";
import { useT } from "../lib/i18n";
import { flagOf, useStore } from "../lib/store";
import { Icon } from "../ui/Icon";

/** Same rendition as the grid, so the thumbnails are already cached. */
const THUMB_EDGE = 512;

type Props = {
  kind: StackKind;
  frames: Item[];
  current: string;
  /** Photo id -> numbers of the compare panes showing it. */
  marks?: Record<string, number[]>;
  onChoose: (id: string) => void;
  /** Present when the frames can be written out as a Live Photo. */
  onLivePhoto?: () => void;
};

/** The frames of the stack the photo on screen belongs to, to step through or jump between. */
export function StackStrip({ kind, frames, current, marks, onChoose, onLivePhoto }: Props) {
  const t = useT();
  const flags = useStore((s) => s.flags);
  const ratings = useStore((s) => s.ratings);
  const list = useRef<HTMLDivElement>(null);
  const at = frames.findIndex((f) => f.id === current);

  useEffect(() => {
    list.current?.querySelector<HTMLElement>(".on")?.scrollIntoView({ block: "nearest", inline: "center" });
  }, [current, frames.length]);

  return (
    <div className="stack-strip glass" role="group" aria-label={kind === "pixelshift" ? t.pixelShift : t.burst}>
      <span className="stack-strip-label">
        <Icon name={kind === "pixelshift" ? "shift" : "stack"} size={14} />
        {kind === "pixelshift" ? t.pixelShift : t.burst}
        <b>
          {at + 1}/{frames.length}
        </b>
      </span>
      <div className="stack-strip-frames" ref={list}>
        {frames.map((f) => {
          const flag = flagOf(flags, f);
          return (
            <button key={f.id} className={`${f.id === current ? "on" : ""} ${flag === -1 ? "rejected" : ""}`} title={f.name} aria-label={f.name} aria-current={f.id === current} onClick={() => onChoose(f.id)}>
              <img src={imageUrl("thumb", f.id, f.mtime, THUMB_EDGE)} alt="" loading="lazy" decoding="async" draggable={false} />
              {/\.nefx$/i.test(f.id) && <i className="merged">{t.merged}</i>}
              {flag === 1 && <i className="pick" />}
              {flag === -1 && <i className="reject" />}
              {(ratings[f.id] || 0) > 0 && <i className="stars">{ratings[f.id]}</i>}
              {marks?.[f.id] && <i className="slot-mark">{marks[f.id].join(" ")}</i>}
            </button>
          );
        })}
      </div>
      {onLivePhoto && (
        <button className="stack-strip-action" title={t.livePhotoHint} onClick={onLivePhoto}>
          <Icon name="live" size={15} />
          {t.livePhoto}
        </button>
      )}
    </div>
  );
}
