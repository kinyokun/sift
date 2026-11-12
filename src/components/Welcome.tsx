import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, imageUrl, type Album } from "../lib/api";
import { useT } from "../lib/i18n";
import { folderName, useStore } from "../lib/store";
import { Icon } from "../ui/Icon";

export async function chooseFolder(): Promise<string | null> {
  const picked = await open({ directory: true, multiple: false });
  return typeof picked === "string" ? picked : null;
}

/** Mobile: pick files in the system picker and copy them into a new album. */
export async function importPhotos(): Promise<string | null> {
  const picked = await open({ multiple: true, directory: false });
  const sources = Array.isArray(picked) ? picked : picked ? [picked] : [];
  if (!sources.length) return null;
  const stamp = new Date().toISOString().slice(0, 16).replace("T", " ").replace(":", ".");
  return api.importFiles(sources, stamp);
}

function Mark() {
  return (
    <svg className="mark" viewBox="0 0 64 64" aria-hidden="true">
      <g fill="none" stroke="currentColor" strokeWidth="4" strokeLinecap="round" strokeLinejoin="round">
        <path d="M10 22v-6a6 6 0 0 1 6-6h6M42 10h6a6 6 0 0 1 6 6v6M54 42v6a6 6 0 0 1-6 6h-6M22 54h-6a6 6 0 0 1-6-6v-6" />
        <rect className="mark-focus" x="24" y="24" width="16" height="16" rx="4" />
      </g>
    </svg>
  );
}

export function Welcome() {
  const t = useT();
  const info = useStore((s) => s.info);
  const recents = useStore((s) => s.settings.recents);
  const openFolder = useStore((s) => s.openFolder);
  const setSheet = useStore((s) => s.setSheet);
  const notify = useStore((s) => s.notify);
  const [albums, setAlbums] = useState<Album[]>([]);
  const [busy, setBusy] = useState(false);
  const mobile = !!info?.mobile;

  useEffect(() => {
    if (mobile) api.listAlbums().then(setAlbums).catch(() => {});
  }, [mobile]);

  const primary = async () => {
    try {
      if (!mobile) {
        const dir = await chooseFolder();
        if (dir) await openFolder(dir);
        return;
      }
      setBusy(true);
      const album = await importPhotos();
      if (album) await openFolder(album);
    } catch (e) {
      notify(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="welcome" data-tauri-drag-region>
      <div className="welcome-card">
        <Mark />
        <h1>Sift</h1>
        <p className="tagline">{t.appTagline}</p>
        <button className="primary large" onClick={primary} disabled={busy}>
          {mobile ? t.importPhotos : t.openFolder}
        </button>
        {!mobile && <p className="hint">{t.dropHint}</p>}
      </div>

      {mobile && albums.length > 0 && (
        <section className="albums">
          <h3>{t.albums}</h3>
          <div className="album-grid">
            {albums.map((a) => (
              <button key={a.path} className="album" onClick={() => openFolder(a.path)}>
                {a.cover && <img src={imageUrl("thumb", a.cover, 0, 512)} alt="" />}
                <span>{a.name || t.allPhotos}</span>
                <small>{t.photos(a.count)}</small>
              </button>
            ))}
          </div>
        </section>
      )}
      {mobile && albums.length === 0 && <p className="hint center">{t.emptyLibraryHint}</p>}

      {!mobile && recents.length > 0 && (
        <section className="recents">
          <h3>{t.recent}</h3>
          {recents.map((r) => (
            <button key={r} onClick={() => openFolder(r)} title={r}>
              <Icon name="folder" size={18} />
              <span>{folderName(r)}</span>
              <small>{r}</small>
            </button>
          ))}
        </section>
      )}

      <button className="link help-link" onClick={() => setSheet("help")}>
        {t.help}
      </button>
    </main>
  );
}
