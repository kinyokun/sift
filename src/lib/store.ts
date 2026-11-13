import { create } from "zustand";
import { api, fileName, type AppInfo, type CropBox, type Item, type LookInfo, type Meta, type Session, type Stack, type StackKind } from "./api";
import { DEFAULT_KEYS, resolveKeys, type Action } from "./shortcuts";

export type FlagFilter = "all" | "picked" | "unflagged" | "rejected";
export type SortBy = "name" | "time" | "rating" | "size";
export type SheetName = "export" | "findraw" | "transfer" | "settings" | "help";

export type Settings = {
  lang: "auto" | "zh" | "en";
  pair: boolean;
  /** For RAW+JPEG pairs, show the RAW's embedded preview instead of the JPEG. */
  preferRaw: boolean;
  /** Show a burst or a pixel-shift series as one cell in the grid. */
  stacks: boolean;
  autoDevelop: boolean;
  /** Move to the next photo after rating or flagging in the viewer. */
  autoAdvance: boolean;
  /** Keep the info sidebar open in the viewer. */
  infoPinned: boolean;
  /** Mark blown highlights and crushed shadows on the photo in the viewer. */
  clipping: boolean;
  wheelZoom: boolean | null;
  thumb: number;
  /** Width of a tile on the look wall. */
  wallTile: number;
  /** Order of the grid and of the viewer opened from it. */
  sortBy: SortBy;
  sortDesc: boolean;
  keys: Record<Action, string>;
  /** Bumped when the default bindings change, so old saved maps are replaced once. */
  keysVersion: number;
  onboarded: boolean;
  recents: string[];
};

const SETTINGS_KEY = "sift.settings";
const MAX_RECENTS = 6;
const KEYS_VERSION = 5;
/** Progress is written this long after the last change. */
const SAVE_DELAY_MS = 250;

const defaults: Settings = {
  lang: "auto",
  pair: true,
  preferRaw: false,
  stacks: true,
  autoDevelop: true,
  autoAdvance: false,
  infoPinned: true,
  clipping: false,
  wheelZoom: null,
  thumb: 184,
  wallTile: 230,
  sortBy: "name",
  sortDesc: false,
  keys: { ...DEFAULT_KEYS },
  keysVersion: KEYS_VERSION,
  onboarded: false,
  recents: [],
};

function loadSettings(): Settings {
  try {
    const saved = JSON.parse(localStorage.getItem(SETTINGS_KEY) || "{}");
    let keys = { ...DEFAULT_KEYS };
    // Versions 4 and 5 only added actions; bindings saved since version 3 carry over.
    if (saved.keysVersion === KEYS_VERSION || saved.keysVersion === 4 || saved.keysVersion === 3) keys = resolveKeys(saved.keys || {});
    else if (saved.keysVersion === 2) {
      // Version 2 put "original" on the letter O, which reads like the 0 of
      // "clear rating". Move it unless the user had already changed it; every
      // other customised binding is kept.
      const { original, ...rest } = saved.keys || {};
      keys = resolveKeys(original === "O" ? rest : { ...rest, original });
    }
    return { ...defaults, ...saved, keys, keysVersion: KEYS_VERSION };
  } catch {
    return defaults;
  }
}

type Toast = { id: number; text: string };

