import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect, useMemo, useRef, useState } from "react";
import { api, isRawPath, type CropBox, type CopyReport, type ExportKind, type FrameOptions, type FrameStyle, type Item, type Progress, type Rename } from "../lib/api";
import { useT } from "../lib/i18n";
import { cropOf, flagOf, folderName, lookOf, useStore, visibleItems } from "../lib/store";
import { Row, Segmented, Sheet, Toggle } from "../ui/controls";

const LAST_DEST_KEY = "sift.exportDest";
const LAST_RENAME_KEY = "sift.exportRename";
const DEFAULT_TEMPLATE = "{date}_{seq}";
const SIZES = [0, 4096, 2560, 2048];

type Scope = "picked" | "selection" | "shown";

/** Photos an action applies to: picks first, then a multi-selection, then everything shown. */
export function useScopes(): { scopes: Record<Scope, Item[]>; initial: Scope } {
  const s = useStore();
  return useMemo(() => {
    const shown = visibleItems(s);
    const picked = s.items.filter((it) => flagOf(s.flags, it) === 1);
    const chosen = new Set(s.selection);
    const selection = s.selection.length > 1 ? s.items.filter((it) => chosen.has(it.id)) : [];
    const initial: Scope = picked.length ? "picked" : selection.length ? "selection" : "shown";
    return { scopes: { picked, selection, shown }, initial };
  }, [s.items, s.flags, s.selection, s.filter, s.minStars, s.ratings]);
}

function useProgress(): Progress | null {
  const [progress, setProgress] = useState<Progress | null>(null);
  useEffect(() => {
    const off = listen<Progress>("task-progress", (e) => setProgress(e.payload));
    return () => void off.then((f) => f());
  }, []);
  return progress;
}

function ProgressBar({ progress }: { progress: Progress | null }) {
  const ratio = progress && progress.total ? progress.done / progress.total : 0;
  return (
    <div className="progress">
      <div className="progress-track">
        <div style={{ width: `${ratio * 100}%` }} />
      </div>
      <small>{progress?.name || "\u00a0"}</small>
    </div>
  );
}

function Report({ report, dest }: { report: CopyReport; dest: string }) {
  const t = useT();
  const mobile = useStore((s) => !!s.info?.mobile);
  return (
    <div className="report">
      <strong>{report.cancelled ? t.cancelled : t.exported(report.copied)}</strong>
      {report.skipped > 0 && <span>{t.skippedExisting(report.skipped)}</span>}
      {report.missing.length > 0 && <span>{t.missingRaw(report.missing.length)}</span>}
      {report.failed.length > 0 && <span className="bad">{t.failedCount(report.failed.length)}</span>}
      {(report.missing.length > 0 || report.failed.length > 0) && <pre>{[...report.failed, ...report.missing].slice(0, 200).join("\n")}</pre>}
      {!mobile && (
        <button className="link" onClick={() => revealItemInDir(dest).catch(() => {})}>
          {t.reveal}
        </button>
      )}
    </div>
  );
}

function FolderRow(props: { label: string; value: string; onChange: (dir: string) => void }) {
  const t = useT();
  const pick = async () => {
    const dir = await open({ directory: true, multiple: false, defaultPath: props.value || undefined });
    if (typeof dir === "string") props.onChange(dir);
  };
  return (
    <Row label={props.label} hint={props.value || t.notChosen} onClick={pick}>
      <span className="link">{t.choose}</span>
    </Row>
  );
}

