import { convertFileSrc, invoke } from "@tauri-apps/api/core";

export type Item = {
  id: string;
  name: string;
  paths: string[];
  raw: boolean;
  paired: boolean;
  /** The folder the shot is in below the opened one ("2026/day 1"); empty for the opened folder itself. */
  dir: string;
  /** The movie of a Live Photo, when this photo has one. */
  live?: string | null;
  size: number;
  mtime: number;
};

/** Shots that belong together: one burst, or a pixel-shift series with its merged file first. */
export type StackKind = "burst" | "pixelshift";
/** `copies` maps a member that is a second copy of an exposure to the member it repeats. */
export type Stack = { kind: StackKind; ids: string[]; copies?: Record<string, string> };

export type AfBox = { x: number; y: number; w: number; h: number; focused: boolean };

export type Exif = {
  make: string | null;
  model: string | null;
  lens: string | null;
  focal: number | null;
  aperture: number | null;
  shutter: number | null;
  iso: number | null;
  bias: number | null;
  datetime: string | null;
  width: number | null;
  height: number | null;
  focal35: number | null;
  /** EXIF ExposureProgram and MeteringMode codes. */
  program: number | null;
  metering: number | null;
  manual_wb: boolean | null;
  flash: boolean | null;
  /** As set in the camera. */
  artist: string | null;
  copyright: string | null;
};

export type Histogram = { r: number[]; g: number[]; b: number[]; luma: number[]; shadows: number; highlights: number };

/** What makes a photo HDR: coded for HDR screens (PQ, HLG), or an ordinary picture with a gain map. */
export type Hdr = "pq" | "hlg" | "gainMap";

/** What the grid sorts, filters and stacks by, read from a photo. */
export type Brief = { rating: number; date: string | null; hdr: Hdr | null };

export type Meta = {
  format: string;
  raw: boolean;
  orientation: number;
  rating: number;
  exif: Exif;
  af: { boxes: AfBox[]; source: string; mode: string | null } | null;
  /** The camera's electronic level: degrees turned clockwise seen from behind, and pointed above the horizon. */
  level?: { roll: number; pitch: number | null } | null;
  /** In-camera look (Nikon Picture Control); `custom` marks a user or Imaging Cloud recipe. */
  look: { name: string; base: string; custom: boolean } | null;
  /** The camera's Picture Control can be replaced by an NP3, tone curve included, when Nikon's SDK develops the file. */
  look_recordable?: boolean;
  preview_width: number;
  preview_height: number;
  low_res: boolean;
  hdr?: Hdr | null;
};

export type AppInfo = {
  platform: string;
  mobile: boolean;
  version: string;
  library: string | null;
  cache_bytes: number;
  dev_open?: string | null;
  dev_view?: number | null;
  /** Nikon's renderer exists for this platform / is part of this build. */
  nikon_supported?: boolean;
  nikon_sdk?: boolean;
  /** Bursts can be saved as Live Photos (macOS and Windows). */
  live_photo?: boolean;
  /** Photos can be saved to the system photo library, Live Photos as Live Photos (iOS). */
  save_to_photos?: boolean;
  /** This build updates itself (official macOS and Windows builds). */
  updater?: boolean;
  /** Deleted photos go into the system's trash (macOS and Windows); elsewhere they are deleted for good. */
  trash?: boolean;
};

/** One release's section of CHANGELOG.md. */
export type Release = { version: string; date: string | null; notes: string };
/** A newer release. `releases`: what changed since the installed version, newest first. `delta_size`: what there
 * is to download when only the changed parts are. `ready`: downloaded and waiting to be put in place. */
export type UpdateOffer = { version: string; current: string; releases: Release[]; delta_size: number | null; ready: boolean };
/** `step`: downloading, laying the new version out beside the installed one, or putting it in place. */
export type UpdateProgress = { downloaded: number; total: number | null; step: "download" | "prepare" | "install" };

export type CopyReport = {
  copied: number;
  skipped: number;
  failed: string[];
  missing: string[];
  cancelled: boolean;
};

export type Progress = { done: number; total: number; name: string };
/** `kind` is "face" for a person, otherwise the animal ("cat", "bird", ...). */
export type EyeTarget = { x: number; y: number; face: [number, number, number, number]; kind: string };
export type Album = { path: string; name: string; count: number; cover: string | null };
export type TransferInfo = { url: string; pin: string; qr: string; count: number; inbox: string };

