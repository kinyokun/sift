import { useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { Icon } from "./Icon";

/** What a control says on hover, in the app's own tooltip (`Tips`): its name and, as key caps, the
 * shortcuts that do the same (bindings such as `Shift+V`). */
export function tipProps(label: string, keys?: string[]): Record<string, string> {
  const bound = keys?.filter(Boolean) ?? [];
  return bound.length ? { "data-tip": label, "data-keys": bound.join("|") } : { "data-tip": label };
}

export function Segmented<T extends string>(props: {
  value: T;
  options: { value: T; label: ReactNode }[];
  onChange: (value: T) => void;
  className?: string;
}) {
  return (
    <div className={`segmented ${props.className ?? ""}`} role="tablist">
      {props.options.map((o) => (
        <button
          key={o.value}
          role="tab"
          aria-selected={o.value === props.value}
          className={o.value === props.value ? "on" : ""}
          onClick={() => props.onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** A number in `text` typed by hand: a comma may be the decimal point, a unit after it is left out. */
export function typedNumber(text: string): number | null {
  const m = /^\s*([-−+]?\s*(?:\d+(?:[.,]\d*)?|[.,]\d+))\s*[^\d.,]*$/.exec(text);
  return m ? Number(m[1].replace(/[−\s]/g, (c) => (c === "−" ? "-" : "")).replace(",", ".")) : null;
}

/** A slider whose value also stands in a field to type it in: Enter or leaving the field sets it, Escape puts it
 * back, ↑ and ↓ step. The field shows the value times `scale` (a share as a percentage), to `digits` decimals,
 * followed by `unit`; a number typed in is held to the slider's range but not to its steps. `onStart` and `onEnd`
 * bracket a drag of the slider and a visit to the field, the changes made in one go. */
export function NumberSlider(props: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (value: number) => void;
  scale?: number;
  digits?: number;
  unit?: string;
  className?: string;
  onStart?: () => void;
  onEnd?: () => void;
  onDoubleClick?: () => void;
}) {
  const scale = props.scale ?? 1;
  const digits = props.digits ?? 0;
  const shown = String(Number((props.value * scale).toFixed(digits)));
  const [text, setText] = useState<string | null>(null);
  const [bad, setBad] = useState(false);
  // Escape leaves the field without setting what was typed in it.
  const dropping = useRef(false);
  const clamp = (v: number) => Math.min(props.max, Math.max(props.min, v));
  const set = (v: number) => {
    const value = clamp(Number(v.toFixed(6)));
    if (value !== props.value) props.onChange(value);
  };
  const typed = text === null ? null : typedNumber(text);
  return (
    <span className={`slider-value ${props.className ?? ""}`}>
      <input
        className="slider"
        type="range"
        min={props.min}
        max={props.max}
        step={props.step}
        value={props.value}
        aria-label={props.label}
        onPointerDown={props.onStart}
        onPointerUp={props.onEnd}
        onDoubleClick={props.onDoubleClick}
        onChange={(e) => props.onChange(Number(e.target.value))}
      />
      <label className={`num-field ${bad ? "bad" : ""}`}>
        <input
          value={text ?? shown}
          inputMode="decimal"
          spellCheck={false}
          autoComplete="off"
          size={Math.max(2, shown.length)}
          aria-label={props.label}
          aria-invalid={bad}
          data-own-escape=""
          onFocus={(e) => {
            e.currentTarget.select();
            props.onStart?.();
          }}
          onChange={(e) => {
            setText(e.target.value);
            setBad(false);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              if (text !== null && typed === null) setBad(true);
              else e.currentTarget.blur();
            } else if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              dropping.current = true;
              e.currentTarget.blur();
            } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
              e.preventDefault();
              const step = props.step * (e.shiftKey ? 10 : 1) * (e.key === "ArrowUp" ? 1 : -1);
              set((typed === null ? props.value : Number(typed.toFixed(digits)) / scale) + step);
              setText(null);
              setBad(false);
            }
          }}
          onBlur={() => {
            if (!dropping.current && typed !== null) set(Number(typed.toFixed(digits)) / scale);
            dropping.current = false;
            setText(null);
            setBad(false);
            props.onEnd?.();
          }}
        />
        {props.unit && <span>{props.unit}</span>}
      </label>
    </span>
  );
}
export function Stars(props: { value: number; onChange?: (value: number) => void; size?: number; tips?: { label: string; key: string }[] }) {
  const size = props.size ?? 18;
  return (
    <div className="stars" role="radiogroup">
      {[1, 2, 3, 4, 5].map((n) => (
        <button
          key={n}
          className={n <= props.value ? "on" : ""}
          aria-label={`${n}`}
          {...(props.tips ? tipProps(props.tips[n - 1].label, [props.tips[n - 1].key]) : {})}
          aria-checked={n === props.value}
          role="radio"
          tabIndex={-1}
          disabled={!props.onChange}
          // Tapping the current rating clears it.
          onClick={() => props.onChange?.(n === props.value ? 0 : n)}
        >
          <Icon name="star" size={size} fill={n <= props.value} />
        </button>
      ))}
    </div>
  );
}

export function Toggle(props: { checked: boolean; onChange: (checked: boolean) => void; label?: string }) {
  return (
    <button
      className={`switch ${props.checked ? "on" : ""}`}
      role="switch"
      aria-checked={props.checked}
      aria-label={props.label}
      onClick={() => props.onChange(!props.checked)}
    >
      <span />
    </button>
  );
}

export function IconButton(props: {
  icon: string;
  label: string;
  onClick?: () => void;
  active?: boolean;
  disabled?: boolean;
  size?: number;
  className?: string;
  /** Bindings of the shortcuts that do the same, shown on hover. */
  keys?: string[];
}) {
  return (
    <button
      className={`icon-button ${props.active ? "on" : ""} ${props.className ?? ""}`}
      {...tipProps(props.label, props.keys)}
      aria-label={props.label}
      aria-pressed={props.active}
      disabled={props.disabled}
      onClick={props.onClick}
    >
      <Icon name={props.icon} size={props.size ?? 20} />
    </button>
  );
}

/** The least room kept between a menu and the edge of the window, in pixels. */
const EDGE = 8;

/** Keeps an open menu inside the window: one that opens upwards is centred on its button whatever its
 * width, and any menu that would cross an edge of the window is moved back inside. */
export function useInsideWindow(menu: RefObject<HTMLElement | null>, open: boolean) {
  useLayoutEffect(() => {
    const el = menu.current;
    if (!open || !el) return;
    el.style.translate = "";
    if (el.classList.contains("up")) el.style.marginLeft = `${-el.offsetWidth / 2}px`;
    const r = el.getBoundingClientRect();
    // The rectangle is still growing with the opening animation; its centre is not.
    const left = (r.left + r.right - el.offsetWidth) / 2;
    const out = Math.max(0, left + el.offsetWidth - (window.innerWidth - EDGE)) || Math.min(0, left - EDGE);
    if (out) el.style.translate = `${-out}px 0`;
  }, [menu, open]);
}

export type MenuItem = { label: string; onSelect: () => void; checked?: boolean; icon?: string; danger?: boolean } | "divider";

/** A button that opens a small popover menu. */
export function Menu(props: { button: ReactNode; label: string; items: MenuItem[]; align?: "left" | "right"; up?: boolean; className?: string; keys?: string[] }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  useInsideWindow(menu, open);
  useEffect(() => {
    if (!open) return;
    const close = (e: Event) => {
      if (e instanceof KeyboardEvent) {
        if (e.key !== "Escape") return;
        // Escape closes only the menu, not the screen underneath, nor full screen.
        e.stopPropagation();
        e.preventDefault();
        setOpen(false);
      } else if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("pointerdown", close, true);
    window.addEventListener("keydown", close, true);
    return () => {
      window.removeEventListener("pointerdown", close, true);
      window.removeEventListener("keydown", close, true);
    };
  }, [open]);
  return (
    <div className={`menu-anchor ${props.className ?? ""}`} ref={ref}>
      <button className={`icon-button ${open ? "on" : ""}`} {...tipProps(props.label, props.keys)} aria-label={props.label} aria-expanded={open} onClick={() => setOpen(!open)}>
        {props.button}
      </button>
      {open && (
        <div ref={menu} className={`menu solid ${props.align === "left" ? "left" : "right"} ${props.up ? "up" : ""}`} role="menu">
          {props.items.map((item, i) =>
            item === "divider" ? (
              <hr key={i} />
            ) : (
              <button
                key={i}
                role="menuitem"
                className={item.danger ? "danger" : undefined}
                onClick={() => {
                  setOpen(false);
                  item.onSelect();
                }}
              >
                <span className="menu-check">{item.checked ? <Icon name="check" size={15} /> : item.icon ? <Icon name={item.icon} size={16} /> : null}</span>
                <span className="menu-label">{item.label}</span>
              </button>
            ),
          )}
        </div>
      )}
    </div>
  );
}

/** Without `onClose` the sheet stays until whatever it shows has finished. `tabs` stay put above what scrolls. */
export function Sheet(props: { title: string; onClose?: () => void; children: ReactNode; footer?: ReactNode; wide?: boolean; tabs?: ReactNode }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // A field that puts back what was typed on Escape gets it first.
      if (e.key === "Escape" && !(e.target as Element | null)?.closest?.("[data-own-escape]")) {
        e.stopPropagation();
        if (!props.onClose) return;
        e.preventDefault();
        props.onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [props.onClose]);
  return (
    <div className="sheet-backdrop" onPointerDown={(e) => e.target === e.currentTarget && props.onClose?.()}>
      <section className={`sheet solid ${props.wide ? "wide" : ""} ${props.tabs ? "tabbed" : ""}`} role="dialog" aria-modal="true" aria-label={props.title}>
        <header>
          <h2>{props.title}</h2>
          {props.onClose && (
            <button className="sheet-close" aria-label="Close" onClick={props.onClose}>
              <Icon name="x" size={15} />
            </button>
          )}
        </header>
        {props.tabs && <div className="sheet-tabs">{props.tabs}</div>}
        <div className="sheet-body">{props.children}</div>
        {props.footer && <footer>{props.footer}</footer>}
      </section>
    </div>
  );
}

export function Row(props: { label: ReactNode; hint?: ReactNode; children?: ReactNode; onClick?: () => void }) {
  return (
    <div className={`row ${props.onClick ? "tappable" : ""}`} onClick={props.onClick}>
      <div className="row-label">
        <span>{props.label}</span>
        {props.hint && <small>{props.hint}</small>}
      </div>
      {props.children && <div className="row-value">{props.children}</div>}
    </div>
  );
}