type State = {
  info: AppInfo | null;
  settings: Settings;
  folder: string | null;
  items: Item[];
  loading: boolean;
  ratings: Record<string, number>;
  /** Capture time of each photo ("YYYY-MM-DD HH:MM:SS"), where the file records one. */
  dates: Record<string, string>;
  /** File name -> 1 picked, -1 rejected. Every file of a shot carries the flag. */
  flags: Record<string, number>;
  metas: Record<string, Meta>;
  /** File name -> look chosen for that photo; saved with the folder's progress. */
  looks: Record<string, string>;
  /** File name -> confirmed crop; saved with the folder's progress and used on export. */
  crops: Record<string, CropBox>;
  /** Looks imported into the app. */
  library: LookInfo[];
  filter: FlagFilter;
  minStars: number;
  /** Bursts and pixel-shift series of the folder, known once every file has been read. */
  stacks: Stack[];
  /** Photo id -> its stack and its place in it. */
  stackOf: Record<string, [number, number]>;
  /** Stacks opened in the grid, by the id of their first photo. */
  expanded: string[];
  /** Grid cells that are selected: a collapsed stack counts as one, by its first frame. */
  selection: string[];
  cursor: string | null;
  /** Ids shown in the viewer, frozen when it opens so marking never reshuffles them.
   * `compare` holds the photos it opened comparing, if it did. */
  viewer: { ids: string[]; index: number; compare?: string[] } | null;
  sheet: SheetName | null;
  toast: Toast | null;

  init: () => Promise<void>;
  setSettings: (patch: Partial<Settings>) => void;
  openFolder: (path: string) => Promise<void>;
  closeFolder: () => void;
  reload: () => Promise<void>;
  /** Picks up files that appeared in the folder, leaving everything on screen as it is. */
  rescan: () => Promise<void>;
  setFilter: (filter: FlagFilter) => void;
  setMinStars: (n: number) => void;
  select: (ids: string[], cursor?: string | null) => void;
  /** Opens a stack in the grid, or folds it up again. */
  toggleStack: (key: string) => void;
  /** Opens the viewer on a photo, or comparing several (in grid order). */
  openViewer: (id: string, compare?: string[]) => void;
  closeViewer: () => void;
  setViewerIndex: (index: number) => void;
  rate: (ids: string[], rating: number) => Promise<void>;
  flag: (ids: string[], value: number) => void;
  /** Assigns a look to photos; `null` goes back to the photo as shot. */
  setLook: (ids: string[], look: string | null) => void;
  setLibrary: (library: LookInfo[]) => void;
  /** Confirms a crop for photos; `null` restores the full frame. */
  setCrop: (ids: string[], crop: CropBox | null) => void;
  loadMeta: (id: string) => Promise<Meta | null>;
  setSheet: (sheet: SheetName | null) => void;
  notify: (text: string) => void;
};

export function folderName(path: string): string {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;
}

/** The confirmed crop of a shot, if any. */
export function cropOf(crops: Record<string, CropBox>, item: Item): CropBox | null {
  for (const p of item.paths) {
    const c = crops[fileName(p)];
    if (c) return c;
  }
  return null;
}

/** The look chosen for a shot, if any. */
export function lookOf(looks: Record<string, string>, item: Item): string | null {
  for (const p of item.paths) {
    const l = looks[fileName(p)];
    if (l) return l;
  }
  return null;
}

/** The flag of a shot: any of its files may carry it. */
export function flagOf(flags: Record<string, number>, item: Item): number {
  for (const p of item.paths) {
    const f = flags[fileName(p)];
    if (f) return f;
  }
  return 0;
}

const two = (n: number) => String(n).padStart(2, "0");

/** When a photo was taken, as sortable text; the file's own date stands in when it records none. */
export function takenAt(dates: Record<string, string>, item: Item): string {
  const known = dates[item.id];
  if (known) return known;
  const d = new Date(item.mtime);
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())} ${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
}

/** One cell of the grid: a photo, or a folded stack of them. */
export type Entry = {
  /** What selection and the cursor go by: the first frame shown. */
  id: string;
  /** The frame that stands for the stack. */
  item: Item;
  /** Every frame the cell covers, in shooting order. */
  members: Item[];
  /** Set when the photo belongs to a stack with more than one frame on show.
   * `index` is its place among them (from 1) once the stack is opened. */
  stack: { key: string; kind: StackKind; index: number; total: number; open: boolean } | null;
};

