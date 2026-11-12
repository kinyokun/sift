import { useRef } from "react";
import type { AfBox } from "../lib/api";

/** Crop rectangle in normalised image coordinates. */
export type Crop = { x: number; y: number; w: number; h: number };

/** Guide modes that have more than one orientation. */
export const ROTATABLE_GUIDES = [3, 5];
export const GUIDE_MODES = 7;

export const CROP_RATIOS: { id: string; ratio: number | null }[] = [
  { id: "original", ratio: 0 },
  { id: "1:1", ratio: 1 },
  { id: "4:5", ratio: 4 / 5 },
  { id: "3:4", ratio: 3 / 4 },
  { id: "2:3", ratio: 2 / 3 },
  { id: "9:16", ratio: 9 / 16 },
  { id: "3:2", ratio: 3 / 2 },
  { id: "16:9", ratio: 16 / 9 },
  { id: "free", ratio: null },
];

/** Largest centred crop with the given pixel aspect ratio. */
export function centeredCrop(imageW: number, imageH: number, ratio: number | null): Crop {
  if (!ratio) return ratio === null ? { x: 0.08, y: 0.08, w: 0.84, h: 0.84 } : { x: 0, y: 0, w: 1, h: 1 };
  const imageRatio = imageW / imageH;
  const w = ratio >= imageRatio ? 1 : ratio / imageRatio;
  const h = ratio >= imageRatio ? imageRatio / ratio : 1;
  return { x: (1 - w) / 2, y: (1 - h) / 2, w, h };
}

const PHI = (1 + Math.sqrt(5)) / 2;
const MIN_CROP = 0.06;
/** Smallest side of a drawn focus area, in screen pixels. */
const MIN_AF_SIZE = 14;

/** Golden spiral inside a PHI x 1 rectangle, as quarter arcs through shrinking squares. */
function spiralPath(): string {
  let [x, y, w, h] = [0, 0, PHI, 1];
  let d = `M${x} ${y + h}`;
  for (let i = 0; i < 9; i++) {
    const side = i % 2 === 0 ? h : w;
    let end: [number, number];
    switch (i % 4) {
      case 0:
        end = [x + h, y];
        x += h;
        w -= h;
        break;
      case 1:
        end = [x + w, y + w];
        y += w;
        h -= w;
        break;
      case 2:
        end = [x + w - h, y + h];
        w -= h;
        break;
      default:
        end = [x, y + h - w];
        h -= w;
    }
    d += ` A${side} ${side} 0 0 1 ${end[0]} ${end[1]}`;
  }
  return d;
}
const SPIRAL = spiralPath();

