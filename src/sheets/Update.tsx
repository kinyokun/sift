import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useState, type ReactNode } from "react";
import type { Release, UpdateProgress } from "../lib/api";
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
 * They may arrive over the network, so they are laid out here and never inserted as HTML. */
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
  return <>{blocks}</>;
}

/** Releases one after the other, each under its version and date. */
export function ReleaseNotes({ releases }: { releases: Release[] }) {
  const t = useT();
  return (
    <div className="update-notes">
      {releases.map((r) => (
        <section key={r.version} className="release">
          <h4>
            {r.version}
            {r.date && <span>{r.date}</span>}
          </h4>
          {r.notes.trim() ? <Notes text={r.notes} /> : <p className="dim">{t.updateNoNotes}</p>}
        </section>
      ))}
    </div>
  );
}

/** The releases in CHANGELOG.md, newest first, without the unreleased changes. */
export function parseChangelog(text: string): Release[] {
  const releases: Release[] = [];
  let current: { release: Release; lines: string[] } | null = null;
  const finish = () => {
    if (current) releases.push({ ...current.release, notes: current.lines.join("\n").trim() });
  };
  for (const line of text.split(/\r?\n/)) {
    const head = line.match(/^## \[([^\]]+)\](?:\s+-\s+(.+))?\s*$/);
    if (head) {
      finish();
      current = head[1].toLowerCase() === "unreleased" ? null : { release: { version: head[1], date: head[2]?.trim() ?? null, notes: "" }, lines: [] };
      continue;
    }
    // The link definitions at the end belong to no release.
    if (/^\[[^\]]+\]:\s+\S+/.test(line)) {
      finish();
      current = null;
      continue;
    }
    current?.lines.push(line);
  }
  finish();
  return releases;
}

const megabytes = (bytes: number) => (bytes / 1048576).toFixed(1);

function Progress({ progress, installing }: { progress: UpdateProgress | null; installing: boolean }) {
  const t = useT();
  const { downloaded = 0, total = null, step = "download" } = progress ?? {};
  const size = total ? `${megabytes(downloaded)} / ${megabytes(total)} MB` : downloaded ? `${megabytes(downloaded)} MB` : "";
  const label = installing || step === "install" ? t.updateInstalling : step === "prepare" ? t.updatePreparing : `${t.updateDownloading} ${size}`;
  const done = installing || step !== "download" ? 1 : total ? downloaded / total : 0;
  return (
    <div className="progress">
      <div className="progress-track">
        <div style={{ width: `${done * 100}%` }} />
      </div>
      <small>{label}</small>
    </div>
  );
}

export function UpdateSheet({ onClose }: { onClose: () => void }) {
  const t = useT();
  const { phase, offer, progress, error } = useStore((s) => s.update);
  const act = useStore.getState();
  if (!offer) return null;
  const downloading = phase === "downloading";
  const installing = phase === "installing";
  // Closing while it downloads leaves the download running, to be offered again once it is done.
  const later = () => {
    act.updateLater();
    onClose();
  };
  return (
    <Sheet
      title={t.updateTitle}
      onClose={installing ? undefined : later}
      footer={
        downloading || installing ? (
          <>
            <Progress progress={progress} installing={installing} />
            {downloading && (
              <>
                <button onClick={act.cancelUpdate}>{t.cancel}</button>
                <button onClick={later}>{t.updateLater}</button>
              </>
            )}
          </>
        ) : phase === "failed" ? (
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
            <button onClick={later}>{t.updateLater}</button>
            <button className="primary" onClick={act.installUpdate}>
              {phase === "ready" ? t.updateRestart : t.updateNow}
            </button>
          </>
        )
      }
    >
      <div className="update">
        <p className="update-versions">
          Sift {offer.current} <span aria-hidden="true">→</span> <strong>{offer.version}</strong>
          {offer.delta_size != null && phase !== "ready" && <span> · {t.updateDeltaSize(megabytes(offer.delta_size))}</span>}
        </p>
        {phase === "ready" && <p className="note">{t.updateReadyNote}</p>}
        {downloading && <p className="note">{t.updateBackgroundNote}</p>}
        {phase === "failed" && (
          <p className="note bad">
            {t.updateFailed}
            {error ? ` · ${error}` : ""}
            <br />
            {t.updateFailedHint}
          </p>
        )}
        <ReleaseNotes releases={offer.releases} />
      </div>
    </Sheet>
  );
}

/** Every release so far, from the CHANGELOG.md this build was made with. */
export function ChangelogSheet({ onClose }: { onClose: () => void }) {
  const t = useT();
  const [releases, setReleases] = useState<Release[] | null>(null);
  useEffect(() => {
    import("../../CHANGELOG.md?raw").then(({ default: text }) => setReleases(parseChangelog(text))).catch(() => setReleases([]));
  }, []);
  return (
    <Sheet title={t.changelog} onClose={onClose} wide>
      {releases && <ReleaseNotes releases={releases} />}
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
    : phase === "ready" && offer ? t.updateReady(offer.version)
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
        <Row label={t.updateAuto} hint={t.updateAutoHint}>
          <Toggle
            checked={auto}
            onChange={(checkUpdates) => {
              act.setSettings({ checkUpdates });
              if (checkUpdates) act.checkUpdate();
            }}
          />
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
