import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import {
  api,
  shotKey,
  type AppInfo,
  type CropBox,
  type Item,
  type LookInfo,
  type Meta,
  type Session,
  type Stack,
  type StackKind,
  type Tag,
  type UpdateOffer,
  type UpdateProgress,
} from "./api";
import { DEFAULT_KEYS, resolveKeys, type Action } from "./shortcuts";

export type FlagFilter = "all" | "picked" | "unflagged" | "rejected";
export type SortBy = "name" | "time" | "rating" | "size";
export type SheetName = "export" | "findraw" | "transfer" | "settings" | "help" | "update" | "delete";

/** What the delete sheet asks about: photos by id. `forGood`: they would not go into a trash. `why`: the
 * trash did not take them, and this is what the system said. `busy` while they are being deleted. */
export type DeleteRequest = { ids: string[]; forGood: boolean; why?: string; busy?: boolean };

/** Where the updater stands. `offer` is kept for as long as a newer release is known;
 * "failed" without one means the check failed, with one the download or installation. */
export type UpdateState = {
  phase: "idle" | "checking" | "current" | "available" | "downloading" | "installing" | "failed";
  offer: UpdateOffer | null;
  progress: UpdateProgress | null;
  error: string | null;
};

/** The scroll wheel in the viewer zooms, goes to the photo after or before, or moves a
 * zoomed photo the way a trackpad scrolls (a sideways swipe then turns the page). */
export type WheelMode = "zoom" | "turn" | "pan";

/** A Mac is used with a trackpad more often than with a wheel. */
export function wheelModeOf(settings: Settings, platform: string | undefined): WheelMode {
  return settings.wheel ?? (platform === "macos" ? "pan" : "zoom");
}

export type Settings = {
  lang: "auto" | "zh" | "en";
  pair: boolean;
  /** For RAW+JPEG pairs, show the RAW's embedded preview instead of the JPEG. */
  preferRaw: boolean;
  /** Also list the photos in the folders inside the one that is opened. */
  subfolders: boolean;
  /** Show a burst or a pixel-shift series as one cell in the grid. */
  stacks: boolean;
  /** Write the still of a Live Photo as HEIC instead of JPEG. */
  liveHeif: boolean;
  autoDevelop: boolean;
  /** Move to the next photo after rating or flagging in the viewer. */
  autoAdvance: boolean;
  /** Keep the info sidebar open in the viewer. */
  infoPinned: boolean;
  /** Mark blown highlights and crushed shadows on the photo in the viewer. */
  clipping: boolean;
  /** What the scroll wheel does in the viewer; unset, what suits the platform (`wheelModeOf`). */
  wheel: WheelMode | null;
  /** What a double click on the photo does in the viewer. */
  doubleClick: "zoom" | "back";
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
  /** Look for a newer release when the app starts (builds that update themselves). */
  checkUpdates: boolean;
  /** The release the user chose not to be asked about again. */
  skippedUpdate: string | null;
  /** Names of the tags made lately, newest first, offered again in other folders. */
  tagNames: string[];
};

const SETTINGS_KEY = "sift.settings";
const MAX_RECENTS = 6;
const KEYS_VERSION = 8;
/** The colours a tag can have; a tag keeps its place in this list. */
export const TAG_COLORS = ["#ff453a", "#ff9f0a", "#ffd60a", "#30d158", "#64d2ff", "#0a84ff", "#bf5af2", "#ff6eb4"];
const MAX_TAG_NAME = 40;
const MAX_TAG_NAMES = 12;
const NO_TAGS: string[] = [];
/** The folder is listed again this often to see whether photos came or went; a slow
 * listing (a network share) is repeated less often, so it takes a twentieth of the time at most. */
const WATCH_MS = 4000;
/** A file written less than this long ago is looked at again, in case it was still being copied. */
const UNSETTLED_MS = 15000;
/** Progress is written this long after the last change. */
const SAVE_DELAY_MS = 250;
/** The update check waits this long after launch, so reopening the last folder goes first. */
const UPDATE_CHECK_DELAY_MS = 3000;
const NO_UPDATE: UpdateState = { phase: "idle", offer: null, progress: null, error: null };

