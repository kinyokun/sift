import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { api, type FrameStyle, type KeptMark, type MarkPlace, type Watermark } from "../lib/api";
import { useT } from "../lib/i18n";
import { Row, Segmented, Toggle } from "../ui/controls";

/** A watermark kept among the export settings; `backdrop` says its picture has a plain background that can go. */
export type MarkPreset = Watermark & { id: string; backdrop?: boolean };

const PLACES: MarkPlace[] = ["nw", "n", "ne", "w", "c", "e", "sw", "s", "se"];
const PICTURE: Watermark = {
  kind: "picture",
  picture: "",
  knockout: false,
  text: "© {year} {artist}",
  weight: 450,
  place: "se",
  at: null,
  margin: 0.03,
  size: 0.2,
  opacity: 0.8,
  angle: 0,
  tile: false,
  gap: 0.12,
  tint: "own",
  shadow: false,
};
const TEXT: Watermark = { ...PICTURE, kind: "text", size: 0.03, opacity: 0.85, tint: "#ffffff", shadow: true };
/** Slider ranges of the size: a picture's width, the letters of text. */
const SIZE = { picture: [0.05, 0.6, 0.01], text: [0.008, 0.12, 0.001] } as const;
const FIELDS = ["{year}", "{date}", "{artist}", "{copyright}", "{camera}", "{lens}", "{params}", "{file}"] as const;

const newId = () => Math.random().toString(36).slice(2, 10);

/** The watermarks kept in the export settings `raw`, and the one chosen. Before there could be several, there
 * was one picture (kept as id "") laid by `raw.mark`; a text watermark is there to start with. */
export function savedMarks(raw: { marks?: unknown; markId?: unknown; mark?: unknown }): { marks: MarkPreset[]; markId: string } {
  const listed = Array.isArray(raw.marks) ? (raw.marks as Partial<MarkPreset>[]).filter((m) => m && typeof m.id === "string") : [];
  let marks: MarkPreset[] = listed.map((m) => ({ ...(m.kind === "text" ? TEXT : PICTURE), ...m }) as MarkPreset);
  if (!marks.length) {
    const old = raw.mark && typeof raw.mark === "object" ? (raw.mark as Partial<Watermark>) : null;
    marks = [{ ...TEXT, id: newId() }];
    if (old) marks.unshift({ ...PICTURE, place: old.place ?? PICTURE.place, size: old.size ?? PICTURE.size, opacity: old.opacity ?? PICTURE.opacity, id: newId() });
  }
  const markId = marks.some((m) => m.id === raw.markId) ? (raw.markId as string) : marks[0].id;
  return { marks, markId };
}

/** What the export is told of a kept watermark. */
export function toWatermark({ id: _, backdrop: __, ...mark }: MarkPreset): Watermark {
  return mark;
}

/** Whether a watermark can be laid: it has text, or a picture that is there. */
export function markReady(mark: MarkPreset, pictures: Record<string, string | null>): boolean {
  return mark.kind === "text" ? mark.text.trim() !== "" : !!pictures[mark.picture];
}

const loaded = new Map<string, Promise<string | null>>();

/** Object URLs of the watermark pictures `ids`, null for one that is not there. */
export function usePictures(ids: string[]): Record<string, string | null> {
  const [urls, setUrls] = useState<Record<string, string | null>>({});
  const key = [...new Set(ids)].sort().join();
  useEffect(() => {
    let stale = false;
    for (const id of key ? key.split(",") : []) {
      if (!loaded.has(id))
        loaded.set(
          id,
          api
            .watermark(id)
            .then((png) => (png.byteLength ? URL.createObjectURL(new Blob([png], { type: "image/png" })) : null))
            .catch(() => null),
        );
      void loaded.get(id)!.then((url) => !stale && setUrls((u) => (u[id] === url ? u : { ...u, [id]: url })));
    }
    return () => void (stale = true);
  }, [key]);
  return urls;
}

/** An SVG drawn by the webview as PNG bytes, 2048 px on its long edge. */
async function svgPng(svg: string): Promise<number[]> {
  const url = URL.createObjectURL(new Blob([svg], { type: "image/svg+xml" }));
  try {
    const img = new Image();
    img.src = url;
    await img.decode();
    let [w, h] = [img.naturalWidth, img.naturalHeight];
    if (!w || !h) {
      const box = /viewBox\s*=\s*["']\s*[-\d.]+[\s,]+[-\d.]+[\s,]+([\d.]+)[\s,]+([\d.]+)/i.exec(svg);
      [w, h] = box ? [Number(box[1]), Number(box[2])] : [1, 1];
    }
    const k = 2048 / Math.max(w, h);
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(w * k));
    canvas.height = Math.max(1, Math.round(h * k));
    canvas.getContext("2d")!.drawImage(img, 0, 0, canvas.width, canvas.height);
    const blob = await new Promise<Blob>((done, fail) => canvas.toBlob((b) => (b ? done(b) : fail("svg")), "image/png"));
    return Array.from(new Uint8Array(await blob.arrayBuffer()));
  } finally {
    URL.revokeObjectURL(url);
  }
}

