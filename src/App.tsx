import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect } from "react";
import { Library } from "./components/Library";
import { Viewer } from "./components/Viewer";
import { Welcome } from "./components/Welcome";
import { useT } from "./lib/i18n";
import { actionFor, isTyping } from "./lib/shortcuts";
import { useStore } from "./lib/store";
import { ExportSheet, FindRawSheet, TransferSheet } from "./sheets/Export";
import { HelpSheet, SettingsSheet } from "./sheets/Settings";
import { UpdateSheet } from "./sheets/Update";

/** Toasts are raised with short codes so the store stays free of UI text. */
function Toast() {
  const t = useT();
  const toast = useStore((s) => s.toast);
  if (!toast) return null;
  const [code, detail] = toast.text.split(/:(.*)/s);
  const known: Record<string, string> = {
    sidecar: t.sidecarNote,
    ratingFailed: `${t.ratingFailed}${detail ? ` · ${detail}` : ""}`,
    noFace: t.noFace,
    noFaceNoAf: t.noFaceNoAf,
    noFocusData: t.noFocusData,
    originalNeedsRaw: t.originalNeedsRaw,
    resumed: t.resumed,
    lookImported: t.lookImported(Number(detail) || 0),
    lookSaved: t.lookSaved(detail ?? ""),
    lookApplied: t.lookApplied(Number(detail) || 0),
    selectFirst: t.selectFirst,
    tagOn: t.tagOn(detail ?? ""),
    tagOff: t.tagOff(detail ?? ""),
    tagTaken: t.tagTaken,
    livePhotoSaved: t.livePhotoSaved(detail ?? ""),
    livePhotoSavedElsewhere: t.livePhotoSavedElsewhere(detail ?? ""),
    livePhotoFailed: `${t.livePhotoFailed}${detail ? ` · ${detail}` : ""}`,
    savingToPhotos: t.savingToPhotos,
    savedToPhotos: (([n, live]) => t.savedToPhotos(Number(n) || 0, Number(live) || 0))((detail ?? "").split(",")),
    photosDenied: t.photosDenied,
    photosFailed: `${t.photosFailed}${detail ? ` · ${detail}` : ""}`,
    shortcutClash: t.shortcutClash(t.actions[detail as keyof typeof t.actions] ?? detail ?? ""),
    refreshed: (([added, removed]) => t.refreshed(Number(added) || 0, Number(removed) || 0))((detail ?? "").split(",")),
    refreshedSame: t.refreshedSame,
    photoGone: t.photoGone,
    subfoldersShown: t.subfoldersShown(Number(detail) || 0),
  };
  return (
    <div className="toast glass" role="status" key={toast.id}>
      {known[code] ?? toast.text}
    </div>
  );
}

export function App() {
  const folder = useStore((s) => s.folder);
  const viewer = useStore((s) => s.viewer);
  const sheet = useStore((s) => s.sheet);
  const setSheet = useStore((s) => s.setSheet);

  useEffect(() => {
    useStore.getState().init();
    // A dropped folder opens directly; a dropped photo opens its folder.
    const off = getCurrentWebview()
      .onDragDropEvent((e) => {
        if (e.payload.type !== "drop" || !e.payload.paths.length) return;
        const path = e.payload.paths[0];
        const isFile = /\.[a-z0-9]{2,5}$/i.test(path);
        useStore.getState().openFolder(isFile ? path.replace(/[\\/][^\\/]*$/, "") : path);
      })
      .catch(() => null);
    return () => void off.then((f) => f?.());
  }, []);

  // The help key works on every screen and closes the sheet again; so does refreshing, wherever a folder is open.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const st = useStore.getState();
      if (isTyping(e)) return;
      const action = actionFor(st.settings.keys, e);
      if (action === "refresh" && !st.sheet) {
        // Also keeps the webview from reloading the whole page on Ctrl+R.
        e.preventDefault();
        if (st.folder) st.refresh(true);
        return;
      }
      if (action !== "help") return;
      if (st.sheet && st.sheet !== "help") return;
      e.preventDefault();
      st.setSheet(st.sheet === "help" ? null : "help");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const close = () => setSheet(null);
  return (
    <>
      {!folder ? <Welcome /> : <Library />}
      {folder && viewer && <Viewer />}
      {sheet === "export" && folder && <ExportSheet onClose={close} />}
      {sheet === "findraw" && <FindRawSheet onClose={close} />}
      {sheet === "transfer" && folder && <TransferSheet onClose={close} />}
      {sheet === "settings" && <SettingsSheet onClose={close} />}
      {sheet === "help" && <HelpSheet onClose={close} />}
      {sheet === "update" && <UpdateSheet onClose={close} />}
      <Toast />
    </>
  );
}