const defaults: Settings = {
  lang: "auto",
  pair: true,
  preferRaw: false,
  subfolders: false,
  stacks: true,
  liveHeif: false,
  autoDevelop: true,
  autoAdvance: false,
  infoPinned: true,
  clipping: false,
  wheel: null,
  doubleClick: "zoom",
  thumb: 184,
  wallTile: 230,
  sortBy: "name",
  sortDesc: false,
  keys: { ...DEFAULT_KEYS },
  keysVersion: KEYS_VERSION,
  onboarded: false,
  recents: [],
  checkUpdates: true,
  skippedUpdate: null,
  tagNames: [],
};

function loadSettings(): Settings {
  try {
    const saved = JSON.parse(localStorage.getItem(SETTINGS_KEY) || "{}");
    let keys = { ...DEFAULT_KEYS };
    // Versions 4 to 8 only added actions; bindings saved since version 3 carry over.
    if (saved.keysVersion >= 3 && saved.keysVersion <= KEYS_VERSION) keys = resolveKeys(saved.keys || {});
    else if (saved.keysVersion === 2) {
      // Version 2 put "original" on the letter O, which reads like the 0 of
      // "clear rating". Move it unless the user had already changed it; every
      // other customised binding is kept.
      const { original, ...rest } = saved.keys || {};
      keys = resolveKeys(original === "O" ? rest : { ...rest, original });
    }
    // Up to 0.4.2 the wheel either zoomed or did not.
    const { wheelZoom, ...rest } = saved;
    const wheel = typeof wheelZoom === "boolean" ? (wheelZoom ? "zoom" : "pan") : null;
    return { ...defaults, wheel, ...rest, keys, keysVersion: KEYS_VERSION };
  } catch {
    return defaults;
  }
}

type Toast = { id: number; text: string };

type State = {
  info: AppInfo | null;
  settings: Settings;
  folder: string | null;
  /** The photos of the folders inside `folder` are listed too: the setting, or a folder with none of its own. */
  deep: boolean;
  /** Only the photos of this folder inside `folder` are shown ("" is `folder` itself); `null` shows all. */
  subfolder: string | null;
  items: Item[];
  loading: boolean;
  ratings: Record<string, number>;
  /** Capture time of each photo ("YYYY-MM-DD HH:MM:SS"), where the file records one. */
  dates: Record<string, string>;
  /** File name (below a subfolder: its path from the opened folder) -> 1 picked, -1 rejected. Every file of a shot carries the flag. */
  flags: Record<string, number>;
  metas: Record<string, Meta>;
  /** File name -> look chosen for that photo; saved with the folder's progress. */
  looks: Record<string, string>;
  /** File name -> confirmed crop; saved with the folder's progress and used on export. */
  crops: Record<string, CropBox>;
  /** Looks imported into the app. */
  library: LookInfo[];
  /** The tags of this folder, in the order they were made; saved with its progress. */
  tags: Tag[];
  /** File name -> ids of the tags the photo carries. Every file of a shot carries them. */
  tagged: Record<string, string[]>;
  /** Only photos with one of these tags are shown; empty shows all. */
  tagFilter: string[];
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
  /** The photos the delete sheet is asking about. */
  doomed: DeleteRequest | null;
  toast: Toast | null;
  update: UpdateState;

  init: () => Promise<void>;
  setSettings: (patch: Partial<Settings>) => void;
  openFolder: (path: string) => Promise<void>;
  closeFolder: () => void;
  reload: () => Promise<void>;
  /** Lists the folder again: photos that appeared are added, ones that are gone leave the grid and the
   * viewer, and everything else on screen stays as it is. `manual` says what was found even when nothing changed. */
  refresh: (manual?: boolean) => Promise<void>;
  setSubfolder: (subfolder: string | null) => void;
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
  /** Deletes photos, by id. One goes into the system's trash at once; several are asked about first (`ask`
   * says so either way), and so is everything where there is no trash to take it back out of. */
  deletePhotos: (ids: string[], ask?: boolean) => void;
  /** Deletes what the delete sheet asked about. */
  confirmDelete: () => Promise<void>;
  cancelDelete: () => void;
  /** Makes a tag, or finds the one of that name; resolves to its id, `null` for a name with nothing in it. */
  addTag: (name: string) => string | null;
  /** False when the name is empty or another tag has it. */
  renameTag: (id: string, name: string) => boolean;
  /** Gives a tag the next colour. */
  recolorTag: (id: string) => void;
  /** Deletes a tag and takes it off every photo. */
  removeTag: (id: string) => void;
  /** Puts a tag on photos or takes it off; left open, it goes on unless all of them have it. Says which it did. */
  tag: (ids: string[], tagId: string, on?: boolean) => boolean;
  setTagFilter: (tagFilter: string[]) => void;
  /** Assigns a look to photos; `null` goes back to the photo as shot. */
  setLook: (ids: string[], look: string | null) => void;
  setLibrary: (library: LookInfo[]) => void;
  /** Confirms a crop for photos; `null` restores the full frame. */
  setCrop: (ids: string[], crop: CropBox | null) => void;
  loadMeta: (id: string) => Promise<Meta | null>;
  setSheet: (sheet: SheetName | null) => void;
  notify: (text: string) => void;
  /** Saves photos to the system photo library (iOS), Live Photos as Live Photos. */
  saveToPhotos: (items: Item[]) => Promise<void>;
  /** Looks for a newer release. The check at launch asks nothing about a skipped release and
   * keeps quiet when it fails; one the user asked for (`manual`) always shows what it found. */
  checkUpdate: (manual?: boolean) => Promise<void>;
  /** Downloads and installs the release on offer; the app restarts when it is in place. */
  installUpdate: () => Promise<void>;
  cancelUpdate: () => void;
  /** Stops asking about the release on offer. */
  skipUpdate: () => void;
};