type Shown = Pick<State, "items" | "filter" | "minStars" | "flags" | "ratings" | "dates" | "stacks" | "stackOf" | "expanded"> & {
  settings: Pick<Settings, "sortBy" | "sortDesc" | "stacks">;
};

/** The frame a folded stack shows: a pick, else the best rated, else the first that is not rejected. */
function coverOf(members: Item[], s: Pick<State, "flags" | "ratings">): Item {
  const picked = members.find((m) => flagOf(s.flags, m) === 1);
  if (picked) return picked;
  let best: Item | null = null;
  for (const m of members) if ((s.ratings[m.id] || 0) > (best ? s.ratings[best.id] || 0 : 0)) best = m;
  return best ?? members.find((m) => flagOf(s.flags, m) !== -1) ?? members[0];
}

/** The cells of the grid: the photos that pass the filters, stacks folded, in the chosen order. */
export function visibleEntries(s: Shown): Entry[] {
  const shown = s.items.filter((it) => {
    const flag = flagOf(s.flags, it);
    if (s.filter === "picked" && flag !== 1) return false;
    if (s.filter === "rejected" && flag !== -1) return false;
    if (s.filter === "unflagged" && flag !== 0) return false;
    return (s.ratings[it.id] || 0) >= s.minStars;
  });
  const single = (it: Item): Entry => ({ id: it.id, item: it, members: [it], stack: null });
  let entries: Entry[];
  if (!s.settings.stacks || !s.stacks.length) entries = shown.map(single);
  else {
    // A stack is as large as the number of its frames that pass the filters.
    const frames = new Map<number, Item[]>();
    for (const it of shown) {
      const at = s.stackOf[it.id];
      if (!at) continue;
      const list = frames.get(at[0]);
      if (list) list.push(it);
      else frames.set(at[0], [it]);
    }
    const open = new Set(s.expanded);
    const placed = new Set<number>();
    entries = [];
    for (const it of shown) {
      const at = s.stackOf[it.id];
      const members = at && frames.get(at[0]);
      if (!at || !members || members.length < 2) {
        entries.push(single(it));
        continue;
      }
      // The whole stack goes where the first of its frames is.
      if (placed.has(at[0])) continue;
      placed.add(at[0]);
      members.sort((x, y) => s.stackOf[x.id][1] - s.stackOf[y.id][1]);
      const { kind, ids } = s.stacks[at[0]];
      const stack = { key: ids[0], kind, total: members.length };
      if (open.has(stack.key)) members.forEach((m, i) => entries.push({ ...single(m), stack: { ...stack, index: i + 1, open: true } }));
      else entries.push({ id: members[0].id, item: coverOf(members, s), members, stack: { ...stack, index: 0, open: false } });
    }
  }
  const { sortBy, sortDesc } = s.settings;
  if (sortBy === "name") return sortDesc ? entries.reverse() : entries;
  const key: (it: Item) => string | number =
    sortBy === "time" ? (it) => takenAt(s.dates, it) : sortBy === "rating" ? (it) => s.ratings[it.id] || 0 : (it) => it.size;
  const sign = sortDesc ? -1 : 1;
  // Array.sort is stable, so equal photos stay in name order either way.
  return entries
    .map((e) => ({ e, k: key(e.item) }))
    .sort((a, b) => (a.k < b.k ? -sign : a.k > b.k ? sign : 0))
    .map((x) => x.e);
}

/** Every photo that passes the filters, in grid order; the frames of a stack stay together. Scans arrive sorted by name. */
export function visibleItems(s: Shown): Item[] {
  return visibleEntries(s).flatMap((e) => e.members);
}

/** The photos behind grid cells: a folded stack stands for all its frames. */
export function framesOf(entries: Entry[], ids: string[]): Item[] {
  const by = new Map(entries.map((e) => [e.id, e]));
  return ids.flatMap((id) => by.get(id)?.members ?? []);
}

