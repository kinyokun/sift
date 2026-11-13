import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { NO_LOOK, imageUrl, isRawPath, type Item, type LookInfo } from "../lib/api";
import { useT } from "../lib/i18n";
import { useStore } from "../lib/store";
import { useWheelSize } from "../lib/wheelSize";
import { Icon } from "../ui/Icon";

/** Long edge of a tile's rendition. */
const TILE_EDGE = 640;
/** Tiles rendered at a time: looks on a RAW take a moment each, and closing
 * the wall should not leave a long queue behind. */
const AT_ONCE = 2;
/** Width of a tile, which the slider and Ctrl + wheel change. */
const TILE_MIN = 160;
/** Tiles wider than this many device pixels are rendered at twice the size. */
const TILE_SHARP = 800;
/** The grid's gap and side padding (see `.look-wall-grid`). */
const GAP = 14;
const SIDES = 44;

type Props = {
  item: Item;
  /** The file looks are rendered from (see `lookSource`). */
  source: { path: string };
  looks: LookInfo[];
  /** Look of the photo now: a library look, `NO_LOOK`, or null for as shot. */
  current: string | null;
  onChoose: (look: string | null) => void;
  onClose: () => void;
};

/** One photo in every look of the library, side by side; a click picks one. */
export function LookWall({ item, source, looks, current, onChoose, onClose }: Props) {
  const t = useT();
  const tileSize = useStore((s) => s.settings.wallTile);
  const setTile = (wallTile: number) => useStore.getState().setSettings({ wallTile });
  const grid = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = grid.current;
    if (!el) return;
    const measure = () => setWidth(el.clientWidth);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  // The largest tiles are two to a row; zooming on from there picks the one under the pointer.
  const tileMax = Math.max(TILE_MIN, Math.floor((width - SIDES - GAP) / 2));
  const shown = Math.min(tileSize, tileMax);
  const edge = shown * devicePixelRatio > TILE_SHARP ? TILE_EDGE * 2 : TILE_EDGE;
  useWheelSize(grid, {
    get: () => useStore.getState().settings.wallTile,
    set: setTile,
    min: TILE_MIN,
    max: tileMax,
    past: (at) => {
      const hit = document.elementFromPoint(at.clientX, at.clientY)?.closest<HTMLElement>(".look-tile");
      if (hit) hit.click();
    },
  });
  const tiles: { id: string | null; name: string; url: string }[] = [
    { id: null, name: t.lookAsShot, url: imageUrl("screen", item.id, item.mtime, 1024) },
    ...(item.paths.some(isRawPath) ? [{ id: NO_LOOK, name: t.original, url: imageUrl("look", source.path, item.mtime, edge, NO_LOOK) }] : []),
    ...looks.map((l) => ({ id: l.id, name: l.name, url: imageUrl("look", source.path, item.mtime, edge, l.id) })),
  ];
  // Tiles are asked for in order, a few at a time; `settled` counts those that arrived or failed.
  const [settled, setSettled] = useState<Record<string, true>>({});
  useEffect(() => setSettled({}), [item.id]);
  const done = tiles.filter((tile) => settled[tile.url]).length;
  const settle = (url: string) => setSettled((s) => (s[url] ? s : { ...s, [url]: true }));

  return (
    <div className="look-wall" onClick={onClose}>
      <header>
        <strong>{t.lookWallTitle}</strong>
        <span>{t.lookWallHint}</span>
        <input
          className="size-slider wall-size"
          type="range"
          min={TILE_MIN}
          max={tileMax}
          step={2}
          value={shown}
          title={`${t.size} · ${t.pinchOrCtrl}`}
          aria-label={t.size}
          onClick={(e) => e.stopPropagation()}
          onChange={(e) => setTile(Number(e.target.value))}
        />
        <button className="icon-button on-dark" aria-label={t.cancel} title={t.cancel} onClick={onClose}>
          <Icon name="x" />
        </button>
      </header>
      <div className="look-wall-grid" ref={grid} style={{ "--tile": `${shown}px` } as CSSProperties}>
        {tiles.map((tile, i) => (
          <button
            key={tile.id ?? "as-shot"}
            className={`look-tile ${tile.id === current ? "on" : ""}`}
            onClick={(e) => {
              e.stopPropagation();
              onChoose(tile.id);
            }}
          >
            <span className={`look-tile-image ${settled[tile.url] ? "" : "loading"}`}>
              {i < done + AT_ONCE && <img src={tile.url} alt="" draggable={false} decoding="async" onLoad={() => settle(tile.url)} onError={() => settle(tile.url)} />}
            </span>
            <span className="look-tile-name">{tile.name}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