/** Lets the user choose a picture to keep as a watermark; null when they choose none. */
export async function pickPicture(name: string): Promise<KeptMark | null> {
  const file = await open({ multiple: false, filters: [{ name, extensions: ["png", "jpg", "jpeg", "webp", "svg", "tif", "tiff", "bmp", "gif"] }] });
  if (typeof file !== "string") return null;
  return /\.svg$/i.test(file) ? api.setWatermark(null, await svgPng(await api.readSvg(file))) : api.setWatermark(file);
}

/** Where the photo is on a framed picture `w` × `h`, as [x, y, width, height] shares of it; mirrors the sizes
 * `frame::render` gives the bar and the border around a photo. */
export function photoArea(style: FrameStyle | null, w: number, h: number): [number, number, number, number] {
  if (!style || !w || !h) return [0, 0, 1, 1];
  if (style === "bar") {
    // The bar is 0.085 of the photo's long edge.
    const ph = w * 1.085 >= h ? h - 0.085 * w : h / 1.085;
    return [0, 0, 1, ph / h];
  }
  // Margins of 0.035 and a foot of 0.11 of the long edge.
  let pw = w / 1.07;
  let ph = h - 0.145 * pw;
  if (ph > pw) {
    ph = h / 1.145;
    pw = w - 0.07 * ph;
  }
  const m = 0.035 * Math.max(pw, ph);
  return [m / w, m / h, pw / w, ph / h];
}

function Slider(props: { label: string; value: number; min: number; max: number; step: number; shown: string; onChange: (v: number) => void }) {
  return (
    <Row label={props.label}>
      <span className="slider-value">
        <input className="size-slider" type="range" min={props.min} max={props.max} step={props.step} value={props.value} onChange={(e) => props.onChange(Number(e.target.value))} aria-label={props.label} />
        <output>{props.shown}</output>
      </span>
    </Row>
  );
}