let toastSeq = 0;
let saveTimer: ReturnType<typeof setTimeout> | undefined;
const pendingMeta = new Map<string, Promise<Meta | null>>();

export const useStore = create<State>((set, get) => {
  /** The photo being looked at, as a file name that survives re-scans. */
  const position = (): string | null => {
    const { viewer, cursor, items } = get();
    const id = viewer ? viewer.ids[viewer.index] : cursor;
    return items.find((it) => it.id === id)?.name ?? null;
  };

  const saveNow = () => {
    clearTimeout(saveTimer);
    const { folder, flags, looks, crops, viewer, filter, minStars, loading } = get();
    if (!folder || loading) return;
    const session: Session = { flags, looks, crops, cursor: position(), viewer: !!viewer, filter, minStars };
    api.saveSession(folder, session).catch(() => {});
  };

  /** Saves progress shortly after a change; a crash loses at most this window. */
  const save = () => {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(saveNow, SAVE_DELAY_MS);
  };

  const readRatings = async (path: string, items: Item[]) => {
    // Ratings and capture times live in the files; read them in the background in batches.
    const BATCH = 200;
    for (let i = 0; i < items.length; i += BATCH) {
      const part = items.slice(i, i + BATCH);
      const values = await api.readBriefs(part.map((it) => it.id));
      if (get().folder !== path) return;
      const ratings = { ...get().ratings };
      const dates = { ...get().dates };
      part.forEach((it, k) => {
        if (values[k]?.rating) ratings[it.id] = values[k].rating;
        if (values[k]?.date) dates[it.id] = values[k].date;
      });
      set({ ratings, dates });
    }
    // Every file has been read by now, so the stacks cost no second pass.
    // A Live Photo made from a burst is a photo of its own, not one more frame of it.
    const stacks = await api.groupShots(items.filter((it) => !it.live).map((it) => it.id)).catch(() => []);
    if (get().folder !== path) return;
    const stackOf: Record<string, [number, number]> = {};
    stacks.forEach((st, k) => st.ids.forEach((id, i) => (stackOf[id] = [k, i])));
    set({ stacks, stackOf });
    settle();
  };

  /** The grid cell a photo is in. */
  const cellOf = (id: string): string => visibleEntries(get()).find((e) => e.members.some((m) => m.id === id))?.id ?? id;

  /** After stacks fold or open, selection and cursor move to the cells their photos are in now. */
  const settle = () => {
    const { selection, cursor, viewer } = get();
    if (viewer || (!selection.length && !cursor)) return;
    const cells = new Map<string, string>();
    for (const e of visibleEntries(get())) for (const m of e.members) cells.set(m.id, e.id);
    set({ selection: [...new Set(selection.map((id) => cells.get(id) ?? id))], cursor: cursor ? cells.get(cursor) ?? cursor : null });
  };

  return {
    info: null,
    settings: loadSettings(),
    folder: null,
    items: [],
    loading: false,
    ratings: {},
    dates: {},
    flags: {},
    metas: {},
    looks: {},
    crops: {},
    library: [],
    filter: "all",
    minStars: 0,
    stacks: [],
    stackOf: {},
    expanded: [],
    selection: [],
    cursor: null,
    viewer: null,
    sheet: null,
    toast: null,

    init: async () => {
      const info = await api.appInfo();
      set({ info });
      document.documentElement.dataset.platform = info.platform;
      document.documentElement.dataset.mobile = String(info.mobile);
      api.listLooks().then((library) => set({ library })).catch(() => {});
      window.addEventListener("pagehide", saveNow);
      document.addEventListener("visibilitychange", () => document.hidden && saveNow());
      if (info.dev_open) {
        await get().openFolder(info.dev_open);
        const item = get().items[info.dev_view ?? -1];
        if (item) get().openViewer(item.id);
        return;
      }
      // Pick up where the last run stopped (also after a crash or power cut).
      const last = await api.lastSession().catch(() => null);
      if (last) {
        await get().openFolder(last);
        if (get().folder === last && get().items.length) get().notify("resumed");
      } else if (!get().settings.onboarded) set({ sheet: "help" });
    },

    setSettings: (patch) => {
      const settings = { ...get().settings, ...patch };
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
      set({ settings });
      if ("stacks" in patch) settle();
    },

    openFolder: async (path) => {
      saveNow();
      set({ loading: true, folder: path, items: [], ratings: {}, dates: {}, flags: {}, looks: {}, crops: {}, metas: {}, stacks: [], stackOf: {}, expanded: [], selection: [], cursor: null, viewer: null });
      try {
        const { pair, preferRaw } = get().settings;
        const [items, saved] = await Promise.all([api.scanFolder(path, pair, preferRaw), api.loadSession(path)]);
        if (get().folder !== path) return;
        const at = items.find((it) => it.paths.some((p) => fileName(p) === saved?.cursor));
        set({
          items,
          flags: saved?.flags ?? {},
          looks: saved?.looks ?? {},
          crops: saved?.crops ?? {},
          loading: false,
          filter: (saved?.filter as FlagFilter) ?? "all",
          minStars: saved?.minStars ?? 0,
          selection: at ? [at.id] : [],
          cursor: at?.id ?? null,
        });
        if (at && saved?.viewer) get().openViewer(at.id);
        // A fresh folder starts on its first photo, so keys work straight away.
        const first = at ? null : visibleItems(get())[0];
        if (first) set({ selection: [first.id], cursor: first.id });
        const recents = [path, ...get().settings.recents.filter((r) => r !== path)].slice(0, MAX_RECENTS);
        get().setSettings({ recents });
        save();
        await readRatings(path, items);
      } catch (e) {
        set({ loading: false, folder: null });
        get().notify(String(e));
      }
    },

    closeFolder: () => {
      saveNow();
      api.clearLastSession().catch(() => {});
      set({ folder: null, items: [], stacks: [], stackOf: {}, expanded: [], viewer: null, selection: [], cursor: null });
    },

    reload: async () => {
      const { folder, settings } = get();
      if (!folder) return;
      const at = position();
      const items = await api.scanFolder(folder, settings.pair, settings.preferRaw);
      if (get().folder !== folder) return;
      const keep = items.find((it) => it.paths.some((p) => fileName(p) === at));
      set({ items, metas: {}, dates: {}, stacks: [], stackOf: {}, viewer: null, selection: keep ? [keep.id] : [], cursor: keep?.id ?? null });
      await readRatings(folder, items);
    },

    rescan: async () => {
      const { folder, settings } = get();
      if (!folder) return;
      const items = await api.scanFolder(folder, settings.pair, settings.preferRaw);
      if (get().folder !== folder) return;
      const known = new Set(get().items.map((it) => it.id));
      const fresh = items.filter((it) => !known.has(it.id));
      set({ items });
      const briefs = await api.readBriefs(fresh.map((it) => it.id)).catch(() => []);
      if (get().folder !== folder) return;
      const ratings = { ...get().ratings };
      const dates = { ...get().dates };
      fresh.forEach((it, k) => {
        if (briefs[k]?.rating) ratings[it.id] = briefs[k].rating;
        if (briefs[k]?.date) dates[it.id] = briefs[k].date!;
      });
      set({ ratings, dates });
    },

    setFilter: (filter) => {
      set({ filter, selection: [], cursor: null });
      save();
    },
    setMinStars: (minStars) => {
      set({ minStars, selection: [], cursor: null });
      save();
    },
    select: (ids, cursor) => {
      set({ selection: ids, cursor: cursor === undefined ? ids[ids.length - 1] ?? null : cursor });
      save();
    },
    toggleStack: (key) => {
      const { expanded } = get();
      set({ expanded: expanded.includes(key) ? expanded.filter((k) => k !== key) : [...expanded, key] });
      settle();
    },

    openViewer: (id, compare) => {
      const ids = visibleItems(get()).map((it) => it.id);
      const shown = (compare ?? []).filter((c) => ids.includes(c)).sort((a, b) => ids.indexOf(a) - ids.indexOf(b));
      const index = ids.indexOf(shown.length > 1 ? shown[0] : id);
      if (index >= 0) set({ viewer: shown.length > 1 ? { ids, index, compare: shown } : { ids, index } });
      save();
    },
    closeViewer: () => {
      const v = get().viewer;
      if (!v) return;
      // Back in the grid the photo may be inside a folded stack.
      const id = cellOf(v.ids[v.index]);
      // A selection made in the grid survives a look at one of its photos.
      const { selection } = get();
      set({ viewer: null, selection: selection.length > 1 && selection.includes(id) ? selection : [id], cursor: id });
      save();
    },
    setViewerIndex: (index) => {
      const v = get().viewer;
      if (v && index >= 0 && index < v.ids.length) set({ viewer: { ...v, index } });
      save();
    },

    rate: async (ids, rating) => {
      const { items, ratings } = get();
      const previous = { ...ratings };
      const next = { ...ratings };
      ids.forEach((id) => (next[id] = rating));
      set({ ratings: next });
      try {
        let sidecar = false;
        for (const id of ids) {
          const item = items.find((it) => it.id === id);
          if (item && (await api.setRating(item.paths, rating)) === "sidecar") sidecar = true;
        }
        if (sidecar && rating > 0) get().notify("sidecar");
      } catch (e) {
        set({ ratings: previous });
        get().notify(`ratingFailed:${e}`);
      }
    },

    flag: (ids, value) => {
      const { items, flags } = get();
      const next = { ...flags };
      for (const id of ids) {
        const item = items.find((it) => it.id === id);
        for (const p of item?.paths ?? []) {
          if (value === 0) delete next[fileName(p)];
          else next[fileName(p)] = value;
        }
      }
      set({ flags: next });
      save();
    },

    setLook: (ids, look) => {
      const { items, looks } = get();
      const next = { ...looks };
      for (const id of ids) {
        const item = items.find((it) => it.id === id);
        for (const p of item?.paths ?? []) {
          if (look) next[fileName(p)] = look;
          else delete next[fileName(p)];
        }
      }
      set({ looks: next });
      save();
    },
    setLibrary: (library) => set({ library }),

    setCrop: (ids, crop) => {
      const { items, crops } = get();
      const next = { ...crops };
      for (const id of ids) {
        const item = items.find((it) => it.id === id);
        for (const p of item?.paths ?? []) {
          if (crop) next[fileName(p)] = crop;
          else delete next[fileName(p)];
        }
      }
      set({ crops: next });
      save();
    },

    loadMeta: (id) => {
      const cached = get().metas[id];
      if (cached) return Promise.resolve(cached);
      let pending = pendingMeta.get(id);
      if (!pending) {
        pending = api
          .readMeta(id)
          .then(async (meta) => {
            // An edited or re-saved JPEG may have lost its maker notes; the paired RAW still has them.
            const raw = get().items.find((it) => it.id === id)?.paths.find((p) => p !== id);
            if ((!meta.af || !meta.look) && raw) {
              const other = await api.readMeta(raw).catch(() => null);
              meta = { ...meta, af: meta.af ?? other?.af ?? null, look: meta.look ?? other?.look ?? null };
            }
            set({ metas: { ...get().metas, [id]: meta } });
            return meta;
          })
          .catch(() => null)
          .finally(() => pendingMeta.delete(id));
        pendingMeta.set(id, pending);
      }
      return pending;
    },

    setSheet: (sheet) => set({ sheet }),
    notify: (text) => {
      const id = ++toastSeq;
      set({ toast: { id, text } });
      setTimeout(() => get().toast?.id === id && set({ toast: null }), 2600);
    },
  };
});
