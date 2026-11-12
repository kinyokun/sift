// Development only: lets the UI run in a normal browser by routing Tauri
// calls to the HTTP bridge (`cargo run -p sift --example bridge`).
// Open http://localhost:1420/?bridge=http://127.0.0.1:4777&pick=/path/to/photos
const params = new URLSearchParams(location.search);
const base = params.get("bridge")!;
let callbacks = 0;

type Internals = Record<string, unknown>;
const w = window as unknown as { __TAURI_INTERNALS__: Internals; __TAURI_EVENT_PLUGIN_INTERNALS__: Internals } & Record<string, unknown>;

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
    if (cmd.startsWith("plugin:event|")) return 0;
    if (cmd === "plugin:dialog|open") {
      // A dialog with file filters is the look picker; anything else picks the photo folder.
      const filters = (args as { options?: { filters?: { extensions: string[] }[] } })?.options?.filters ?? [];
      return filters.length ? params.getAll("look") : params.get("pick");
    }
    if (cmd.startsWith("plugin:")) return null;
    if (cmd === "import_files") return params.get("pick");
    const res = await fetch(`${base}/invoke/${cmd}`, { method: "POST", body: JSON.stringify(args ?? {}) });
    if (!res.ok) throw await res.text();
    if (!(res.headers.get("content-type") || "").includes("json")) return res.arrayBuffer();
    const value = await res.json();
    if (cmd === "app_info") {
      value.mobile = params.has("mobile");
      value.platform = params.get("platform") ?? value.platform;
    }
    return value;
  },
};
w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };

export {};
