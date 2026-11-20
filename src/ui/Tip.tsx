import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { keyCaps } from "../lib/shortcuts";

/** Hovering this long over a control shows its tooltip. */
const TIP_DELAY_MS = 350;
/** Gap between a control and its tooltip, and the least distance from the window's edge. */
const TIP_GAP = 8;

type Shown = { label: string; keys: string[]; rect: DOMRect };

/** The tooltip of every control marked by `tipProps` with shortcuts: its name and the key caps that do
 * the same. One for the whole window, fixed in place, so no scrolling container cuts it off. */
export function Tips() {
  const [tip, setTip] = useState<Shown | null>(null);
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let timer = 0;
    let target: HTMLElement | null = null;
    const hide = () => {
      clearTimeout(timer);
      target = null;
      setTip(null);
    };
    const over = (e: PointerEvent) => {
      if (e.pointerType === "touch") return;
      const el = (e.target as Element | null)?.closest?.<HTMLElement>("[data-tip]") ?? null;
      if (el === target) return;
      hide();
      target = el;
      if (!el) return;
      timer = window.setTimeout(() => {
        if (el.isConnected) setTip({ label: el.dataset.tip ?? "", keys: (el.dataset.keys ?? "").split("|").filter(Boolean), rect: el.getBoundingClientRect() });
      }, TIP_DELAY_MS);
    };
    const out = (e: PointerEvent) => !e.relatedTarget && hide();
    window.addEventListener("pointerover", over);
    document.addEventListener("pointerout", out);
    // A click, a key or a scroll changes what is under the pointer; the tip comes back on the next control.
    window.addEventListener("pointerdown", hide, true);
    window.addEventListener("keydown", hide, true);
    window.addEventListener("wheel", hide, { capture: true, passive: true });
    window.addEventListener("blur", hide);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("pointerover", over);
      document.removeEventListener("pointerout", out);
      window.removeEventListener("pointerdown", hide, true);
      window.removeEventListener("keydown", hide, true);
      window.removeEventListener("wheel", hide, true);
      window.removeEventListener("blur", hide);
    };
  }, []);

  // Above a control in the lower half of the window, below one in the upper half; centred on it and kept inside.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el || !tip) return;
    const { rect } = tip;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    const left = Math.min(window.innerWidth - w - TIP_GAP, Math.max(TIP_GAP, rect.left + rect.width / 2 - w / 2));
    const top = rect.top > window.innerHeight / 2 ? rect.top - h - TIP_GAP : rect.bottom + TIP_GAP;
    el.style.left = `${left}px`;
    el.style.top = `${Math.max(TIP_GAP, top)}px`;
    el.style.visibility = "visible";
  }, [tip]);

  if (!tip) return null;
  return (
    <div className="tip solid" ref={box} role="tooltip">
      <span>{tip.label}</span>
      {tip.keys.map((binding, i) => (
        <span className="tip-keys" key={i}>
          {i > 0 && <i>/</i>}
          {keyCaps(binding).map((cap, j) => (
            <kbd key={j}>{cap}</kbd>
          ))}
        </span>
      ))}
    </div>
  );
}