export type FrameStyle = "bar" | "border" | "glass";
export type FrameOptions = {
  style: FrameStyle;
  dark: boolean;
  camera: boolean;
  lens: boolean;
  params: boolean;
  date: boolean;
  caption: string;
};

export type ImageKind = "thumb" | "screen" | "full" | "dev" | "orig" | "look" | "clip" | "live";
/** A look in the library; `approximate` marks Nikon NP3 recipes, which are emulated. */
export type LookInfo = {
  id: string;
  name: string;
  approximate: boolean;
  /** A Nikon Picture Control (NP3). */
  nikon: boolean;
  /** It has a custom tone curve, which Nikon's SDK only develops from files that can record it. */
  curve: boolean;
};

export function isNefPath(path: string): boolean {
  return /\.(nef|nrw)$/i.test(path);
}
/** Look id meaning "no look at all" (the plain rendering of the RAW). */
export const NO_LOOK = "original";
export type ExportKind = "all" | "raw" | "jpeg";
/** A crop as normalised [x, y, width, height] of the photo as turned, and the angle in degrees (clockwise)
 * the photo is straightened by before it is cut, when it is. */
export type CropBox = [number, number, number, number, number?];
/** Where on a picture the watermark goes: a corner, the middle of a side, or the centre. */
export type MarkPlace = "nw" | "n" | "ne" | "w" | "c" | "e" | "sw" | "s" | "se";
export type MarkKind = "picture" | "text";
/** The watermark on exported pictures: a picture the app keeps (`picture`, an id from [api.setWatermark];
 * `knockout` takes out its plain background) or `text` in Inter at `weight` (100 to 900), with fields filled in
 * from each photo: {year} {date} {artist} {copyright} {camera} {lens} {params} {file}. Sizes are shares of the
 * picture's shorter side: a picture's width, the letters of text. `at` puts its centre on a spot (shares of the
 * width and height) instead of `place`; `tile` repeats it over the whole picture, `gap` apart. `tint` is
 * "own", "auto" (white or black, whichever stands out) or "#rrggbb"; `angle` is in degrees clockwise. */
export type Watermark = {
  kind: MarkKind;
  picture: string;
  knockout: boolean;
  text: string;
  weight: number;
  place: MarkPlace;
  at: [number, number] | null;
  margin: number;
  size: number;
  opacity: number;
  angle: number;
  tile: boolean;
  gap: number;
  tint: string;
  shadow: boolean;
};
/** A watermark picture the app keeps; `backdrop`: it has no transparency but a plain background that can go. */
export type KeptMark = { id: string; backdrop: boolean };
/** How exported pictures come out. `size` is the long edge (0: as they are); any size or a watermark
 * renders every photo again, at `quality`. With `strip`, nothing the camera recorded is kept. */
export type Output = { size: number; quality: number; strip: boolean; watermark: Watermark | null };
export type Rename = { template: string; start: number };
/** A label for sorting photos into piles: a person, a client, a use. `color` is a place in the palette. */
export type Tag = { id: string; name: string; color: number };
/** One folder of an export sorted by tag: the tag's name, and the shots that carry it, by their place in the list. */
export type TagFolder = { tag: string; shots: number[] };

/** Culling progress of one folder, saved continuously. */
export type Session = {
  /** File name ([shotKey]) -> 1 picked, -1 rejected. */
  flags: Record<string, number>;
  /** File name of the photo last looked at. */
  cursor: string | null;
  viewer: boolean;
  filter: string;
  minStars: number;
  hdrOnly?: boolean;
  /** File name -> look id chosen for that photo (absent: as shot). */
  looks?: Record<string, string>;
  /** File name -> confirmed crop as normalised [x, y, width, height]. */
  crops?: Record<string, CropBox>;
  /** File name -> quarter turns clockwise (1 to 3) the photo was given. */
  turns?: Record<string, number>;
  /** The folder inside the opened one the grid was narrowed to. */
  subfolder?: string | null;
  /** The tags of this folder, in the order they were made. */
  tags?: Tag[];
  /** File name -> ids of the tags the photo carries. */
  tagged?: Record<string, string[]>;
  /** Tags the grid was narrowed to. */
  tagFilter?: string[];
};