/** The kept watermarks to choose from, and how the chosen one looks and is laid. */
export function MarkPanel(props: {
  marks: MarkPreset[];
  mark: MarkPreset;
  pictures: Record<string, string | null>;
  onChange: (patch: Partial<MarkPreset>) => void;
  onChoose: (id: string) => void;
  onAdd: () => void;
  onRemove: () => void;
  onPick: () => void;
}) {
  const t = useT();
  const { mark, onChange } = props;
  const text = useRef<HTMLTextAreaElement>(null);
  const insert = (field: string) => {
    const el = text.current;
    const [a, b] = el ? [el.selectionStart, el.selectionEnd] : [mark.text.length, mark.text.length];
    onChange({ text: mark.text.slice(0, a) + field + mark.text.slice(b) });
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(a + field.length, a + field.length);
    });
  };
  const fieldName: Record<(typeof FIELDS)[number], string> = {
    "{year}": t.fieldYear,
    "{date}": t.fieldDate,
    "{artist}": t.fieldArtist,
    "{copyright}": t.fieldCopyright,
    "{camera}": t.fieldCamera,
    "{lens}": t.fieldLens,
    "{params}": t.fieldParams,
    "{file}": t.fieldFile,
  };
  const picture = props.pictures[mark.picture];
  const [min, max, step] = SIZE[mark.kind];
  const colours = [
    ...(mark.kind === "picture" ? [{ value: "own", label: t.colorOwn }] : []),
    { value: "#ffffff", label: t.colorWhite },
    { value: "#000000", label: t.colorBlack },
    { value: "auto", label: t.colorAuto },
  ];
  const percent = (v: number) => `${Math.round(v * 100)}%`;
  return (
    <>
      <div className="row mark-presets">
        {props.marks.map((m) => (
          <button key={m.id} className={m.id === mark.id ? "on" : ""} aria-pressed={m.id === mark.id} onClick={() => props.onChoose(m.id)} title={m.kind === "text" ? m.text : t.watermarkPicture}>
            {m.kind === "picture" ? props.pictures[m.picture] ? <img src={props.pictures[m.picture]!} alt="" /> : t.watermarkPicture : m.text.split("\n")[0].trim() || t.watermarkText}
          </button>
        ))}
        <button className="add" onClick={props.onAdd} title={t.watermarkAdd} aria-label={t.watermarkAdd}>
          +
        </button>
        {props.marks.length > 1 && (
          <button className="link remove" onClick={props.onRemove}>
            {t.watermarkRemove}
          </button>
        )}
      </div>
      <div className="row">
        <Segmented
          className="grow"
          value={mark.kind}
          options={[
            { value: "text", label: t.watermarkText },
            { value: "picture", label: t.watermarkPicture },
          ]}
          onChange={(kind) => {
            const start = kind === "text" ? TEXT : PICTURE;
            onChange({ kind, size: start.size, tint: kind === "text" && mark.tint === "own" ? TEXT.tint : mark.tint });
            if (kind === "picture" && !picture) props.onPick();
          }}
        />
      </div>
      {mark.kind === "text" ? (
        <div className="row mark-text">
          <textarea ref={text} className="text" rows={2} value={mark.text} spellCheck={false} placeholder="© {year} {artist}" onChange={(e) => onChange({ text: e.target.value })} aria-label={t.watermarkText} />
          <div className="chips mark-fields">
            <button onClick={() => insert("©")}>©</button>
            {FIELDS.map((f) => (
              <button key={f} onClick={() => insert(f)}>
                {fieldName[f]}
              </button>
            ))}
          </div>
          <small>{t.watermarkFieldsHint}</small>
        </div>
      ) : (
        <Row label={picture ? t.watermarkChange : t.watermarkPick} hint={t.watermarkPictureHint} onClick={props.onPick}>
          <span className="mark-pick">{picture && <img src={picture} alt="" />}</span>
        </Row>
      )}
      {mark.kind === "picture" && mark.backdrop && (
        <Row label={t.watermarkKnockout}>
          <Toggle checked={mark.knockout} onChange={(knockout) => onChange({ knockout })} />
        </Row>
      )}
      {mark.kind === "text" && (
        <Row label={t.watermarkWeight}>
          <Segmented
            value={String(mark.weight)}
            options={[
              { value: "300", label: t.weightLight },
              { value: "450", label: t.weightRegular },
              { value: "700", label: t.weightBold },
            ]}
            onChange={(w) => onChange({ weight: Number(w) })}
          />
        </Row>
      )}
      <Row label={t.watermarkColor}>
        <span className="mark-colours">
          <Segmented value={mark.tint} options={colours} onChange={(tint) => onChange({ tint })} />
          <label className={`swatch ${mark.tint.startsWith("#") && !["#ffffff", "#000000"].includes(mark.tint) ? "on" : ""}`} title={t.colorCustom}>
            <input type="color" value={mark.tint.startsWith("#") ? mark.tint : "#ff3b30"} onChange={(e) => onChange({ tint: e.target.value })} aria-label={t.colorCustom} />
          </label>
        </span>
      </Row>
      <Row label={t.watermarkLayout}>
        <Segmented
          value={mark.tile ? "tile" : "single"}
          options={[
            { value: "single", label: t.layoutSingle },
            { value: "tile", label: t.layoutTile },
          ]}
          onChange={(v) => onChange(v === "tile" ? { tile: true, angle: mark.angle || -30 } : { tile: false })}
        />
      </Row>
      {!mark.tile && (
        <Row label={t.watermarkPlace} hint={t.watermarkPlaceHint}>
          <div className="mark-places" role="radiogroup" aria-label={t.watermarkPlace}>
            {PLACES.map((p) => {
              const on = !mark.at && mark.place === p;
              return <button key={p} role="radio" aria-checked={on} className={on ? "on" : ""} onClick={() => onChange({ place: p, at: null })} />;
            })}
          </div>
        </Row>
      )}
      <Slider label={t.watermarkSize} value={mark.size} min={min} max={max} step={step} shown={percent(mark.size / max)} onChange={(size) => onChange({ size })} />
      <Slider label={t.watermarkOpacity} value={mark.opacity} min={0.1} max={1} step={0.05} shown={percent(mark.opacity)} onChange={(opacity) => onChange({ opacity })} />
      <Slider label={t.watermarkAngle} value={mark.angle} min={-90} max={90} step={1} shown={`${mark.angle}°`} onChange={(angle) => onChange({ angle })} />
      {mark.tile ? (
        <Slider label={t.watermarkGap} value={mark.gap} min={0} max={0.5} step={0.01} shown={percent(mark.gap / 0.5)} onChange={(gap) => onChange({ gap })} />
      ) : (
        !mark.at && <Slider label={t.watermarkMargin} value={mark.margin} min={0} max={0.1} step={0.005} shown={percent(mark.margin / 0.1)} onChange={(margin) => onChange({ margin })} />
      )}
      <Row label={t.watermarkShadow} hint={t.watermarkShadowHint}>
        <Toggle checked={mark.shadow} onChange={(shadow) => onChange({ shadow })} />
      </Row>
    </>
  );
}