export function folderName(path: string): string {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;
}

/** The confirmed crop of a shot, if any. */
export function cropOf(crops: Record<string, CropBox>, item: Item): CropBox | null {
  for (const p of item.paths) {
    const c = crops[shotKey(item.dir, p)];
    if (c) return c;
  }
  return null;
}

/** The look chosen for a shot, if any. */
export function lookOf(looks: Record<string, string>, item: Item): string | null {
  for (const p of item.paths) {
    const l = looks[shotKey(item.dir, p)];
    if (l) return l;
  }
  return null;
}

/** The flag of a shot: any of its files may carry it. */
export function flagOf(flags: Record<string, number>, item: Item): number {
  for (const p of item.paths) {
    const f = flags[shotKey(item.dir, p)];
    if (f) return f;
  }
  return 0;
}

/** The tags a shot carries, by id: any of its files may hold them. */
export function tagsOf(tagged: Record<string, string[]>, item: Item): string[] {
  for (const p of item.paths) {
    const ids = tagged[shotKey(item.dir, p)];
    if (ids?.length) return ids;
  }
  return NO_TAGS;
}

const sameName = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
/** A tag's name as it is kept: one line, no spaces around it, short enough for a file name. */
const tagName = (name: string) => name.replace(/\s+/g, " ").trim().slice(0, MAX_TAG_NAME).trim();

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

