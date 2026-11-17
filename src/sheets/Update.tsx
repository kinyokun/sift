import { openUrl } from "@tauri-apps/plugin-opener";
import type { ReactNode } from "react";
import type { UpdateProgress } from "../lib/api";
import { useT } from "../lib/i18n";
import { useStore } from "../lib/store";
import { Row, Sheet, Toggle } from "../ui/controls";

/** Where an update can be fetched by hand when the app cannot do it. */
const RELEASES = "https://github.com/kinyokun/sift/releases/latest";

function openReleases() {
  openUrl(RELEASES).catch((e) => useStore.getState().notify(String(e)));
}

/** `code`, **bold** and [links](…) of a line of Markdown; a link keeps only its text. */
function inline(text: string): ReactNode[] {
  return text.split(/(`[^`]+`|\*\*[^*]+\*\*|\[[^\]]+\]\([^)]*\))/).map((part, i) => {
    if (/^`.+`$/.test(part)) return <code key={i}>{part.slice(1, -1)}</code>;
    if (/^\*\*.+\*\*$/.test(part)) return <strong key={i}>{part.slice(2, -2)}</strong>;
    const link = part.match(/^\[([^\]]+)\]\(/);
    return link ? link[1] : part;
  });
}

/** Release notes are a section of CHANGELOG.md: headings, bullet lists and paragraphs.
 * They arrive over the network, so they are laid out here and never inserted as HTML. */
function Notes({ text }: { text: string }) {
  const blocks: ReactNode[] = [];
  let items: string[] = [];
  const flush = () => {
    if (!items.length) return;
    blocks.push(
      <ul key={blocks.length}>
        {items.map((item, i) => (
          <li key={i}>{inline(item)}</li>
        ))}
      </ul>,
    );
    items = [];
  };
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    const heading = line.match(/^#{1,6}\s+(.*)$/);
    const bullet = line.match(/^[-*]\s+(.*)$/);
    if (bullet) items.push(bullet[1]);
    else if (items.length && line && /^\s/.test(raw)) items[items.length - 1] += ` ${line}`;
    else {
      flush();
      if (heading) blocks.push(<h5 key={blocks.length}>{inline(heading[1])}</h5>);
      else if (line) blocks.push(<p key={blocks.length}>{inline(line)}</p>);
    }
  }
  flush();
  return <div className="update-notes">{blocks}</div>;
}

const megabytes = (bytes: number) => (bytes / 1048576).toFixed(1);

function Download({ progress, installing }: { progress: UpdateProgress | null; installing: boolean }) {
  const t = useT();
  const { downloaded = 0, total = null } = progress ?? {};
  const size = total ? `${megabytes(downloaded)} / ${megabytes(total)} MB` : downloaded ? `${megabytes(downloaded)} MB` : "";
  return (
    <div className="progress">
      <div className="progress-track">
        <div style={{ width: `${installing ? 100 : total ? (downloaded / total) * 100 : 0}%` }} />
      </div>
      <small>{installing ? t.updateInstalling : `${t.updateDownloading} ${size}`}</small>
    </div>
  );
}

export function UpdateSheet({ onClose }: { onClose: () => void }) {
  const t = useT();
  const { phase, offer, progress, error } = useStore((s) => s.update);
  const act = useStore.getState();
  if (!offer) return null;
  const busy = phase === "downloading" || phase === "installing";
  const failed = phase === "failed";
  return (
    <Sheet
      title={t.updateTitle}
      onClose={busy ? undefined : onClose}
      footer={
        busy ? (
          <>
            <Download progress={progress} installing={phase === "installing"} />
            {phase === "downloading" && <button onClick={act.cancelUpdate}>{t.cancel}</button>}
          </>
        ) : failed ? (
          <>
            <button onClick={openReleases}>{t.updateOpenReleases}</button>
            <span className="spacer" />
            <button onClick={onClose}>{t.close}</button>
            <button className="primary" onClick={act.installUpdate}>
              {t.retry}
            </button>
          </>
        ) : (
          <>
            <button onClick={act.skipUpdate}>{t.updateSkip}</button>
            <span className="spacer" />
            <button onClick={onClose}>{t.updateLater}</button>
            <button className="primary" onClick={act.installUpdate}>
              {t.updateNow}
            </button>
          </>
        )
      }
    >
      <div className="update">
        <p className="update-versions">
          Sift {offer.current} <span aria-hidden="true">→</span> <strong>{offer.version}</strong>
        </p>
        {failed && (
          <p className="note bad">
            {t.updateFailed}
            {error ? ` · ${error}` : ""}
            <br />
            {t.updateFailedHint}
          </p>
        )}
        {offer.notes.trim() ? <Notes text={offer.notes} /> : <p className="note">{t.updateNoNotes}</p>}
      </div>
    </Sheet>
  );
}

/** The "Updates" part of Settings; official desktop builds only. */
export function UpdateSettings() {
  const t = useT();
  const version = useStore((s) => s.info?.version);
  const auto = useStore((s) => s.settings.checkUpdates);
  const { phase, offer, error } = useStore((s) => s.update);
  const act = useStore.getState();
  const checkFailed = phase === "failed" && !offer;
  const status =
    phase === "checking" ? t.updateChecking
    : phase === "current" ? t.updateCurrent
    : checkFailed ? t.updateCheckFailed
    : phase === "downloading" || phase === "installing" ? t.updateDownloading
    : offer ? t.updateAvailable(offer.version)
    : undefined;
  return (
    <>
      <h4>{t.updates}</h4>
      <div className="group">
        <Row label={`Sift ${version ?? ""}`} hint={status}>
          {offer ? (
            <button className="link" onClick={() => act.setSheet("update")}>
              {t.view}
            </button>
          ) : (
            <button className="link" disabled={phase === "checking"} onClick={() => act.checkUpdate(true)}>
              {t.updateCheck}
            </button>
          )}
        </Row>
        <Row label={t.updateAuto}>
          <Toggle checked={auto} onChange={(checkUpdates) => act.setSettings({ checkUpdates })} />
        </Row>
      </div>
      {checkFailed && (
        <p className="note">
          {error}{" "}
          <button className="link" onClick={openReleases}>
            {t.updateOpenReleases}
          </button>
        </p>
      )}
    </>
  );
}
