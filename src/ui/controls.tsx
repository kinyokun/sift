import { useEffect, useRef, useState, type ReactNode } from "react";
import { Icon } from "./Icon";

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

export function Stars(props: { value: number; onChange?: (value: number) => void; size?: number }) {
  const size = props.size ?? 18;
  return (
    <div className="stars" role="radiogroup">
      {[1, 2, 3, 4, 5].map((n) => (
        <button
          key={n}
          className={n <= props.value ? "on" : ""}
          aria-label={`${n}`}
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
}) {
  return (
    <button
      className={`icon-button ${props.active ? "on" : ""} ${props.className ?? ""}`}
      title={props.label}
      aria-label={props.label}
      aria-pressed={props.active}
      disabled={props.disabled}
      onClick={props.onClick}
    >
      <Icon name={props.icon} size={props.size ?? 20} />
    </button>
  );
}

export type MenuItem = { label: string; onSelect: () => void; checked?: boolean; icon?: string } | "divider";

/** A button that opens a small popover menu. */
export function Menu(props: { button: ReactNode; label: string; items: MenuItem[]; align?: "left" | "right"; up?: boolean; className?: string }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: Event) => {
      if (e instanceof KeyboardEvent) {
        if (e.key !== "Escape") return;
        // Escape closes only the menu, not the screen underneath.
        e.stopPropagation();
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
      <button className={`icon-button ${open ? "on" : ""}`} title={props.label} aria-label={props.label} aria-expanded={open} onClick={() => setOpen(!open)}>
        {props.button}
      </button>
      {open && (
        <div className={`menu glass ${props.align === "left" ? "left" : "right"} ${props.up ? "up" : ""}`} role="menu">
          {props.items.map((item, i) =>
            item === "divider" ? (
              <hr key={i} />
            ) : (
              <button
                key={i}
                role="menuitem"
                onClick={() => {
                  setOpen(false);
                  item.onSelect();
                }}
              >
                <span className="menu-check">{item.checked ? <Icon name="check" size={15} /> : item.icon ? <Icon name={item.icon} size={16} /> : null}</span>
                {item.label}
              </button>
            ),
          )}
        </div>
      )}
    </div>
  );
}

export function Sheet(props: { title: string; onClose: () => void; children: ReactNode; footer?: ReactNode; wide?: boolean }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        props.onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [props.onClose]);
  return (
    <div className="sheet-backdrop" onPointerDown={(e) => e.target === e.currentTarget && props.onClose()}>
      <section className={`sheet glass ${props.wide ? "wide" : ""}`} role="dialog" aria-modal="true" aria-label={props.title}>
        <header>
          <h2>{props.title}</h2>
          <button className="sheet-close" aria-label="Close" onClick={props.onClose}>
            <Icon name="x" size={15} />
          </button>
        </header>
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
