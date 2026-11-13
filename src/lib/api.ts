import { convertFileSrc, invoke } from "@tauri-apps/api/core";

export type Item = {
  id: string;
  name: string;
  paths: string[];
  raw: boolean;
  paired: boolean;
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
};

export type Histogram = { r: number[]; g: number[]; b: number[]; luma: number[]; shadows: number; highlights: number };

export type Meta = {
  format: string;
  raw: boolean;
  orientation: number;
  rating: number;
  exif: Exif;
  af: { boxes: AfBox[]; source: string; mode: string | null } | null;
  /** In-camera look (Nikon Picture Control); `custom` marks a user or Imaging Cloud recipe. */
  look: { name: string; base: string; custom: boolean } | null;
  /** The camera's Picture Control can be replaced by an NP3, tone curve included, when Nikon's SDK develops the file. */
  look_recordable?: boolean;
  preview_width: number;
  preview_height: number;
  low_res: boolean;
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
  /** Bursts can be exported as Live Photos (Apple platforms). */
  live_photo?: boolean;
};

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
export type CropBox = [number, number, number, number];
export type Rename = { template: string; start: number };

/** Culling progress of one folder, saved continuously. */
export type Session = {
  /** File name -> 1 picked, -1 rejected. */
  flags: Record<string, number>;
  /** File name of the photo last looked at. */
  cursor: string | null;
  viewer: boolean;
  filter: string;
  minStars: number;
  /** File name -> look id chosen for that photo (absent: as shot). */
  looks?: Record<string, string>;
  /** File name -> confirmed crop as normalised [x, y, width, height]. */
  crops?: Record<string, CropBox>;
};

const RAW_EXT = /\.(cr3|cr2|nef|nefx|nrw|arw|sr2|srf|dng|raf|orf|rw2|pef|3fr|iiq|erf|srw)$/i;
export const isRawPath = (path: string) => RAW_EXT.test(path);
export const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

/** URL of a rendition served by the app's `sift://` protocol. */
export function imageUrl(kind: ImageKind, path: string, version: number, edge = 0, look?: string): string {
  const extra = look ? `&l=${encodeURIComponent(look)}` : "";
  return `${convertFileSrc(kind, "sift")}?p=${encodeURIComponent(path)}&v=${version}&e=${edge}${extra}`;
}

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  scanFolder: (path: string, pair: boolean, preferRaw: boolean) => invoke<Item[]>("scan_folder", { path, pair, preferRaw }),
  readMeta: (path: string) => invoke<Meta>("read_meta", { path }),
  readBriefs: (paths: string[]) => invoke<{ rating: number; date: string | null }[]>("read_briefs", { paths }),
  /** Bursts and pixel-shift series among the displayed files of a folder. */
  groupShots: (paths: string[]) => invoke<Stack[]>("group_shots", { paths }),
  /** Seconds each of `frames` is shown for when they play as a Live Photo. */
  livePhotoPace: (frames: string[]) => invoke<number>("live_photo_pace", { frames }),
  /** Writes `frames` as a Live Photo next to them (one `.pvt` bundle holding a JPEG and a MOV), `key` being the still; resolves to the bundle's path. */
  saveLivePhoto: (frames: string[], key: number) => invoke<string>("save_live_photo", { frames, key }),
  setRating: (paths: string[], rating: number) => invoke<"embedded" | "sidecar">("set_rating", { paths, rating }),
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
  ) => invoke<CopyReport>("export_files", { shots, dest, plan: { kind, rename, looks, crops } }),
  listLooks: () => invoke<LookInfo[]>("list_looks"),
  importLooks: (sources: string[]) => invoke<LookInfo[]>("import_looks", { sources }),
  removeLook: (id: string) => invoke<LookInfo[]>("remove_look", { id }),
  /** Saves the Picture Control a photo was shot with to the look library; resolves to its id. */
  saveCameraLook: (path: string) => invoke<string>("save_camera_look", { path }),
  histogram: (path: string, look: string | null, edge: number, crop: CropBox | null) => invoke<Histogram>("histogram", { path, look, edge, crop }),
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
  ) => invoke<CopyReport>("export_framed", { request: { files, dest, options, max_edge: maxEdge, rename, looks, sources, crops } }),
  previewFramed: (path: string, options: FrameOptions) => invoke<ArrayBuffer>("preview_framed", { path, options }),
  findRaws: (picked: string, raws: string, dest: string, loose: boolean) =>
    invoke<CopyReport>("find_raws", { picked, raws, dest, loose }),
  loadSession: (folder: string) => invoke<Partial<Session> | null>("load_session", { folder }),
  saveSession: (folder: string, session: Session) => invoke<void>("save_session", { folder, session }),
  lastSession: () => invoke<string | null>("last_session"),
  clearLastSession: () => invoke<void>("clear_last_session"),
  clearCache: () => invoke<void>("clear_cache"),
  listAlbums: () => invoke<Album[]>("list_albums"),
  importFiles: (sources: string[], album: string) => invoke<string>("import_files", { sources, album }),
  transferStart: (folder: string, files: string[]) => invoke<TransferInfo>("transfer_start", { folder, files }),
  transferStop: () => invoke<void>("transfer_stop"),
};
