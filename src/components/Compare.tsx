import { useEffect, useRef } from "react";
import { imageUrl, type Item } from "../lib/api";
import { useT } from "../lib/i18n";
import { flagOf, useStore } from "../lib/store";
import { Icon } from "../ui/Icon";

/** Photos the viewer can put side by side. */
export const MAX_PANES = 4;
/** Height kept free at the top of each pane in compare for its bar. */
export const SLOT_BAR = 38;

/** Columns and rows for `n` panes: a row of them, a column on a tall screen, 2 × 2 for four. */
export function paneGrid(n: number, width: number, height: number): { cols: number; rows: number } {
  const cols = n === 4 ? 2 : height > width * 1.05 ? 1 : n;
  return { cols, rows: Math.ceil(n / cols) };
}

type BarProps = {
  /** 1-based number of the pane, as marked in the burst strip and the picker. */
  number: number;
  item: Item;
  index: number;
  total: number;
  active: boolean;
  picking: boolean;
  top: number;
  onActivate: () => void;
  onStep: (delta: 1 | -1) => void;
  onPick: () => void;
};

/** One pane's own controls in compare: which photo it shows, and stepping it alone. */
export function SlotBar({ number, item, index, total, active, picking, top, onActivate, onStep, onPick }: BarProps) {
  const t = useT();
  const flag = useStore((s) => flagOf(s.flags, item));
  const stars = useStore((s) => s.ratings[item.id] || 0);
  return (
    <div className={`slot-bar glass ${active ? "active" : ""}`} style={{ top: top + 6 }} onPointerDown={onActivate}>
      <b className="slot-number">{number}</b>
      <button className="slot-name" data-picker-toggle title={t.compareChoose} aria-label={t.compareChoose} aria-expanded={picking} onClick={onPick}>
        <span>{item.name}</span>
        <small>
          {index + 1}/{total}
        </small>
        <Icon name="wall" size={13} />
      </button>
      {flag === 1 && <i className="slot-flag pick" title={t.pick} />}
      {flag === -1 && <i className="slot-flag reject" title={t.reject} />}
      {stars > 0 && <i className="slot-stars">★{stars}</i>}
      <button className="slot-step" aria-label={t.actions.prev} disabled={index === 0} onClick={() => onStep(-1)}>
        <Icon name="back" size={15} />
      </button>
      <button className="slot-step" aria-label={t.actions.next} disabled={index >= total - 1} onClick={() => onStep(1)}>
        <Icon name="forward" size={15} />
      </button>
    </div>
  );
}

type PickerProps = {
  items: Item[];
  current: string;
  /** Photo id -> numbers of the panes showing it. */
  marks: Record<string, number[]>;
  top: number;
  bottom: number;
  onChoose: (id: string) => void;
  onClose: () => void;
};

/** Every photo in the viewer as thumbnails, to put any of them in one pane. */
export function ComparePicker({ items, current, marks, top, bottom, onChoose, onClose }: PickerProps) {
  const t = useT();
  const flags = useStore((s) => s.flags);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    root.current?.querySelector<HTMLElement>(".on")?.scrollIntoView({ block: "center" });
  }, []);

  useEffect(() => {
    const away = (e: PointerEvent) => {
      const target = e.target as Element | null;
      if (root.current?.contains(target) || target?.closest?.("[data-picker-toggle]")) return;
      onClose();
    };
    window.addEventListener("pointerdown", away, true);
    return () => window.removeEventListener("pointerdown", away, true);
  }, [onClose]);

  return (
    <div className="compare-picker glass" ref={root} style={{ top: top + SLOT_BAR + 4, bottom: bottom + 8 }} role="listbox" aria-label={t.compareChoose}>
      {items.map((it) => (
        <button
          key={it.id}
          role="option"
          aria-selected={it.id === current}
          className={`${it.id === current ? "on" : ""} ${flagOf(flags, it) === -1 ? "rejected" : ""}`}
          title={it.name}
          onClick={() => onChoose(it.id)}
        >
          <img src={imageUrl("thumb", it.id, it.mtime, 512)} alt="" loading="lazy" decoding="async" draggable={false} />
          {marks[it.id] && <i>{marks[it.id].join(" ")}</i>}
        </button>
      ))}
    </div>
  );
}
