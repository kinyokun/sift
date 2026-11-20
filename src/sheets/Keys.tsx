import { useEffect } from "react";
import { useT } from "../lib/i18n";
import { ACTIONS, APPLE, keyCaps, type Action } from "../lib/shortcuts";
import { useStore, wheelModeOf } from "../lib/store";

export type Group = "nav" | "mark" | "inspect" | "look" | "compare" | "app";

/** The actions by what they are for: the colours of the keyboard map, the sections of the list in Settings. */
export const GROUPS: Record<Group, Action[]> = {
  nav: ["prev", "next", "stackPrev", "stackNext"],
  mark: ["pick", "reject", "unflag", "delete", "rate0", "rate1", "rate2", "rate3", "rate4", "rate5", "tags", "tag1", "tag2", "tag3", "tag4", "tag5", "tag6", "tag7", "tag8", "tag9"],
  inspect: ["eye", "zoom", "af", "clipping", "level", "guides", "guidesPrev", "flip", "crop", "rotateLeft", "rotateRight", "info", "immersive"],
  look: ["original", "lookPrev", "lookNext", "wall"],
  compare: ["compare", "panes", "side", "link"],
  app: ["editor", "refresh", "help"],
};
const groupOf = (a: Action) => (Object.keys(GROUPS) as Group[]).find((g) => GROUPS[g].includes(a)) ?? "app";

/** A key on the drawn keyboard: the key it stands for (as in a binding), what is printed on it, and its width
 * in quarters of a letter key. Keys without `key` are drawn only to give the keyboard its shape. */
type Cap = { key?: string; glyph?: string; w?: number; modifier?: boolean };

const letters = (s: string): Cap[] => [...s].map((key) => ({ key }));
const ROWS: Cap[][] = [
  [...letters("`1234567890-="), { key: "Backspace", glyph: "⌫", w: 8 }],
  [{ key: "Tab", glyph: "Tab", w: 6 }, ...letters("QWERTYUIOP[]"), { key: "\\", w: 6 }],
  [{ glyph: "⇪", w: 7 }, ...letters("ASDFGHJKL;'"), { key: "Enter", glyph: "↩", w: 9 }],
  [{ glyph: "⇧", w: 9, modifier: true }, ...letters("ZXCVBNM,./"), { glyph: "⇧", w: 11, modifier: true }],
  APPLE
    ? [{ glyph: "⌃", w: 5 }, { glyph: "⌥", w: 5, modifier: true }, { glyph: "⌘", w: 5, modifier: true }, { key: "Space", glyph: "Space", w: 23 }, { glyph: "⌘", w: 5, modifier: true }, { glyph: "⌥", w: 5, modifier: true }]
    : [{ glyph: "Ctrl", w: 5, modifier: true }, { glyph: "Win", w: 5 }, { glyph: "Alt", w: 5, modifier: true }, { key: "Space", glyph: "Space", w: 23 }, { glyph: "Alt", w: 5, modifier: true }, { glyph: "Ctrl", w: 5, modifier: true }],
];
/** The arrow keys, drawn at the end of the last row: they step when nothing else is put on them. */
const ARROWS: { key: string; glyph: string; fallback: Action }[] = [
  { key: "ArrowLeft", glyph: "←", fallback: "prev" },
  { key: "ArrowUp", glyph: "↑", fallback: "stackPrev" },
  { key: "ArrowDown", glyph: "↓", fallback: "stackNext" },
  { key: "ArrowRight", glyph: "→", fallback: "next" },
];
const DRAWN = new Set([...ROWS.flat().flatMap((c) => (c.key ? [c.key] : [])), ...ARROWS.map((a) => a.key)]);
const MODIFIER_ORDER = ["", "Shift", "Alt", "Mod", "Ctrl", "Meta"];

type Line = { prefix: string; action: Action };

/** What the modifiers of a binding look like in front of an action's name: ⇧, ⌥⌘, Ctrl+. */
function prefixOf(mods: string[]): string {
  const caps = keyCaps(mods.join("+"));
  return APPLE ? caps.join("") : caps.map((c) => (/\w$/.test(c) ? `${c}+` : c)).join("");
}

/** Every binding, filed under its key, the key alone first. */
function linesByKey(keys: Record<Action, string>): Map<string, (Line & { rank: number })[]> {
  const out = new Map<string, (Line & { rank: number })[]>();
  for (const action of ACTIONS) {
    const binding = keys[action];
    if (!binding) continue;
    const parts = binding.split("+");
    const key = parts.pop()!;
    const rank = parts.length ? Math.max(...parts.map((m) => MODIFIER_ORDER.indexOf(m))) + parts.length * 10 : 0;
    const list = out.get(key) ?? [];
    list.push({ prefix: prefixOf(parts), action, rank });
    list.sort((a, b) => a.rank - b.rank);
    out.set(key, list);
  }
  return out;
}

