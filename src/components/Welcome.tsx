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
    <svg className="mark" viewBox="236 236 560 560" aria-hidden="true">
      <path
        d="M560 292C544 289 528 288 510 288C430 288 372 330 372 398C372 468 432 492 512 514C596 538 660 566 660 638C660 708 596 744 512 744C450 744 400 722 372 688"
        fill="none"
        stroke="currentColor"
        strokeWidth="84"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <circle className="mark-focus" cx="642" cy="352" r="46" />
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