function Guides(props: { mode: number; flip: number; x: number; y: number; w: number; h: number }) {
  const { mode, x, y, w, h } = props;
  const line = (x1: number, y1: number, x2: number, y2: number, key: string) => (
    <line key={key} x1={x + x1 * w} y1={y + y1 * h} x2={x + x2 * w} y2={y + y2 * h} />
  );
  let shapes: React.ReactNode = null;
  if (mode === 1 || mode === 2) {
    const a = mode === 1 ? 1 / 3 : 1 - 1 / PHI;
    shapes = [line(a, 0, a, 1, "v1"), line(1 - a, 0, 1 - a, 1, "v2"), line(0, a, 1, a, "h1"), line(0, 1 - a, 1, 1 - a, "h2")];
  } else if (mode === 3) {
    const fx = props.flip === 1 || props.flip === 2;
    const fy = props.flip >= 2;
    const sx = (fx ? -w : w) / PHI;
    const sy = fy ? -h : h;
    shapes = <path d={SPIRAL} transform={`matrix(${sx} 0 0 ${sy} ${fx ? x + w : x} ${fy ? y + h : y})`} />;
  } else if (mode === 4) {
    shapes = [line(0, 0, 1, 1, "d1"), line(1, 0, 0, 1, "d2")];
  } else if (mode === 5) {
    // Golden triangle: one diagonal plus a perpendicular from each other corner.
    const rising = props.flip % 2 === 0;
    const [a, b] = rising ? [[0, h], [w, 0]] : [[0, 0], [w, h]];
    const corners = rising ? [[0, 0], [w, h]] : [[w, 0], [0, h]];
    const len2 = (b[0] - a[0]) ** 2 + (b[1] - a[1]) ** 2;
    const px = (p: number[], key: string, q: number[]) => <line key={key} x1={x + p[0]} y1={y + p[1]} x2={x + q[0]} y2={y + q[1]} />;
    shapes = [
      px(a, "diag", b),
      ...corners.map((c, i) => {
        const t = ((c[0] - a[0]) * (b[0] - a[0]) + (c[1] - a[1]) * (b[1] - a[1])) / len2;
        return px(c, `p${i}`, [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
      }),
    ];
  } else if (mode === 6) {
    shapes = [line(0.5, 0, 0.5, 1, "cv"), line(0, 0.5, 1, 0.5, "ch")];
  }
  return (
    <>
      <g className="guide shadow">{shapes}</g>
      <g className="guide">{shapes}</g>
    </>
  );
}

type Props = {
  /** Size of the photo on screen, in CSS pixels. The overlay is drawn in
   * screen space so lines keep their width at any zoom level. */
  width: number;
  height: number;
  /** Size of the photo in image pixels, for the crop read-out. */
  pixelWidth: number;
  pixelHeight: number;
  af: AfBox[] | null;
  guides: number;
  flip: number;
  crop: Crop | null;
  cropRatio: number | null;
  onCrop: (crop: Crop) => void;
  style?: React.CSSProperties;
};

export function Overlay(props: Props) {
  const { width: W, height: H, crop } = props;
  const drag = useRef<{ kind: string; startX: number; startY: number; base: Crop } | null>(null);

  const begin = (kind: string) => (e: React.PointerEvent) => {
    if (!crop || e.button !== 0) return;
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { kind, startX: e.clientX, startY: e.clientY, base: crop };
  };
  const move = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    e.stopPropagation();
    const dx = (e.clientX - d.startX) / W;
    const dy = (e.clientY - d.startY) / H;
    const b = d.base;
    if (d.kind === "move") {
      props.onCrop({ ...b, x: Math.min(1 - b.w, Math.max(0, b.x + dx)), y: Math.min(1 - b.h, Math.max(0, b.y + dy)) });
      return;
    }
    // Resize from a corner; the opposite corner stays put.
    const left = d.kind.includes("w");
    const top = d.kind.includes("n");
    const ax = left ? b.x + b.w : b.x;
    const ay = top ? b.y + b.h : b.y;
    const px = Math.min(1, Math.max(0, (left ? b.x : b.x + b.w) + dx));
    const py = Math.min(1, Math.max(0, (top ? b.y : b.y + b.h) + dy));
    let w = Math.max(MIN_CROP, Math.abs(px - ax));
    let h = Math.max(MIN_CROP, Math.abs(py - ay));
    const ratio = props.cropRatio === 0 ? W / H : props.cropRatio;
    if (ratio) {
      // Normalised aspect: pixel ratio adjusted for the image's own shape.
      const r = ratio / (W / H);
      const maxW = left ? ax : 1 - ax;
      const maxH = top ? ay : 1 - ay;
      w = Math.min(Math.max(w, h * r), maxW, maxH * r);
      h = w / r;
    }
    props.onCrop({ x: left ? ax - w : ax, y: top ? ay - h : ay, w, h });
  };
  const end = (e: React.PointerEvent) => {
    if (drag.current) e.stopPropagation();
    drag.current = null;
  };

  const region = crop ? { x: crop.x * W, y: crop.y * H, w: crop.w * W, h: crop.h * H } : { x: 0, y: 0, w: W, h: H };
  const handle = 11;
  const corners: [string, number, number][] = crop
    ? [
        ["nw", region.x, region.y],
        ["ne", region.x + region.w, region.y],
        ["sw", region.x, region.y + region.h],
        ["se", region.x + region.w, region.y + region.h],
      ]
    : [];

  return (
    <svg className="overlay" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={props.style}>
      {crop && (
        <path className="crop-mask" fillRule="evenodd" d={`M0 0H${W}V${H}H0Z M${region.x} ${region.y}h${region.w}v${region.h}h${-region.w}Z`} />
      )}
      {props.guides > 0 && <Guides mode={props.guides} flip={props.flip} {...region} />}
      {props.af?.map((b, i) => {
        // Small areas (pinpoint AF, or any area on a zoomed-out photo) are
        // drawn at a minimum size around their centre so they stay visible.
        const w = Math.max(b.w * W, MIN_AF_SIZE);
        const h = Math.max(b.h * H, MIN_AF_SIZE);
        const x = (b.x + b.w / 2) * W - w / 2;
        const y = (b.y + b.h / 2) * H - h / 2;
        return (
          <g key={i} className={`af ${b.focused ? "focused" : ""}`}>
            <rect className="shadow" x={x} y={y} width={w} height={h} rx={2} />
            <rect x={x} y={y} width={w} height={h} rx={2} />
          </g>
        );
      })}
      {crop && (
        <g className="crop" onPointerMove={move} onPointerUp={end} onPointerCancel={end}>
          <rect className="crop-body" x={region.x} y={region.y} width={region.w} height={region.h} onPointerDown={begin("move")} />
          <rect className="crop-frame" x={region.x} y={region.y} width={region.w} height={region.h} />
          {corners.map(([id, cx, cy]) => (
            <rect key={id} className={`crop-handle ${id}`} x={cx - handle} y={cy - handle} width={handle * 2} height={handle * 2} rx={handle / 2.500} onPointerDown={begin(id)} />
          ))}
          <text className="crop-label" x={region.x + 8} y={region.y + 18} fontSize={12}>
            {Math.round(crop.w * props.pixelWidth)} × {Math.round(crop.h * props.pixelHeight)}
          </text>
        </g>
      )}
    </svg>
  );
}