export function ExportSheet({ onClose }: { onClose: () => void }) {
  const t = useT();
  const folder = useStore((s) => s.folder)!;
  const mobile = useStore((s) => !!s.info?.mobile);
  const { scopes, initial } = useScopes();
  const [scope, setScope] = useState<Scope>(initial);
  const [mode, setMode] = useState<"copy" | "frame">(mobile ? "frame" : "copy");
  const [dest, setDest] = useState(() => (mobile ? `${folder}/Export` : localStorage.getItem(LAST_DEST_KEY) || ""));
  const [options, setOptions] = useState<FrameOptions>({ style: "bar", dark: false, camera: true, lens: true, params: true, date: true, caption: "" });
  const [maxEdge, setMaxEdge] = useState(0);
  const [kind, setKind] = useState<ExportKind>("all");
  const [renaming, setRenaming] = useState(false);
  const [template, setTemplate] = useState(() => localStorage.getItem(LAST_RENAME_KEY) || DEFAULT_TEMPLATE);
  const [start, setStart] = useState(1);
  const [example, setExample] = useState("");
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<CopyReport | null>(null);
  const [preview, setPreview] = useState<string | null>(null);
  const progress = useProgress();
  const items = scopes[scope];
  const sample = items[0]?.id;
  const rename: Rename | null = renaming && template.trim() ? { template: template.trim(), start: Math.max(0, start) } : null;
  // Files of each shot that the chosen type keeps.
  const shots = useMemo(
    () =>
      items
        .map((it) => (mode === "frame" ? [it.id] : it.paths.filter((p) => kind === "all" || (kind === "raw") === isRawPath(p))))
        .filter((shot) => shot.length > 0),
    [items, kind, mode],
  );
  const hasRaw = items.some((it) => it.paths.some(isRawPath));
  // Looks chosen while culling are applied on export.
  const chosen = useStore((s) => s.looks);
  const cropped = useStore((s) => s.crops);
  const looks: Record<string, string> = {};
  const sources: Record<string, string> = {};
  const crops: Record<string, CropBox> = {};
  let styled = 0;
  for (const it of items) {
    const look = lookOf(chosen, it);
    const crop = cropOf(cropped, it);
    if (look || crop) styled++;
    for (const p of it.paths) {
      if (look) looks[p] = look;
      if (crop) crops[p] = crop;
    }
    if (look) sources[it.id] = it.paths.find(isRawPath) ?? it.id;
  }
  // The copy job gets every file of a shot; a look is rendered from its RAW even when only JPEGs are wanted.
  const wanted = items.map((it) => it.paths).filter((paths) => kind === "all" || paths.some((p) => (kind === "raw") === isRawPath(p)));

  // Show what the first and last file will be called.
  useEffect(() => {
    if (!rename || !shots.length) return setExample("");
    let stale = false;
    const ends = shots.length > 1 ? [shots[0], shots[shots.length - 1]] : [shots[0]];
    const timer = setTimeout(async () => {
      const names = await api.exportNames(ends, rename).catch(() => []);
      if (stale || !names.length) return;
      // The preview numbers two shots; the real last number depends on the count.
      const last = shots.length > 1 ? await api.exportNames(shots, rename).then((all) => all[all.length - 1]).catch(() => "") : "";
      if (!stale) setExample(last ? `${names[0]}  …  ${last}` : names[0]);
    }, 200);
    return () => {
      stale = true;
      clearTimeout(timer);
    };
  }, [renaming, template, start, shots]);

  // Live preview of the frame on the first photo.
  useEffect(() => {
    if (mode !== "frame" || !sample) return;
    let url: string | null = null;
    let stale = false;
    const timer = setTimeout(async () => {
      try {
        const bytes = await api.previewFramed(sample, options);
        if (stale) return;
        url = URL.createObjectURL(new Blob([bytes], { type: "image/jpeg" }));
        setPreview(url);
      } catch {
        setPreview(null);
      }
    }, 180);
    return () => {
      stale = true;
      clearTimeout(timer);
      if (url) URL.revokeObjectURL(url);
    };
  }, [mode, sample, JSON.stringify(options)]);

  const run = async () => {
    setBusy(true);
    setReport(null);
    try {
      if (!mobile) localStorage.setItem(LAST_DEST_KEY, dest);
      if (rename) localStorage.setItem(LAST_RENAME_KEY, rename.template);
      const result =
        mode === "copy"
          ? await api.exportFiles(wanted, dest, kind, rename, looks, crops)
          : await api.exportFramed(shots.map((shot) => shot[0]), dest, options, maxEdge, rename, looks, sources, crops);
      setReport(result);
    } catch (e) {
      useStore.getState().notify(String(e));
    } finally {
      setBusy(false);
    }
  };

  const scopeOptions = (
    [
      ["picked", t.scopePicked(scopes.picked.length)],
      ["selection", t.scopeSelection(scopes.selection.length)],
      ["shown", t.scopeShown(scopes.shown.length)],
    ] as [Scope, string][]
  )
    .filter(([id]) => scopes[id].length > 0 || id === "shown")
    .map(([value, label]) => ({ value, label }));
  const set = (patch: Partial<FrameOptions>) => setOptions({ ...options, ...patch });
  const fields: [keyof FrameOptions, string][] = [
    ["camera", t.fieldCamera],
    ["lens", t.fieldLens],
    ["params", t.fieldParams],
    ["date", t.fieldDate],
  ];

  return (
    <Sheet
      title={t.exportTitle}
      onClose={onClose}
      wide={mode === "frame"}
      footer={
        busy ? (
          <>
            <ProgressBar progress={progress} />
            <button onClick={() => api.cancelTask()}>{t.cancel}</button>
          </>
        ) : (
          <>
            <span className="spacer" />
            <button onClick={onClose}>{report ? t.done : t.cancel}</button>
            {!report && (
              <button className="primary" disabled={!shots.length || !dest} onClick={run}>
                {t.start}
              </button>
            )}
          </>
        )
      }
    >
      {report ? (
        <Report report={report} dest={dest} />
      ) : (
        <div className={mode === "frame" ? "split" : ""}>
          <div className="form">
            <Segmented value={scope} options={scopeOptions} onChange={setScope} />
            {!mobile && (
              <Segmented
                value={mode}
                options={[
                  { value: "copy", label: t.modeCopy },
                  { value: "frame", label: t.modeFrame },
                ]}
                onChange={setMode}
              />
            )}
            <div className="group">
              {!mobile && <FolderRow label={t.destination} value={dest} onChange={setDest} />}
              {mode === "copy" && hasRaw && (
                <Row label={t.fileTypes}>
                  <Segmented
                    value={kind}
                    options={[
                      { value: "all", label: t.kindAll },
                      { value: "raw", label: t.kindRaw },
                      { value: "jpeg", label: t.kindJpeg },
                    ]}
                    onChange={setKind}
                  />
                </Row>
              )}
              <Row label={t.renameFiles} hint={renaming ? example && `${t.renameExample}: ${example}` : undefined}>
                <Toggle checked={renaming} onChange={setRenaming} />
              </Row>
              {renaming && (
                <div className="row rename">
                  <input className="text" value={template} spellCheck={false} onChange={(e) => setTemplate(e.target.value)} aria-label={t.renameFiles} />
                  <label>
                    {t.renameStart}
                    <input className="text number" type="number" min={0} value={start} onChange={(e) => setStart(Number(e.target.value) || 0)} />
                  </label>
                </div>
              )}
              {mode === "frame" && (
                <>
                  <Row label={t.frameStyle}>
                    <Segmented
                      value={options.style}
                      options={(["bar", "border", "glass"] as FrameStyle[]).map((v) => ({
                        value: v,
                        label: v === "bar" ? t.styleBar : v === "border" ? t.styleBorder : t.styleGlass,
                      }))}
                      onChange={(style) => set({ style })}
                    />
                  </Row>
                  {options.style !== "glass" && (
                    <Row label={t.dark}>
                      <Toggle checked={options.dark} onChange={(dark) => set({ dark })} />
                    </Row>
                  )}
                  <Row label={t.outputSize}>
                    <select value={maxEdge} onChange={(e) => setMaxEdge(Number(e.target.value))}>
                      {SIZES.map((n) => (
                        <option key={n} value={n}>
                          {n ? `${n} px` : t.sizeOriginal}
                        </option>
                      ))}
                    </select>
                  </Row>
                </>
              )}
            </div>
            {mode === "frame" && (
              <>
                <div className="chips">
                  {fields.map(([key, label]) => (
                    <button key={key} className={options[key] ? "on" : ""} onClick={() => set({ [key]: !options[key] })}>
                      {label}
                    </button>
                  ))}
                </div>
                <input className="text" placeholder={t.caption} value={options.caption} onChange={(e) => set({ caption: e.target.value })} />
              </>
            )}
            {renaming && <p className="note">{t.renameHint}</p>}
            {styled > 0 && kind !== "raw" && <p className="note">{t.lookOnExport}</p>}
            {!items.length && <p className="note">{t.nothingToExport}</p>}
            {items.length > 0 && !shots.length && <p className="note bad">{t.nothingOfKind}</p>}
          </div>
          {mode === "frame" && <div className="frame-preview">{preview && <img src={preview} alt="" />}</div>}
        </div>
      )}
    </Sheet>
  );
}