/** The shortcuts drawn on a keyboard, to be taken in at a glance and without scrolling. */
export function KeysOverlay({ onClose }: { onClose: () => void }) {
  const t = useT();
  const keys = useStore((s) => s.settings.keys);
  const wheel = useStore((s) => wheelModeOf(s.settings, s.info?.platform));
  const doubleClick = useStore((s) => s.settings.doubleClick);
  const lines = linesByKey(keys);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  const capOf = (cap: Cap, i: number) => {
    const on = (cap.key && lines.get(cap.key)) || [];
    const main = on.find((l) => !l.prefix);
    return (
      <div
        key={i}
        className={`cap ${main ? `g-${groupOf(main.action)}` : ""} ${cap.key ? "" : "shape"} ${cap.modifier ? "modifier" : ""}`}
        style={{ gridColumn: `span ${cap.w ?? 4}` }}
        title={on.map((l) => `${l.prefix}${keyCaps(cap.key ?? "").join("")}  ${t.actions[l.action]}`).join("\n") || undefined}
      >
        <b>{cap.glyph ?? keyCaps(cap.key ?? "")[0]}</b>
        {on.map((l) => (
          <span key={l.action} className={`g-${groupOf(l.action)} ${l.prefix ? "chord" : "plain"}`}>
            {l.prefix && <i>{l.prefix}</i>}
            {t.keyShort[l.action]}
          </span>
        ))}
      </div>
    );
  };

  const arrows = ARROWS.map((a) => {
    const on = lines.get(a.key) ?? [];
    const shown = on.length ? on : [{ prefix: "", action: a.fallback }];
    return (
      <div key={a.key} className={`cap arrow ${a.key} g-${groupOf(shown.find((l) => !l.prefix)?.action ?? a.fallback)}`}>
        <b>{a.glyph}</b>
        {shown.map((l) => (
          <span key={l.action} className={`g-${groupOf(l.action)} ${l.prefix ? "chord" : "plain"}`}>
            {l.prefix && <i>{l.prefix}</i>}
            {t.keyShort[l.action]}
          </span>
        ))}
      </div>
    );
  });

  const others = ACTIONS.filter((a) => keys[a] && !DRAWN.has(keys[a].split("+").pop()!));
  const fixed: [string[], string][] = [
    [["↩"], t.keysEnter],
    [["Esc"], t.fixedKeys.esc],
    [["Space"], t.fixedKeys.space],
  ];
  const mouse: [string, string][] = [
    [t.mouseRight, t.fixedKeys.rightHold],
    [t.pinchOrCtrl, t.fixedKeys.pinch],
    [t.mouseWheel, t.wheelModes[wheel]],
    [t.mouseDouble, t.doubleClicks[doubleClick]],
  ];

  return (
    <div className="keys-overlay" onClick={onClose} role="dialog" aria-label={t.keysTitle}>
      <header>
        <h2>{t.keysTitle}</h2>
        <div className="keys-legend">
          {(Object.keys(GROUPS) as Group[]).map((g) => (
            <span key={g} className={`g-${g}`}>
              <i />
              {t.keyGroups[g]}
            </span>
          ))}
          <span className="chord-note">
            <kbd>{keyCaps("Shift")[0]}</kbd>
            <kbd>{keyCaps("Alt")[0]}</kbd>
            <kbd>{keyCaps("Mod")[0]}</kbd>
            {t.keysChord}
          </span>
        </div>
        <span className="keys-hint">{t.keysHint}</span>
      </header>
      <div className="keyboard">
        {ROWS.map((row, r) => (
          <div className="key-row" key={r}>
            {row.map(capOf)}
            {r === ROWS.length - 1 && <div className="arrows">{arrows}</div>}
          </div>
        ))}
      </div>
      <footer>
        <section>
          <h3>{t.keysFixed}</h3>
          {fixed.map(([caps, label]) => (
            <div key={label}>
              <span>
                {caps.map((c) => (
                  <kbd key={c}>{c}</kbd>
                ))}
              </span>
              {label}
            </div>
          ))}
        </section>
        <section>
          <h3>{t.keysMouse}</h3>
          {mouse.map(([what, does]) => (
            <div key={what}>
              <span>{what}</span>
              {does}
            </div>
          ))}
        </section>
        {others.length > 0 && (
          <section>
            <h3>{t.keysOther}</h3>
            {others.map((a) => (
              <div key={a}>
                <span>
                  {keyCaps(keys[a]).map((c, i) => (
                    <kbd key={i}>{c}</kbd>
                  ))}
                </span>
                {t.actions[a]}
              </div>
            ))}
          </section>
        )}
      </footer>
    </div>
  );
}
