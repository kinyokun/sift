import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect, useMemo, useRef, useState, type PointerEvent } from "react";
import { api, isRawPath, type CropBox, type CopyReport, type ExportKind, type FrameOptions, type FrameStyle, type Item, type Output, type Progress, type Rename, type TagFolder } from "../lib/api";
import { useT } from "../lib/i18n";
import { cropOf, flagOf, folderName, framesOf, lookOf, tagsOf, turnOf, useStore, visibleEntries } from "../lib/store";
import { TagDot } from "../components/Tags";
import { IconButton, NumberSlider, Row, Segmented, Sheet, Toggle } from "../ui/controls";
import { MarkPanel, markReady, photoArea, pickPicture, savedMarks, toWatermark, usePictures, type MarkPreset } from "./Watermark";

const LAST_DEST_KEY = "sift.exportDest";
const LAST_RENAME_KEY = "sift.exportRename";
const DEFAULT_TEMPLATE = "{date}_{seq}";
/** An export sorted by tag has a rule of its own, remembered apart from the plain one. */
const LAST_TAG_RENAME_KEY = "sift.exportRenameTag";
const DEFAULT_TAG_TEMPLATE = "{tag}_{seq}";
/** What stands for "photos without a tag" among the tags to export. */
const UNTAGGED = "";
const SIZES = [0, 4096, 3000, 2560, 2048, 1600, 1080];
const OUTPUT_KEY = "sift.exportOutput";
/** How pictures come out, remembered between exports, with the watermarks kept and the one chosen; they stay
 * while watermarking is off. */
type Saved = { size: number; quality: number; strip: boolean; marks: MarkPreset[]; markId: string; marked: boolean };

function savedOutput(): Saved {
  let raw: Record<string, unknown> = {};
  try {
    raw = JSON.parse(localStorage.getItem(OUTPUT_KEY) || "{}");
  } catch {}
  const { mark: _, ...rest } = raw;
  return { size: 0, quality: 93, strip: false, marked: false, ...rest, ...savedMarks(raw) } as Saved;
}

type Scope = "picked" | "selection" | "shown";

