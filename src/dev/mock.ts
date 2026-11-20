// Development only: lets the UI run in a normal browser by routing Tauri
// calls to the HTTP bridge (`cargo run -p sift --example bridge`).
// Open http://localhost:1420/?bridge=http://127.0.0.1:4777&pick=/path/to/photos
//
// The bridge has no updater, so the update screens are driven from here:
// `&update=0.4.0` offers that version, `&update=none` finds nothing and
// `&update=fail` fails the check; `&install=fail` breaks the download part
// way and `&install=stall` leaves it there. The download is a 7.1 MB delta,
// or with `&delta=none` the 187 MB package.
//
// Deleting: `&trash=none` plays a platform without a trash, and with
// `&trash=refuse` the trash takes nothing, as on a network share.
const params = new URLSearchParams(location.search);
const base = params.get("bridge")!;
let callbacks = 0;

type Internals = Record<string, unknown>;
const w = window as unknown as { __TAURI_INTERNALS__: Internals; __TAURI_EVENT_PLUGIN_INTERNALS__: Internals } & Record<string, unknown>;

/** Callback ids of the listeners of each event. */
const listeners = new Map<string, number[]>();
const emit = (event: string, payload: unknown) => {
  for (const id of listeners.get(event) ?? []) (w[`_${id}`] as (e: unknown) => void)({ event, id, payload });
};
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

const UPDATE_NOTES = `### 新增

- 对比可以放 2、3 或 4 张照片（四张是 2 × 2），在底栏选张数，或按 \`Shift+C\` 切换
- 在列表里选中 2 到 4 张后，点选择栏的对比按钮或按 \`C\`，直接对比这几张

### 修复

- 列表里多选后打开查看页，选择栏不再盖在查看页底栏上
- 佳能 HDR PQ 模式拍的 CR3 以前没有缩略图；现在从 RAW 显影一次再缩放显示
`;

let cancelled = false;
/** The version the fake download has finished, as the app keeps it until it quits. */
let downloaded: string | null = null;
const MB = 1048576;

/** The releases from the installed one to `found`: `found` itself and, for show, the one before it. */
function releases(found: string) {
  const [major, minor, patch] = found.split(".").map(Number);
  const list = [{ version: found, date: "2026-11-21", notes: UPDATE_NOTES }];
  if (patch > 0) list.push({ version: `${major}.${minor}.${patch - 1}`, date: "2026-11-20", notes: "### 修复\n\n- 导出大量照片时进度条不再停在 99%" });
  return list;
}

async function fakeDownload(found: string) {
  const mode = params.get("install");
  const total = params.get("delta") === "none" ? 187 * MB : 7.1 * MB;
  cancelled = false;
  for (let step = 1; step <= 20; step++) {
    await wait(120);
    if (cancelled) throw "cancelled";
    emit("update-progress", { downloaded: (total * step) / 20, total, step: "download" });
    if (step === 8 && mode === "fail") throw "error sending request for url (https://ghfast.top/…)";
    if (step === 8 && mode === "stall") {
      while (!cancelled) await wait(100);
      throw "cancelled";
    }
  }
  emit("update-progress", { downloaded: total, total, step: "prepare" });
  await wait(600);
  downloaded = found;
}

async function fakeInstall(found: string) {
  if (downloaded !== found) await fakeDownload(found);
  emit("update-progress", { downloaded: 0, total: null, step: "install" });
  // The real app restarts here.
  return new Promise(() => {});
}
w.__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
  transformCallback(fn: unknown) {
    const id = ++callbacks;
    w[`_${id}`] = fn;
    return id;
  },
  unregisterCallback() {},
  convertFileSrc: (path: string) => `${base}/img/${path}`,
  async invoke(cmd: string, args: unknown) {
    if (cmd === "plugin:event|listen") {
      const { event, handler } = args as { event: string; handler: number };
      listeners.set(event, [...(listeners.get(event) ?? []), handler]);
      return handler;
    }
    if (cmd.startsWith("plugin:event|")) return 0;
    if (cmd === "plugin:dialog|open") {
      // A dialog with file filters is the look picker, the watermark's (`&mark=`) or the editor's (`&editor=`);
      // anything else picks the photo folder.
      const filters = (args as { options?: { filters?: { extensions: string[] }[] } })?.options?.filters ?? [];
      const extensions = filters.flatMap((f) => f.extensions);
      if (extensions.includes("app") || extensions.includes("exe")) return params.get("editor");
      if (extensions.includes("svg")) return params.get("mark");
      return filters.length ? params.getAll("look") : params.get("pick");
    }
    if (cmd.startsWith("plugin:")) return null;
    if (cmd === "import_files") return params.get("pick");
    if (cmd === "update_check") {
      await wait(400);
      const found = params.get("update");
      if (found === "fail") throw "error sending request for url (https://github.com/kinyokun/sift/releases/latest/download/latest.json)";
      if (!found || found === "none") return null;
      const info = await (await fetch(`${base}/invoke/app_info`, { method: "POST", body: "{}" })).json();
      const delta_size = params.get("delta") === "none" ? null : 7.1 * MB;
      return { version: found, current: info.version, releases: releases(found), delta_size, ready: downloaded === found };
    }
    if (cmd === "update_download") return fakeDownload(params.get("update")!);
    if (cmd === "update_install") return fakeInstall(params.get("update")!);
    if (cmd === "update_cancel") return void (cancelled = true);
    if (cmd === "update_discard") return void (downloaded = null);
    if (cmd === "delete_photos" && params.get("trash") === "refuse") {
      const { shots, forGood } = args as { shots: string[][]; forGood: boolean };
      if (!forGood) return { failed: shots.map((_, i) => i), reason: "The volume does not have a trash." };
    }
    const res = await fetch(`${base}/invoke/${cmd}`, { method: "POST", body: JSON.stringify(args ?? {}) });
    if (!res.ok) throw await res.text();
    if (!(res.headers.get("content-type") || "").includes("json")) return res.arrayBuffer();
    const value = await res.json();
    if (cmd === "app_info") {
      value.mobile = params.has("mobile");
      value.platform = params.get("platform") ?? value.platform;
      value.updater = params.has("update") && !value.mobile;
      if (value.mobile || params.get("trash") === "none") value.trash = false;
    }
    return value;
  },
};
w.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener(event: string, id: number) {
    listeners.set(event, (listeners.get(event) ?? []).filter((l) => l !== id));
  },
};

export {};
