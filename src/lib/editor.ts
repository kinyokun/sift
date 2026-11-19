import { open } from "@tauri-apps/plugin-dialog";
import { api, isRawPath, type Item } from "./api";
import { stringsFor } from "./i18n";
import { useStore } from "./store";

/** What an editor is called: its file name without `.app` or `.exe`. */
export function editorName(program: string): string {
  return (program.split(/[\\/]/).pop() ?? program).replace(/\.(app|exe)$/i, "");
}

/** Asks for the program photos are edited in and remembers it; null when the user gives none. */
export async function chooseEditor(): Promise<string | null> {
  const st = useStore.getState();
  const mac = st.info?.platform === "macos";
  const picked = await open({
    multiple: false,
    directory: false,
    defaultPath: mac ? "/Applications" : undefined,
    filters: [{ name: stringsFor(st.settings.lang).editorApp, extensions: [mac ? "app" : "exe"] }],
  });
  if (typeof picked !== "string") return null;
  st.setSettings({ editor: picked });
  return picked;
}

/** Opens photos in the editor, asking for one the first time; a pair opens its RAW unless that is switched off. */
export async function editIn(items: Item[]): Promise<void> {
  const st = useStore.getState();
  if (!items.length || st.info?.mobile) return;
  const program = st.settings.editor ?? (await chooseEditor());
  if (!program) return;
  const files = items.map((it) => (st.settings.editorRaw && it.paths.find(isRawPath)) || it.id);
  try {
    await api.openWith(files, program);
  } catch (e) {
    st.notify(String(e));
  }
}
