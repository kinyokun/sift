/** Every action a key can be assigned to in Settings. */
export const ACTIONS = [
  "prev",
  "next",
  "stackPrev",
  "stackNext",
  "pick",
  "reject",
  "unflag",
  "delete",
  "rate0",
  "rate1",
  "rate2",
  "rate3",
  "rate4",
  "rate5",
  "tags",
  "tag1",
  "tag2",
  "tag3",
  "tag4",
  "tag5",
  "tag6",
  "tag7",
  "tag8",
  "tag9",
  "eye",
  "zoom",
  "af",
  "original",
  "lookPrev",
  "lookNext",
  "wall",
  "clipping",
  "guidesPrev",
  "guides",
  "flip",
  "crop",
  "rotateLeft",
  "rotateRight",
  "editor",
  "compare",
  "panes",
  "side",
  "link",
  "info",
  "immersive",
  "refresh",
  "help",
] as const;
export type Action = (typeof ACTIONS)[number];
export const RATE_ACTIONS: Action[] = ["rate0", "rate1", "rate2", "rate3", "rate4", "rate5"];
/** The first tags of a folder, in the order they were made, each have a key of their own. */
export const TAG_ACTIONS: Action[] = ["tag1", "tag2", "tag3", "tag4", "tag5", "tag6", "tag7", "tag8", "tag9"];

const APPLE = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

/** A binding is modifiers plus one key, such as `Alt+D`, `ArrowLeft` or `Space`.
 * `Mod` is Command on Apple devices and Ctrl elsewhere. An empty string means unassigned. */
export const DEFAULT_KEYS: Record<Action, string> = {
  prev: "A",
  next: "D",
  stackPrev: "ArrowUp",
  stackNext: "ArrowDown",
  pick: "P",
  reject: "X",
  unflag: "U",
  // What moves a file to the trash in Finder and in Explorer.
  delete: APPLE ? "Mod+Backspace" : "Delete",
  rate0: "0",
  rate1: "1",
  rate2: "2",
  rate3: "3",
  rate4: "4",
  rate5: "5",
  tags: "T",
  // The digits alone are the ratings.
  tag1: "Shift+1",
  tag2: "Shift+2",
  tag3: "Shift+3",
  tag4: "Shift+4",
  tag5: "Shift+5",
  tag6: "Shift+6",
  tag7: "Shift+7",
  tag8: "Shift+8",
  tag9: "Shift+9",
  eye: "E",
  zoom: "Z",
  af: "F",
  // Not the letter O: on a key cap it cannot be told apart from the 0 that clears a rating.
  original: "N",
  lookPrev: "Mod+A",
  lookNext: "Mod+D",
  wall: "W",
  clipping: "H",
  guidesPrev: "Alt+A",
  guides: "Alt+D",
  flip: "Alt+R",
  crop: "R",
  // As in Lightroom and Bridge.
  rotateLeft: "Mod+[",
  rotateRight: "Mod+]",
  editor: "Mod+E",
  compare: "C",
  panes: "Shift+C",
  side: "Tab",
  link: "L",
  info: "I",
  immersive: "Space",
  refresh: "Mod+R",
  help: "`",
};

const MODIFIER_KEYS = ["Shift", "Control", "Alt", "Meta"];

/** Saved bindings win; a default that would collide with one of them is left
 * unassigned instead, so no key ever triggers two actions. */
export function resolveKeys(saved: Partial<Record<Action, string>>): Record<Action, string> {
  const keys = {} as Record<Action, string>;
  const used = new Set<string>();
  for (const a of ACTIONS) {
    const k = saved[a];
    if (k !== undefined && (k === "" || !used.has(k))) {
      keys[a] = k;
      if (k) used.add(k);
    }
  }
  for (const a of ACTIONS) {
    if (keys[a] !== undefined) continue;
    keys[a] = used.has(DEFAULT_KEYS[a]) ? "" : DEFAULT_KEYS[a];
    if (keys[a]) used.add(keys[a]);
  }
  return keys;
}

/** The key a keyboard event stands for, independent of layout shifts. */
export function keyOf(e: KeyboardEvent): string {
  if (e.code.startsWith("Key")) return e.code.slice(3);
  if (e.code.startsWith("Digit")) return e.code.slice(5);
  if (e.code.startsWith("Numpad") && /\d$/.test(e.code)) return e.code.slice(-1);
  if (e.key === " ") return "Space";
  // The key left of 1 gives ` or ~ depending on Shift and layout.
  if (e.code === "Backquote") return "`";
  return e.key.length === 1 ? e.key.toUpperCase() : e.key;
}

/** The binding string for a keyboard event, or null for a lone modifier. */
export function comboOf(e: KeyboardEvent): string | null {
  if (MODIFIER_KEYS.includes(e.key)) return null;
  const mods = [
    (APPLE ? e.metaKey : e.ctrlKey) && "Mod",
    (APPLE ? e.ctrlKey : e.metaKey) && (APPLE ? "Ctrl" : "Meta"),
    e.altKey && "Alt",
    // ~ is Shift+` on most layouts; treat both as the same key.
    e.shiftKey && e.code !== "Backquote" && "Shift",
  ].filter(Boolean);
  return [...mods, keyOf(e)].join("+");
}

export function actionFor(keys: Record<Action, string>, e: KeyboardEvent): Action | null {
  const combo = comboOf(e);
  return (combo && ACTIONS.find((a) => keys[a] === combo)) || null;
}

/** The star rating an action sets, or null when it is not a rating action. */
export function ratingOf(action: Action | null): number | null {
  const i = action ? RATE_ACTIONS.indexOf(action) : -1;
  return i >= 0 ? i : null;
}

/** Which of the folder's tags an action toggles (from 0), or null when it is not a tag action. */
export function tagOf(action: Action | null): number | null {
  const i = action ? TAG_ACTIONS.indexOf(action) : -1;
  return i >= 0 ? i : null;
}

const SYMBOLS: Record<string, string> = {
  ArrowLeft: "←",
  ArrowRight: "→",
  ArrowUp: "↑",
  ArrowDown: "↓",
  Shift: "⇧",
  Mod: APPLE ? "⌘" : "Ctrl",
  Alt: APPLE ? "⌥" : "Alt",
  Ctrl: APPLE ? "⌃" : "Ctrl",
  Meta: "Win",
  Enter: "↩",
  Backspace: "⌫",
  Delete: "⌦",
  Escape: "Esc",
  "`": "~",
};

/** Human-readable parts of a binding, one per key cap. */
export function keyCaps(binding: string): string[] {
  return binding ? binding.split("+").map((part) => SYMBOLS[part] ?? part) : [];
}

/** True while the user is typing into a form control. */
export function isTyping(e: KeyboardEvent): boolean {
  const el = e.target as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}