type Shown = Pick<State, "items" | "filter" | "minStars" | "flags" | "ratings" | "dates" | "stacks" | "stackOf" | "expanded" | "tagged" | "tagFilter"> & {
  subfolder?: string | null;
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
    if (s.subfolder != null && it.dir !== s.subfolder) return false;
    const flag = flagOf(s.flags, it);
    if (s.filter === "picked" && flag !== 1) return false;
    if (s.filter === "rejected" && flag !== -1) return false;
    if (s.filter === "unflagged" && flag !== 0) return false;
    if (s.tagFilter.length && !tagsOf(s.tagged, it).some((id) => s.tagFilter.includes(id))) return false;
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
/** An update was found while another sheet was open; it is offered when that one closes. */
let offerWaiting = false;
const pendingMeta = new Map<string, Promise<Meta | null>>();
/** The first reading of the folder that is open, while it runs. */
let reading: Promise<void> | null = null;
/** What the listing of the open folder looked like when its photos were last read. `unsettled`: a file
 * was written moments before, so it may still be growing (a copy in progress) and is looked at again. */
let seen: { folder: string; deep: boolean; stamp: string; unsettled: boolean } | null = null;
let nextWatch = 0;
let watching = false;
/** Refreshes run one after the other: the second finds what the first has already picked up. */
let refreshing: Promise<void> = Promise.resolve();
/** A file changed this recently may not be whole yet. A share's clock can run ahead, hence both ways. */
const unsettled = (items: Item[]) => items.some((it) => Math.abs(Date.now() - it.mtime) < UNSETTLED_MS);

export const useStore = create<State>((set, get) => {
  /** The photo being looked at, by the name its flags are kept under, which survives re-scans. */
  const position = (): string | null => {
    const { viewer, cursor, items } = get();
    const id = viewer ? viewer.ids[viewer.index] : cursor;
    const item = items.find((it) => it.id === id);
    return item ? shotKey(item.dir, item.id) : null;
  };

  /** Lists `folder` the way the settings say. A folder with no photos of its own shows the ones in the folders inside it. */
  const scanFolder = async (folder: string): Promise<{ items: Item[]; deep: boolean; found: boolean }> => {
    const { pair, preferRaw, subfolders } = get().settings;
    // On phones the folders inside the library are the albums, each opened on its own.
    const tries = get().info?.mobile ? [false] : subfolders ? [true] : [false, true];
    let items: Item[] = [];
    for (const deep of tries) {
      items = await api.scanFolder(folder, pair, preferRaw, deep);
      // A folder can only be asked about once it has been listed. What changes in between is a file
      // that is new this moment, which `unsettled` has the folder read again for.
      const stamp = await api.folderChanges(folder, deep).catch(() => "");
      seen = { folder, deep, stamp, unsettled: unsettled(items) };
      if (items.length) return { items, deep, found: deep && !subfolders };
    }
    // An empty folder is watched all the way down, wherever its first photos turn up.
    return { items, deep: tries[tries.length - 1], found: false };
  };

  /** Looks whether photos came or went, and picks the changes up. */
  const watch = async () => {
    const { folder, deep, loading } = get();
    if (!folder || loading || watching || document.hidden || Date.now() < nextWatch) return;
    watching = true;
    try {
      const began = Date.now();
      const stamp = await api.folderChanges(folder, deep);
      nextWatch = Date.now() + Math.max(WATCH_MS, (Date.now() - began) * 20);
      const known = seen && seen.folder === folder && seen.deep === deep ? seen.stamp : null;
      if (get().folder === folder && (stamp !== known || seen?.unsettled)) await get().refresh();
    } catch {
      nextWatch = Date.now() + WATCH_MS;
    } finally {
      watching = false;
    }
  };

  const saveNow = () => {
    clearTimeout(saveTimer);
    const { folder, flags, looks, crops, viewer, filter, minStars, loading, subfolder, tags, tagged, tagFilter } = get();
    if (!folder || loading) return;
    const session: Session = { flags, looks, crops, cursor: position(), viewer: !!viewer, filter, minStars, subfolder, tags, tagged, tagFilter };
    api.saveSession(folder, session).catch(() => {});
  };

  /** Saves progress shortly after a change; a crash loses at most this window. */
  const save = () => {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(saveNow, SAVE_DELAY_MS);
  };

  /** Keeps a tag's name to offer it in the folders opened later, newest first. */
  const remember = (name: string) => {
    const { settings, setSettings } = get();
    setSettings({ tagNames: [name, ...settings.tagNames.filter((n) => !sameName(n, name))].slice(0, MAX_TAG_NAMES) });
  };

  /** Reads ratings and capture times from the files, in batches. `again` is for photos read before: stars that
   * are no longer there are taken away, and a file that cannot be read is passed over. False when the folder
   * was left meanwhile. */
  const readBriefs = async (path: string, items: Item[], again = false): Promise<boolean> => {
    const BATCH = 200;
    for (let i = 0; i < items.length; i += BATCH) {
      const part = items.slice(i, i + BATCH);
      const before = get().ratings;
      const ids = part.map((it) => it.id);
      const values = await (again ? api.readBriefs(ids).catch(() => []) : api.readBriefs(ids));
      if (get().folder !== path) return false;
      const ratings = { ...get().ratings };
      const dates = { ...get().dates };
      part.forEach((it, k) => {
        const value = values[k];
        if (!value) return;
        // Stars given while the files were being read are the newer ones.
        if (ratings[it.id] === before[it.id]) {
          if (value.rating) ratings[it.id] = value.rating;
          else if (again) delete ratings[it.id];
        }
        if (value.date) dates[it.id] = value.date;
      });
      set({ ratings, dates });
    }
    return true;
  };

  const readRatings = async (path: string, items: Item[]) => {
    // Ratings and capture times live in the files; they are read in the background.
    // Every file has been read by the end, so the stacks cost no second pass.
    if (await readBriefs(path, items)) await group(path, items);
  };

  /** Finds the bursts and pixel-shift series among `items`. */
  const group = async (path: string, items: Item[]) => {
    // A Live Photo made from a burst is a photo of its own, not one more frame of it.
    const stacks = await api.groupShots(items.filter((it) => !it.live).map((it) => it.id)).catch(() => []);
    if (get().folder !== path) return;
    const stackOf: Record<string, [number, number]> = {};
    stacks.forEach((st, k) => st.ids.forEach((id, i) => (stackOf[id] = [k, i])));
    set({ stacks, stackOf });
    settle();
  };

  /** Reads ratings, capture times and stacks of a folder that has just been listed. */
  const read = (path: string, items: Item[]) => {
    const run = readRatings(path, items).finally(() => reading === run && (reading = null));
    reading = run;
    return run;
  };

  /** Takes over a new list of the folder's photos: what is known about the ones that are gone or were written
   * again is dropped, and cursor, selection and viewer move to the nearest photo that is still there. `lost`
   * says the photo on screen in the viewer was among the ones that went. */
  const adopt = (items: Item[]) => {
    const old = get().items;
    const before = new Map(old.map((it) => [it.id, it]));
    const now = new Set(items.map((it) => it.id));
    const added = items.filter((it) => !before.has(it.id));
    const removed = old.filter((it) => !now.has(it.id));
    // Saved again by another program, or its RAW or movie came or went.
    const changed = items.filter((it) => {
      const was = before.get(it.id);
      return was && (was.mtime !== it.mtime || was.size !== it.size || was.paths.length !== it.paths.length || (was.live ?? null) !== (it.live ?? null));
    });
    if (!added.length && !removed.length && !changed.length) return { added, removed, changed, lost: false };
    const stale = new Set([...removed, ...changed].map((it) => it.id));
    const fresh = <T,>(known: Record<string, T>) => Object.fromEntries(Object.entries(known).filter(([id]) => !stale.has(id)));
    // A photo that is gone hands its place to the next one that is still there, else the one before.
    const heir = (id: string, order: string[]) => {
      const i = order.indexOf(id);
      return now.has(id) ? id : (order.slice(i + 1).find((x) => now.has(x)) ?? order.slice(0, Math.max(i, 0)).reverse().find((x) => now.has(x)) ?? null);
    };
    const { viewer, selection, cursor, ratings, dates, metas } = get();
    const order = visibleEntries(get()).map((e) => e.id);
    const at = cursor && heir(cursor, order);
    const kept = selection.filter((id) => now.has(id));
    let shown = viewer;
    if (viewer) {
      const current = heir(viewer.ids[viewer.index], viewer.ids);
      const ids = viewer.ids.filter((id) => now.has(id));
      const compare = viewer.compare?.filter((id) => now.has(id));
      shown = current ? { ids, index: ids.indexOf(current), ...(compare && compare.length > 1 ? { compare } : {}) } : null;
    }
    const subfolder = get().subfolder;
    const narrowed = subfolder != null && items.some((it) => it.dir === subfolder) ? subfolder : null;
    set({ items, subfolder: narrowed, ratings: fresh(ratings), dates: fresh(dates), metas: fresh(metas), viewer: shown, selection: kept.length ? kept : at ? [at] : [], cursor: at });
    return { added, removed, changed, lost: !!viewer && !now.has(viewer.ids[viewer.index]) };
  };

  /** Lists the open folder again and takes over what is different. */
  const relist = async (manual: boolean) => {
    const folder = get().folder;
    if (!folder || get().loading) return;
    // The first reading of the folder ends with stacks made from its own list of photos.
    await reading;
    if (get().folder !== folder) return;
    const { settings, deep } = get();
    let items: Item[];
    let stamp: string;
    try {
      stamp = await api.folderChanges(folder, deep);
      items = await api.scanFolder(folder, settings.pair, settings.preferRaw, deep);
      if (get().folder !== folder) return;
    } catch (e) {
      // A share that dropped off for a moment says nothing about the photos.
      if (manual) get().notify(String(e));
      return;
    }
    const { added, removed, changed, lost } = adopt(items);
    const reread = [...added, ...changed];
    seen = { folder, deep, stamp, unsettled: unsettled(reread) };
    const same = !reread.length && !removed.length;
    // Photos arriving from a card come in over many looks at the folder; only a refresh that was asked for reports.
    if (manual) get().notify(same ? "refreshedSame" : `refreshed:${added.length},${removed.length}`);
    else if (lost) get().notify("photoGone");
    if (reread.length && !(await readBriefs(folder, reread, true))) return;
    if (!same) await group(folder, get().items);
    // A refresh that was asked for also reads the stars of the photos that look the same: another program may
    // have given some since, and Nikon's keeps them in a file of its own, which the listing says nothing about.
    // Nothing waits for it.
    const known = new Set(reread.map((it) => it.id));
    if (manual) void readBriefs(folder, items.filter((it) => !known.has(it.id)), true);
  };

  /** Deletes photos and takes them out of what is shown. What the trash does not take is asked about again. */
  const erase = async (ids: string[], forGood: boolean) => {
    const { folder, items } = get();
    const wanted = new Set(ids);
    const shots = items.filter((it) => wanted.has(it.id));
    if (!folder || !shots.length) return;
    // A Live Photo's movie goes with its still.
    const files = shots.map((it) => (it.live ? [...it.paths, it.live] : it.paths));
    const done = await api.deletePhotos(files, forGood).catch((e) => ({ failed: shots.map((_, i) => i), reason: String(e) }));
    if (get().folder !== folder) return;
    const stuck = new Set(done.failed.map((i) => shots[i]?.id));
    const gone = new Set(shots.filter((it) => !stuck.has(it.id)).map((it) => it.id));
    if (gone.size) {
      // In turn with the refreshes: one that listed the folder before the photos went would bring them back.
      const run = refreshing.then(() => void (get().folder === folder && adopt(get().items.filter((it) => !gone.has(it.id)))));
      refreshing = run.catch(() => {});
      await run;
      if (get().folder !== folder) return;
      void group(folder, get().items);
    }
    if (!stuck.size) get().notify(`deleted:${gone.size},${forGood ? 1 : 0}`);
    else if (forGood) get().notify(`deleteFailed:${stuck.size},${done.reason ?? ""}`);
    else {
      // A network share has no trash, for one. Finder asks the same question then.
      set({ doomed: { ids: [...stuck] as string[], forGood: true, why: done.reason ?? "" } });
      get().setSheet("delete");
    }
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
    deep: false,
    subfolder: null,
    items: [],
    loading: false,
    ratings: {},
    dates: {},
    flags: {},
    metas: {},
    looks: {},
    crops: {},
    library: [],
    tags: [],
    tagged: {},
    tagFilter: [],
    filter: "all",
    minStars: 0,
    stacks: [],
    stackOf: {},
    expanded: [],
    selection: [],
    cursor: null,
    viewer: null,
    sheet: null,
    doomed: null,
    toast: null,
    update: NO_UPDATE,

    init: async () => {
      const info = await api.appInfo();
      set({ info });
      if (info.updater && get().settings.checkUpdates) setTimeout(() => get().checkUpdate(), UPDATE_CHECK_DELAY_MS);
      document.documentElement.dataset.platform = info.platform;
      document.documentElement.dataset.mobile = String(info.mobile);
      api.listLooks().then((library) => set({ library })).catch(() => {});
      window.addEventListener("pagehide", saveNow);
      document.addEventListener("visibilitychange", () => document.hidden && saveNow());
      // Coming back to the window is when photos have most likely been copied in or deleted.
      const look = () => ((nextWatch = 0), void watch());
      window.addEventListener("focus", look);
      document.addEventListener("visibilitychange", () => !document.hidden && look());
      setInterval(watch, 1000);
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
      set({ loading: true, folder: path, deep: false, subfolder: null, items: [], ratings: {}, dates: {}, flags: {}, looks: {}, crops: {}, tags: [], tagged: {}, tagFilter: [], metas: {}, stacks: [], stackOf: {}, expanded: [], selection: [], cursor: null, viewer: null });
      try {
        const [{ items, deep, found }, saved] = await Promise.all([scanFolder(path), api.loadSession(path)]);
        if (get().folder !== path) return;
        const at = items.find((it) => it.paths.some((p) => shotKey(it.dir, p) === saved?.cursor));
        const subfolder = saved?.subfolder != null && items.some((it) => it.dir === saved.subfolder) ? saved.subfolder : null;
        const tags = saved?.tags ?? [];
        set({
          items,
          deep,
          subfolder,
          flags: saved?.flags ?? {},
          looks: saved?.looks ?? {},
          crops: saved?.crops ?? {},
          tags,
          tagged: saved?.tagged ?? {},
          tagFilter: (saved?.tagFilter ?? []).filter((id) => tags.some((tag) => tag.id === id)),
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
        if (found) get().notify(`subfoldersShown:${items.length}`);
        await read(path, items);
      } catch (e) {
        set({ loading: false, folder: null });
        get().notify(String(e));
      }
    },

    closeFolder: () => {
      saveNow();
      api.clearLastSession().catch(() => {});
      set({ folder: null, deep: false, subfolder: null, items: [], stacks: [], stackOf: {}, expanded: [], viewer: null, selection: [], cursor: null });
    },

    reload: async () => {
      const { folder } = get();
      if (!folder) return;
      const at = position();
      const { items, deep } = await scanFolder(folder);
      if (get().folder !== folder) return;
      const keep = items.find((it) => it.paths.some((p) => shotKey(it.dir, p) === at));
      const subfolder = deep ? get().subfolder : null;
      set({ items, deep, subfolder, metas: {}, dates: {}, stacks: [], stackOf: {}, viewer: null, selection: keep ? [keep.id] : [], cursor: keep?.id ?? null });
      await read(folder, items);
    },

    refresh: (manual = false) => {
      const run = refreshing.then(() => relist(manual));
      refreshing = run.catch(() => {});
      return run;
    },

    setSubfolder: (subfolder) => {
      set({ subfolder, selection: [], cursor: null });
      const first = visibleEntries(get())[0];
      if (first) set({ selection: [first.id], cursor: first.id });
      save();
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
          if (value === 0) delete next[shotKey(item!.dir, p)];
          else next[shotKey(item!.dir, p)] = value;
        }
      }
      set({ flags: next });
      save();
    },

    deletePhotos: (ids, ask) => {
      const { info, items, doomed } = get();
      if (doomed?.busy) return;
      const known = new Set(items.map((it) => it.id));
      ids = ids.filter((id) => known.has(id));
      if (!ids.length) return get().notify("selectFirst");
      const forGood = !info?.trash;
      if (!forGood && !(ask ?? ids.length > 1)) return void erase(ids, false);
      set({ doomed: { ids, forGood } });
      get().setSheet("delete");
    },

    confirmDelete: async () => {
      const asked = get().doomed;
      if (!asked || asked.busy) return;
      set({ doomed: { ...asked, busy: true } });
      await erase(asked.ids, asked.forGood);
      // Unless it has asked the next question by now.
      if (get().doomed?.busy) {
        set({ doomed: null });
        if (get().sheet === "delete") get().setSheet(null);
      }
    },

    cancelDelete: () => {
      if (get().doomed?.busy) return;
      set({ doomed: null });
      if (get().sheet === "delete") get().setSheet(null);
    },

    addTag: (name) => {
      const clean = tagName(name);
      if (!clean) return null;
      const { tags } = get();
      remember(clean);
      const known = tags.find((tag) => sameName(tag.name, clean));
      if (known) return known.id;
      const number = tags.reduce((max, tag) => Math.max(max, Number(tag.id.slice(1)) || 0), 0) + 1;
      // The first colour no tag has yet; with all of them taken, round again.
      const free = TAG_COLORS.findIndex((_, i) => !tags.some((tag) => tag.color === i));
      const tag: Tag = { id: `t${number}`, name: clean, color: free >= 0 ? free : tags.length % TAG_COLORS.length };
      set({ tags: [...tags, tag] });
      save();
      return tag.id;
    },

    renameTag: (id, name) => {
      const clean = tagName(name);
      const { tags } = get();
      if (!clean || tags.some((tag) => tag.id !== id && sameName(tag.name, clean))) return false;
      set({ tags: tags.map((tag) => (tag.id === id ? { ...tag, name: clean } : tag)) });
      remember(clean);
      save();
      return true;
    },

    recolorTag: (id) => {
      set({ tags: get().tags.map((tag) => (tag.id === id ? { ...tag, color: (tag.color + 1) % TAG_COLORS.length } : tag)) });
      save();
    },

    removeTag: (id) => {
      const { tags, tagged, tagFilter } = get();
      const next: Record<string, string[]> = {};
      for (const [key, ids] of Object.entries(tagged)) {
        const kept = ids.filter((t) => t !== id);
        if (kept.length) next[key] = kept;
      }
      set({ tags: tags.filter((tag) => tag.id !== id), tagged: next, tagFilter: tagFilter.filter((t) => t !== id) });
      save();
    },

    tag: (ids, tagId, on) => {
      const { items, tagged, tags } = get();
      const wanted = new Set(ids);
      const targets = items.filter((it) => wanted.has(it.id));
      if (!targets.length || !tags.some((tag) => tag.id === tagId)) return false;
      const add = on ?? !targets.every((it) => tagsOf(tagged, it).includes(tagId));
      const next = { ...tagged };
      for (const item of targets) {
        const now = tagsOf(tagged, item).filter((t) => t !== tagId);
        if (add) now.push(tagId);
        for (const p of item.paths) {
          if (now.length) next[shotKey(item.dir, p)] = now;
          else delete next[shotKey(item.dir, p)];
        }
      }
      set({ tagged: next });
      save();
      return add;
    },

    setTagFilter: (tagFilter) => {
      set({ tagFilter, selection: [], cursor: null });
      save();
    },

    setLook: (ids, look) => {
      const { items, looks } = get();
      const next = { ...looks };
      for (const id of ids) {
        const item = items.find((it) => it.id === id);
        for (const p of item?.paths ?? []) {
          if (look) next[shotKey(item!.dir, p)] = look;
          else delete next[shotKey(item!.dir, p)];
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
          if (crop) next[shotKey(item!.dir, p)] = crop;
          else delete next[shotKey(item!.dir, p)];
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

    setSheet: (sheet) => {
      if (!sheet && offerWaiting) sheet = "update";
      if (sheet === "update") offerWaiting = false;
      set({ sheet });
    },
    notify: (text) => {
      const id = ++toastSeq;
      set({ toast: { id, text } });
      setTimeout(() => get().toast?.id === id && set({ toast: null }), 2600);
    },
    saveToPhotos: async (items) => {
      if (!items.length) return get().notify("selectFirst");
      get().notify("savingToPhotos");
      try {
        const saved = await api.saveToPhotos(items);
        get().notify(`savedToPhotos:${saved},${items.filter((it) => it.live).length}`);
      } catch (e) {
        get().notify(String(e) === "denied" ? "photosDenied" : `photosFailed:${e}`);
      }
    },

    checkUpdate: async (manual = false) => {
      const { info, update } = get();
      if (!info?.updater || ["checking", "downloading", "installing"].includes(update.phase)) return;
      set({ update: { ...NO_UPDATE, phase: "checking" } });
      try {
        const offer = await api.updateCheck();
        set({ update: { ...NO_UPDATE, phase: offer ? "available" : "current", offer } });
        if (!offer || (!manual && offer.version === get().settings.skippedUpdate)) return;
        // A sheet in use is not replaced, except Settings, where the check was asked for.
        const { sheet } = get();
        if (!sheet || sheet === "settings") get().setSheet("update");
        else offerWaiting = sheet !== "update";
      } catch (e) {
        set({ update: manual ? { ...NO_UPDATE, phase: "failed", error: String(e) } : NO_UPDATE });
      }
    },

    installUpdate: async () => {
      const { offer, phase } = get().update;
      if (!offer || phase === "downloading" || phase === "installing") return;
      // The app restarts by itself at the end, so progress is written now.
      saveNow();
      set({ update: { phase: "downloading", offer, progress: null, error: null } });
      const off = await listen<UpdateProgress>("update-progress", ({ payload }) => {
        const now = get().update.phase;
        if (now !== "downloading" && now !== "installing") return;
        set({ update: { phase: payload.installing ? "installing" : "downloading", offer, progress: payload, error: null } });
      }).catch(() => null);
      try {
        await api.updateInstall();
      } catch (e) {
        const error = String(e);
        set({ update: error === "cancelled" ? { ...NO_UPDATE, phase: "available", offer } : { phase: "failed", offer, progress: null, error } });
      } finally {
        off?.();
      }
    },

    cancelUpdate: () => {
      api.updateCancel().catch(() => {});
    },

    skipUpdate: () => {
      const { offer } = get().update;
      if (offer) get().setSettings({ skippedUpdate: offer.version });
      get().setSheet(null);
    },
  };
});