export function FindRawSheet({ onClose }: { onClose: () => void }) {
  const t = useT();
  const folder = useStore((s) => s.folder) ?? "";
  const [picked, setPicked] = useState(folder);
  const [raws, setRaws] = useState("");
  const [dest, setDest] = useState("");
  const [loose, setLoose] = useState(true);
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<CopyReport | null>(null);
  const progress = useProgress();
  const target = dest || (picked ? `${picked.replace(/[\\/]+$/, "")}/RAW` : "");

  const run = async () => {
    setBusy(true);
    try {
      setReport(await api.findRaws(picked, raws, target, loose));
    } catch (e) {
      useStore.getState().notify(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet
      title={t.findRawTitle}
      onClose={onClose}
      footer={
        busy ? (
          <>
            <ProgressBar progress={progress} />
            <button onClick={() => api.cancelTask()}>{t.cancel}</button>
          </>
        ) : (
          <>
            <span className="spacer" />
            <button onClick={onClose}>{report ? t.done : t.cancel}</button>
            {!report && (
              <button className="primary" disabled={!picked || !raws} onClick={run}>
                {t.start}
              </button>
            )}
          </>
        )
      }
    >
      {report ? (
        <Report report={report} dest={target} />
      ) : (
        <div className="form">
          <p className="note lead">{t.findRawIntro}</p>
          <div className="group">
            <FolderRow label={t.pickedFolder} value={picked} onChange={setPicked} />
            <FolderRow label={t.rawFolder} value={raws} onChange={setRaws} />
            <FolderRow label={t.destination} value={target} onChange={setDest} />
            <Row label={t.looseMatch}>
              <Toggle checked={loose} onChange={setLoose} />
            </Row>
          </div>
        </div>
      )}
    </Sheet>
  );
}

export function TransferSheet({ onClose }: { onClose: () => void }) {
  const t = useT();
  const folder = useStore((s) => s.folder)!;
  const { scopes, initial } = useScopes();
  const [info, setInfo] = useState<Awaited<ReturnType<typeof api.transferStart>> | null>(null);
  const [received, setReceived] = useState(0);
  const [error, setError] = useState("");
  const files = useRef(scopes[initial].flatMap((it) => it.paths));

  useEffect(() => {
    let closed = false;
    api
      .transferStart(folder, files.current)
      .then((i) => !closed && setInfo(i))
      .catch((e) => setError(String(e)));
    const off = listen("transfer-received", () => setReceived((n) => n + 1));
    return () => {
      closed = true;
      api.transferStop();
      off.then((f) => f());
    };
  }, [folder]);

  return (
    <Sheet title={t.transferTitle} onClose={onClose} footer={<><span className="spacer" /><button className="primary" onClick={onClose}>{t.done}</button></>}>
      <div className="transfer">
        <p className="note lead">{t.transferIntro}</p>
        {error && <p className="note bad">{error}</p>}
        {info && (
          <>
            <div className="qr" dangerouslySetInnerHTML={{ __html: info.qr }} />
            <div className="transfer-code">
              <small>{t.transferPin}</small>
              <strong>{info.pin}</strong>
              <code>{info.url}</code>
            </div>
            <p className="note">
              {t.transferSharing(info.count)} · {t.transferSameNetwork}
            </p>
            {received > 0 && (
              <p className="note good">
                {t.transferReceived(received)}
                <button className="link" onClick={() => { onClose(); useStore.getState().openFolder(info.inbox); }}>
                  {folderName(info.inbox)}
                </button>
              </p>
            )}
          </>
        )}
      </div>
    </Sheet>
  );
}
