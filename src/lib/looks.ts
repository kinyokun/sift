import { open } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { useStore } from "./store";

/** Asks for look files and adds them to the library, saying how many came in. */
export async function importLooks(): Promise<void> {
  const st = useStore.getState();
  const picked = await open({ multiple: true, filters: [{ name: "Looks", extensions: ["cube", "CUBE", "3dl", "3DL", "png", "PNG", "np3", "NP3"] }] });
  const sources = Array.isArray(picked) ? picked : picked ? [picked] : [];
  if (!sources.length) return;
  try {
    const before = st.library.length;
    const next = await api.importLooks(sources);
    st.setLibrary(next);
    st.notify(`lookImported:${Math.max(next.length - before, sources.length)}`);
  } catch (e) {
    st.notify(String(e));
  }
}