/** Photos an action applies to: picks first, then a multi-selection, then everything shown. */
export function useScopes(): { scopes: Record<Scope, Item[]>; initial: Scope } {
  const s = useStore();
  return useMemo(() => {
    const cells = visibleEntries(s);
    const shown = cells.flatMap((e) => e.members);
    const picked = s.items.filter((it) => flagOf(s.flags, it) === 1);
    // A folded stack that is selected stands for all of its frames.
    const chosen = framesOf(cells, s.selection);
    const selection = chosen.length > 1 ? chosen : [];
    const initial: Scope = picked.length ? "picked" : selection.length ? "selection" : "shown";
    return { scopes: { picked, selection, shown }, initial };
  }, [s.items, s.flags, s.selection, s.filter, s.minStars, s.ratings, s.stacks, s.expanded, s.settings.stacks, s.settings.sortBy, s.settings.sortDesc, s.tagged, s.tagFilter]);
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
  const [saved, setSaved] = useState(savedOutput);
  // Changes build on the latest settings: a picture chosen in a dialog lands after other changes.
  const keep = (patch: Partial<Saved> | ((s: Saved) => Partial<Saved>)) =>
    setSaved((s) => {
      const next = { ...s, ...(typeof patch === "function" ? patch(s) : patch) };
      localStorage.setItem(OUTPUT_KEY, JSON.stringify(next));
      return next;
    });
  const mark = saved.marks.find((m) => m.id === saved.markId) ?? saved.marks[0];
  const pictures = usePictures(saved.marks.filter((m) => m.kind === "picture").map((m) => m.picture));
  const marked = saved.marked && markReady(mark, pictures);
  const output: Output = { size: saved.size, quality: saved.quality, strip: saved.strip, watermark: marked ? toWatermark(mark) : null };
  const [kind, setKind] = useState<ExportKind>("all");
  const [renaming, setRenaming] = useState(false);
  const [plainTemplate, setPlainTemplate] = useState(() => localStorage.getItem(LAST_RENAME_KEY) || DEFAULT_TEMPLATE);
  const [tagTemplate, setTagTemplate] = useState(() => localStorage.getItem(LAST_TAG_RENAME_KEY) || DEFAULT_TAG_TEMPLATE);
  const tags = useStore((s) => s.tags);
  const tagged = useStore((s) => s.tagged);
  const [byTag, setByTag] = useState(false);
  /** Tags that get no folder. Photos without a tag get one of their own unless it is put here too. */
  const [left, setLeft] = useState<string[]>([]);
  const [template, setTemplate] = byTag ? [tagTemplate, setTagTemplate] : [plainTemplate, setPlainTemplate];
  const [start, setStart] = useState(1);
  const [example, setExample] = useState("");
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<CopyReport | null>(null);
  const [preview, setPreview] = useState<string | null>(null);
  const progress = useProgress();
  const items = scopes[scope];
  const [sampleAt, setSampleAt] = useState(0);
  const sampleItem = items[Math.min(sampleAt, items.length - 1)];
  const sample = sampleItem?.id;
  const rename: Rename | null = renaming && template.trim() ? { template: template.trim(), start: Math.max(0, start) } : null;
  // The photos that have a file of the chosen type, and those files.
  const kept = useMemo(() => (mode === "frame" ? items : items.filter((it) => kind === "all" || it.paths.some((p) => (kind === "raw") === isRawPath(p)))), [items, kind, mode]);
  const shots = useMemo(() => kept.map((it) => (mode === "frame" ? [it.id] : it.paths.filter((p) => kind === "all" || (kind === "raw") === isRawPath(p)))), [kept, kind, mode]);
  // Sorted by tag: the photos of every tag, by their place in `kept`; a photo with two tags is in both.
  const groups = useMemo(() => {
    const of = kept.map((it) => tagsOf(tagged, it));
    const listed = (carries: (ids: string[]) => boolean) => of.flatMap((ids, i) => (carries(ids) ? [i] : []));
    return [
      ...tags.map((tag) => ({ id: tag.id, name: tag.name, color: tag.color as number | null, shots: listed((ids) => ids.includes(tag.id)) })),
      { id: UNTAGGED, name: t.untagged, color: null, shots: listed((ids) => !ids.some((id) => tags.some((tag) => tag.id === id))) },
    ].filter((g) => g.shots.length > 0);
  }, [kept, tags, tagged, t]);
  const folders: TagFolder[] = byTag ? groups.filter((g) => !left.includes(g.id)).map((g) => ({ tag: g.name, shots: g.shots })) : [];
  const untaggedLeft = byTag && left.includes(UNTAGGED) ? (groups.find((g) => g.id === UNTAGGED)?.shots.length ?? 0) : 0;
  const ready = shots.length > 0 && (!byTag || folders.length > 0);
  const hasRaw = items.some((it) => it.paths.some(isRawPath));
  // Looks chosen while culling are applied on export.
  const chosen = useStore((s) => s.looks);
  const cropped = useStore((s) => s.crops);
  const turned = useStore((s) => s.turns);
  const looks: Record<string, string> = {};
  const sources: Record<string, string> = {};
  const crops: Record<string, CropBox> = {};
  const turns: Record<string, number> = {};
  let styled = 0;
  for (const it of items) {
    const look = lookOf(chosen, it);
    const crop = cropOf(cropped, it);
    const turn = turnOf(turned, it);
    if (look || crop || turn) styled++;
    for (const p of it.paths) {
      if (look) looks[p] = look;
      if (crop) crops[p] = crop;
      if (turn) turns[p] = turn;
    }
    if (look) sources[it.id] = it.paths.find(isRawPath) ?? it.id;
  }
  // The copy job gets every file of a shot; a look is rendered from its RAW even when only JPEGs are wanted.
  // The movie of a Live Photo goes with its still.
  const wanted = kept.map((it) => (it.live && kind !== "raw" ? [...it.paths, it.live] : it.paths));

  // Show what the first and last file will be called; sorted by tag, those of the first folder.
  const first = folders[0];
  const named = useMemo(() => (first ? first.shots.map((i) => shots[i]) : shots), [first?.tag, first?.shots.join(), shots]);
  useEffect(() => {
    if (!rename || !named.length) return setExample("");
    let stale = false;
    const rule = first ? { ...rename, tag: first.tag } : rename;
    const ends = named.length > 1 ? [named[0], named[named.length - 1]] : [named[0]];
    const timer = setTimeout(async () => {
      const names = await api.exportNames(ends, rule).catch(() => []);
      if (stale || !names.length) return;
      // The preview numbers two shots; the real last number depends on the count.
      const last = named.length > 1 ? await api.exportNames(named, rule).then((all) => all[all.length - 1]).catch(() => "") : "";
      if (!stale) setExample(last ? `${names[0]}  …  ${last}` : names[0]);
    }, 200);
    return () => {
      stale = true;
      clearTimeout(timer);
    };
  }, [renaming, template, start, named]);

  // Live preview of a photo with its frame or watermark.
  const previewing = mode === "frame" || (saved.marked && kind !== "raw");
  const sampleTurn = sampleItem ? turnOf(turned, sampleItem) : 0;
  const sampleCrop = sampleItem ? cropOf(cropped, sampleItem) : null;
  useEffect(() => {
    if (!previewing || !sample) return;
    let url: string | null = null;
    let stale = false;
    const timer = setTimeout(async () => {
      try {
        const bytes = await api.previewExport(sample, mode === "frame" ? options : null, output, sampleTurn, sampleCrop);
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
  }, [previewing, mode, sample, JSON.stringify(options), JSON.stringify(output.watermark), sampleTurn, JSON.stringify(sampleCrop)]);

  const setMark = (patch: Partial<MarkPreset>, more: Partial<Saved> = {}) => {
    const id = mark.id;
    keep((s) => ({ marks: s.marks.map((m) => (m.id === id ? { ...m, ...patch } : m)), ...more }));
  };
  const chooseMark = async () => {
    try {
      const kept = await pickPicture(t.watermarkPictures);
      if (kept) setMark({ kind: "picture", picture: kept.id, backdrop: kept.backdrop, knockout: kept.backdrop }, { marked: true });
    } catch (e) {
      useStore.getState().notify(String(e));
    }
  };
  const addMark = () => {
    const made = { ...mark, id: Math.random().toString(36).slice(2, 10) };
    keep((s) => ({ marks: [...s.marks, made], markId: made.id }));
  };
  const removeMark = () =>
    keep((s) => {
      const at = s.marks.findIndex((m) => m.id === mark.id);
      const rest = s.marks.filter((m) => m.id !== mark.id);
      return { marks: rest, markId: rest[Math.max(0, at - 1)].id };
    });
  // A click or drag on the preview puts the watermark there.
  const placing = saved.marked && !mark.tile && kind !== "raw";
  const placeAt = (e: PointerEvent<HTMLImageElement>) => {
    if (!placing || (e.type === "pointermove" && !e.buttons)) return;
    const img = e.currentTarget;
    if (e.type === "pointerdown") img.setPointerCapture(e.pointerId);
    const box = img.getBoundingClientRect();
    const [x, y, w, h] = photoArea(mode === "frame" ? options.style : null, img.naturalWidth, img.naturalHeight);
    const share = (v: number) => Math.round(Math.min(1, Math.max(0, v)) * 1000) / 1000;
    setMark({ at: [share(((e.clientX - box.left) / box.width - x) / w), share(((e.clientY - box.top) / box.height - y) / h)] });
  };

  const run = async () => {
    setBusy(true);
    setReport(null);
    try {
      if (!mobile) localStorage.setItem(LAST_DEST_KEY, dest);
      if (rename) localStorage.setItem(byTag ? LAST_TAG_RENAME_KEY : LAST_RENAME_KEY, rename.template);
      const result =
        mode === "copy"
          ? await api.exportFiles(wanted, dest, kind, rename, looks, crops, folders, turns, output)
          : await api.exportFramed(shots.map((shot) => shot[0]), dest, options, output.size, rename, looks, sources, crops, folders, turns, output);
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
      wide={previewing}
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
              <button className="primary" disabled={!ready || !dest} onClick={run}>
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
        <div className={previewing ? "split" : ""}>
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
              {tags.length > 0 && (
                <Row label={t.exportByTag} hint={byTag ? t.exportByTagHint : undefined}>
                  <Toggle
                    checked={byTag}
                    onChange={(on) => {
                      setByTag(on);
                      // Files in a tag's folder are named after the tag unless that is turned off again.
                      if (on) setRenaming(true);
                    }}
                  />
                </Row>
              )}
              {byTag && groups.length > 0 && (
                <div className="row chips tag-folders">
                  {groups.map((g) => (
                    <button
                      key={g.id}
                      className={left.includes(g.id) ? "" : "on"}
                      aria-pressed={!left.includes(g.id)}
                      onClick={() => setLeft(left.includes(g.id) ? left.filter((id) => id !== g.id) : [...left, g.id])}
                    >
                      {g.color !== null && <TagDot color={g.color} />}
                      {g.name}
                      <i>{g.shots.length}</i>
                    </button>
                  ))}
                </div>
              )}
              <Row label={t.renameFiles} hint={renaming ? example && `${t.renameExample}: ${first ? `${first.tag}/` : ""}${example}` : undefined}>
                <Toggle checked={renaming} onChange={setRenaming} />
              </Row>
              {renaming && (
                <div className="row rename">
                  <input className="text" value={template} spellCheck={false} onChange={(e) => setTemplate(e.target.value)} aria-label={t.renameFiles} />
                  <label>
                    {t.renameStart}
                    <input className="text number" type="number" min={0} value={start} onChange={(e) => setStart(Number(e.target.value) || 0)} />
                  </label>
                  <p className="note">
                    {t.renameHint}
                    {byTag && t.renameHintTag}
                  </p>
                </div>
              )}
            </div>
            {mode === "frame" && (
              <div className="group">
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
                <div className="row chips">
                  {fields.map(([key, label]) => (
                    <button key={key} className={options[key] ? "on" : ""} aria-pressed={!!options[key]} onClick={() => set({ [key]: !options[key] })}>
                      {label}
                    </button>
                  ))}
                </div>
                <div className="row">
                  <input className="text" placeholder={t.caption} value={options.caption} onChange={(e) => set({ caption: e.target.value })} aria-label={t.caption} />
                </div>
              </div>
            )}
            {(mode === "frame" || kind !== "raw") && (
              <div className="group">
                <Row label={t.outputSize}>
                  <select value={saved.size} onChange={(e) => keep({ size: Number(e.target.value) })}>
                    {SIZES.map((n) => (
                      <option key={n} value={n}>
                        {n ? `${n} px` : t.sizeOriginal}
                      </option>
                    ))}
                  </select>
                </Row>
                <Row label={t.quality}>
                  <NumberSlider label={t.quality} min={50} max={100} step={1} value={saved.quality} onChange={(quality) => keep({ quality: Math.round(quality) })} />
                </Row>
                <Row label={t.stripMeta} hint={t.stripHint}>
                  <Toggle checked={saved.strip} onChange={(strip) => keep({ strip })} />
                </Row>
                <Row label={t.watermark}>
                  <Toggle
                    checked={saved.marked}
                    onChange={(on) => {
                      keep({ marked: on });
                      if (on && mark.kind === "picture" && !pictures[mark.picture]) void chooseMark();
                    }}
                  />
                </Row>
              </div>
            )}
            {saved.marked && (mode === "frame" || kind !== "raw") && (
              <div className="group watermark">
                <MarkPanel
                  marks={saved.marks}
                  mark={mark}
                  pictures={pictures}
                  onChange={setMark}
                  onChoose={(markId) => keep({ markId })}
                  onAdd={addMark}
                  onRemove={removeMark}
                  onPick={chooseMark}
                />
              </div>
            )}
            {untaggedLeft > 0 && <p className="note">{t.exportUntaggedLeft(untaggedLeft)}</p>}
            {byTag && shots.length > 0 && !folders.length && <p className="note bad">{t.exportNoTagged}</p>}
            {mode === "copy" && kind !== "raw" && (styled > 0 || saved.size > 0 || saved.marked) && <p className="note">{t.rendersNote}</p>}
            {!items.length && <p className="note">{t.nothingToExport}</p>}
            {items.length > 0 && !shots.length && <p className="note bad">{t.nothingOfKind}</p>}
          </div>
          {previewing && (
            <div className="frame-preview">
              {preview && <img src={preview} alt="" draggable={false} className={placing ? "placing" : ""} onPointerDown={placeAt} onPointerMove={placeAt} />}
              {items.length > 1 && (
                <div className="preview-nav">
                  <IconButton icon="back" size={16} label={t.previewPrev} onClick={() => setSampleAt((i) => (Math.min(i, items.length - 1) + items.length - 1) % items.length)} />
                  <span>
                    {Math.min(sampleAt, items.length - 1) + 1} / {items.length}
                  </span>
                  <IconButton icon="forward" size={16} label={t.previewNext} onClick={() => setSampleAt((i) => (Math.min(i, items.length - 1) + 1) % items.length)} />
                </div>
              )}
            </div>
          )}
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
  const files = useRef(scopes[initial].flatMap((it) => (it.live ? [...it.paths, it.live] : it.paths)));

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
