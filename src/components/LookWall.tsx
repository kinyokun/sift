import { useEffect, useState } from "react";
import { NO_LOOK, imageUrl, isRawPath, type Item, type LookInfo } from "../lib/api";
import { useT } from "../lib/i18n";
import { Icon } from "../ui/Icon";

/** Long edge of a tile's rendition. */
const TILE_EDGE = 640;
/** Tiles rendered at a time: looks on a RAW take a moment each, and closing
 * the wall should not leave a long queue behind. */
const AT_ONCE = 2;

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
  const tiles: { id: string | null; name: string; url: string }[] = [
    { id: null, name: t.lookAsShot, url: imageUrl("screen", item.id, item.mtime, 1024) },
    ...(item.paths.some(isRawPath) ? [{ id: NO_LOOK, name: t.original, url: imageUrl("look", source.path, item.mtime, TILE_EDGE, NO_LOOK) }] : []),
    ...looks.map((l) => ({ id: l.id, name: l.name, url: imageUrl("look", source.path, item.mtime, TILE_EDGE, l.id) })),
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
        <button className="icon-button on-dark" aria-label={t.cancel} title={t.cancel} onClick={onClose}>
          <Icon name="x" />
        </button>
      </header>
      <div className="look-wall-grid">
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