const RAW_EXT = /\.(cr3|cr2|nef|nefx|nrw|arw|sr2|srf|dng|raf|orf|rw2|rwl|pef|3fr|iiq|erf|srw)$/i;
export const isRawPath = (path: string) => RAW_EXT.test(path);
export const isHeifPath = (path: string) => /\.(heic|heif|hif)$/i.test(path);
export const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;
/** What a file's flag, look and crop are saved under: its name, and below a subfolder (`dir` of its [Item]) its
 * path from the opened folder, since two folders may each have a DSC_0001. */
export const shotKey = (dir: string | undefined, path: string) => (dir ? `${dir}/${fileName(path)}` : fileName(path));

/** URL of a rendition served by the app's `sift://` protocol. */
/** `sdr`: an HDR photo comes as an ordinary picture. */
export function imageUrl(kind: ImageKind, path: string, version: number, edge = 0, look?: string, sdr = false): string {
  const extra = (look ? `&l=${encodeURIComponent(look)}` : "") + (sdr ? "&sdr=1" : "");
  return `${convertFileSrc(kind, "sift")}?p=${encodeURIComponent(path)}&v=${version}&e=${edge}${extra}`;
}

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  /** The shots in a folder; with `deep` also those in the folders inside it. */
  scanFolder: (path: string, pair: boolean, preferRaw: boolean, deep: boolean) => invoke<Item[]>("scan_folder", { path, pair, preferRaw, deep }),
  /** A value that changes when a photo or movie appears in the folder or goes away. */
  folderChanges: (path: string, deep: boolean) => invoke<string>("folder_changes", { path, deep }),
  readMeta: (path: string) => invoke<Meta>("read_meta", { path }),
  /** Makes the thumbnails of `paths` ahead, in the background; stops the run asked for before. */
  warmThumbs: (paths: string[], edge: number) => invoke<void>("warm_thumbs", { paths, edge }),
  /** What was read before from those of `paths` that have not changed since, without opening them. */
  keptBriefs: (paths: string[]) => invoke<(Brief | null)[]>("kept_briefs", { paths }),
  /** `fresh` reads the files again rather than taking what was read from them before. */
  readBriefs: (paths: string[], fresh = false) => invoke<Brief[]>("read_briefs", { paths, fresh }),
  /** Bursts and pixel-shift series among the displayed files of a folder. */
  groupShots: (paths: string[]) => invoke<Stack[]>("group_shots", { paths }),
  /** Seconds each of `frames` is shown for when they play as a Live Photo. */
  livePhotoPace: (frames: string[]) => invoke<number>("live_photo_pace", { frames }),
  /** Writes `frames` as a Live Photo next to them (a still and a MOV of the same name), `key` being the still, a HEIC with `heif` and a JPEG without; resolves to the still's path. */
  saveLivePhoto: (frames: string[], key: number, heif: boolean) => invoke<string>("save_live_photo", { frames, key, heif }),
  setRating: (paths: string[], rating: number) => invoke<"embedded" | "sidecar">("set_rating", { paths, rating }),
  /** Deletes shots, each given as its files: into the trash, or with `forGood` at once. `failed` are the shots,
   * by their place in the list, that are still there, and `reason` is why the first of them is. */
  deletePhotos: (shots: string[][], forGood: boolean) => invoke<{ failed: number[]; reason: string | null }>("delete_photos", { shots, forGood }),
  locateEyes: (path: string, af: AfBox[]) => invoke<EyeTarget | null>("locate_eyes", { path, af }),
  developRaw: (path: string) => invoke<void>("develop_raw", { path }),
  cancelTask: () => invoke<void>("cancel_task"),
  exportFiles: (
    shots: string[][],
    dest: string,
    kind: ExportKind,
    rename: Rename | null,
    looks: Record<string, string>,
    crops: Record<string, CropBox>,
    folders: TagFolder[],
    turns: Record<string, number>,
    output: Output,
  ) => invoke<CopyReport>("export_files", { shots, dest, plan: { kind, rename, looks, crops, folders, turns, output } }),
  listLooks: () => invoke<LookInfo[]>("list_looks"),
  importLooks: (sources: string[]) => invoke<LookInfo[]>("import_looks", { sources }),
  removeLook: (id: string) => invoke<LookInfo[]>("remove_look", { id }),
  /** Saves the Picture Control a photo was shot with to the look library; resolves to its id. */
  saveCameraLook: (path: string) => invoke<string>("save_camera_look", { path }),
  /** `crop` is a rectangle of the upright photo. */
  histogram: (path: string, look: string | null, edge: number, crop: [number, number, number, number] | null) => invoke<Histogram>("histogram", { path, look, edge, crop }),
  exportNames: (shots: string[][], rename: Rename) => invoke<string[]>("export_names", { shots, rename }),
  exportFramed: (
    files: string[],
    dest: string,
    options: FrameOptions,
    maxEdge: number,
    rename: Rename | null,
    looks: Record<string, string>,
    sources: Record<string, string>,
    crops: Record<string, CropBox>,
    folders: TagFolder[],
    turns: Record<string, number>,
    output: Output,
  ) => invoke<CopyReport>("export_framed", { request: { files, dest, options, max_edge: maxEdge, rename, looks, sources, crops, folders, turns, output } }),
  /** A small JPEG of a photo as it will be exported: turned, cropped, watermarked and, given frame options, framed. */
  previewExport: (path: string, options: FrameOptions | null, output: Output, turn: number, crop: CropBox | null) =>
    invoke<ArrayBuffer>("preview_export", { path, options, output, turn, crop }),
  /** Opens files in another program (an `.app` on macOS, an `.exe` on Windows). */
  openWith: (paths: string[], program: string) => invoke<void>("open_with", { paths, program }),
  /** Starts dragging files out of the window while the mouse button is down (macOS, Windows). */
  dragOut: (paths: string[], icon: string, turn: number) => invoke<void>("drag_out", { paths, icon, turn }),
  /** Keeps a picture (a path from the file dialog, or the bytes of a PNG) as a watermark. */
  setWatermark: (source: string | null, data?: number[]) => invoke<KeptMark>("set_watermark", { source, data: data ?? null }),
  /** The text of an SVG from the file dialog. */
  readSvg: (source: string) => invoke<string>("read_svg", { source }),
  /** A watermark picture as PNG bytes; none when there is none. */
  watermark: (id: string) => invoke<ArrayBuffer>("watermark", { id }),
  findRaws: (picked: string, raws: string, dest: string, loose: boolean) =>
    invoke<CopyReport>("find_raws", { picked, raws, dest, loose }),
  loadSession: (folder: string) => invoke<Partial<Session> | null>("load_session", { folder }),
  saveSession: (folder: string, session: Session) => invoke<void>("save_session", { folder, session }),
  lastSession: () => invoke<string | null>("last_session"),
  clearLastSession: () => invoke<void>("clear_last_session"),
  clearCache: () => invoke<void>("clear_cache"),
  listAlbums: () => invoke<Album[]>("list_albums"),
  importFiles: (sources: string[], album: string) => invoke<string>("import_files", { sources, album }),
  /** Saves photos to the system photo library, a Live Photo's movie with its still; rejects with "denied" when not allowed. */
  saveToPhotos: (items: Item[]) => invoke<number>("save_to_photos", { items: items.map((it) => ({ still: it.id, movie: it.live ?? null })) }),
  transferStart: (folder: string, files: string[]) => invoke<TransferInfo>("transfer_start", { folder, files }),
  transferStop: () => invoke<void>("transfer_stop"),
  /** Looks for a newer release; resolves to `null` when this is the newest. */
  updateCheck: () => invoke<UpdateOffer | null>("update_check"),
  /** Downloads and prepares the release found by `updateCheck` (only the changed parts where it can); it is put in
   * place when the app quits. Rejects with "cancelled" after `updateCancel`. Progress arrives as `update-progress` events. */
  updateDownload: () => invoke<void>("update_download"),
  /** Puts the release found by `updateCheck` in place, downloading it first if need be, and restarts the app: it
   * settles only by rejecting. */
  updateInstall: () => invoke<void>("update_install"),
  updateCancel: () => invoke<void>("update_cancel"),
  /** Throws away a downloaded release, so it is not put in place on quitting. */
  updateDiscard: () => invoke<void>("update_discard"),
  /** Tells the macOS menu bar which language the app is shown in. */
  setMenuLanguage: (lang: "zh" | "en") => invoke<void>("set_menu_language", { lang }),
};
