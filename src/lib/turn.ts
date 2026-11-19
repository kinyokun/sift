/** Turning photos by quarter turns, and straightening them by small angles, on screen.
 * Pictures always arrive upright (as the camera says), so what the user set while culling
 * is put on them here, with CSS: nothing is rendered again to turn a photo. */

export type Size = { w: number; h: number };
export type Rect = { x: number; y: number; w: number; h: number };

/** Quarter turns clockwise, 0 to 3, from any whole number. */
export const quarter = (turn: number) => ((Math.round(turn) % 4) + 4) % 4;

/** The size of a picture of `size` once turned. */
export function turnedSize<T extends Size>(size: T, turn: number): T {
  return quarter(turn) % 2 ? { ...size, w: size.h, h: size.w } : size;
}

/** A normalised point of the upright picture, in the turned one. */
export function turnPoint(x: number, y: number, turn: number): [number, number] {
  switch (quarter(turn)) {
    case 1:
      return [1 - y, x];
    case 2:
      return [1 - x, 1 - y];
    case 3:
      return [y, 1 - x];
    default:
      return [x, y];
  }
}

/** A normalised rectangle of the upright picture, in the turned one. */
export function turnRect<T extends Rect>(r: T, turn: number): T {
  switch (quarter(turn)) {
    case 1:
      return { ...r, x: 1 - r.y - r.h, y: r.x, w: r.h, h: r.w };
    case 2:
      return { ...r, x: 1 - r.x - r.w, y: 1 - r.y - r.h };
    case 3:
      return { ...r, x: r.y, y: 1 - r.x - r.w, w: r.h, h: r.w };
    default:
      return r;
  }
}

/** Style for an element of the upright picture's size that fills a box of the turned size,
 * from that box's top left corner. */
export function turnStyle(turn: number, upright: Size): React.CSSProperties {
  const q = quarter(turn);
  const shift = [`0, 0`, `${upright.h}px, 0`, `${upright.w}px, ${upright.h}px`, `0, ${upright.w}px`][q];
  return { position: "absolute", left: 0, top: 0, width: upright.w, height: upright.h, transformOrigin: "0 0", transform: q ? `translate(${shift}) rotate(${q * 90}deg)` : undefined };
}

/** Where a normalised point of a picture of `size` goes when the picture is straightened by
 * `angle` degrees (clockwise) about its centre. */
export function spin(x: number, y: number, angle: number, size: Size): [number, number] {
  if (!angle) return [x, y];
  const a = (angle * Math.PI) / 180;
  const [dx, dy] = [(x - 0.5) * size.w, (y - 0.5) * size.h];
  return [0.5 + (dx * Math.cos(a) - dy * Math.sin(a)) / size.w, 0.5 + (dx * Math.sin(a) + dy * Math.cos(a)) / size.h];
}

/** Whether a normalised crop lies wholly on a picture of `size` straightened by `angle`: a
 * crop never takes in the empty corners the turn leaves. */
export function cropFits(c: Rect, angle: number, size: Size): boolean {
  const e = 1e-6;
  if (c.x < -e || c.y < -e || c.x + c.w > 1 + e || c.y + c.h > 1 + e) return false;
  if (!angle) return true;
  const corners: [number, number][] = [
    [c.x, c.y],
    [c.x + c.w, c.y],
    [c.x, c.y + c.h],
    [c.x + c.w, c.y + c.h],
  ];
  // A corner of the crop is on the picture when turning it back lands inside the picture.
  return corners.every(([x, y]) => {
    const [u, v] = spin(x, y, -angle, size);
    return u > -e && u < 1 + e && v > -e && v < 1 + e;
  });
}

/** The crop `c` made as small as it needs to be about its centre to fit a picture
 * straightened by `angle`; from the middle of the picture when its own centre is too near an edge. */
export function fitCrop<T extends Rect>(c: T, angle: number, size: Size): T {
  if (cropFits(c, angle, size)) return c;
  const around = (cx: number, cy: number, k: number): T => ({ ...c, x: cx - (c.w * k) / 2, y: cy - (c.h * k) / 2, w: c.w * k, h: c.h * k });
  const largest = (cx: number, cy: number) => {
    let [lo, hi] = [0, 1];
    for (let i = 0; i < 24; i++) {
      const mid = (lo + hi) / 2;
      if (cropFits(around(cx, cy, mid), angle, size)) lo = mid;
      else hi = mid;
    }
    return lo;
  };
  const [cx, cy] = [c.x + c.w / 2, c.y + c.h / 2];
  const own = largest(cx, cy);
  const middle = largest(0.5, 0.5);
  return own >= middle * 0.6 ? around(cx, cy, own) : around(0.5, 0.5, middle);
}

/** As far from `from` towards `to` as a crop still fits: a drag stops at the picture's edge. */
export function towards<T extends Rect>(from: T, to: T, angle: number, size: Size): T {
  if (cropFits(to, angle, size)) return to;
  const at = (k: number): T => ({ ...to, x: from.x + (to.x - from.x) * k, y: from.y + (to.y - from.y) * k, w: from.w + (to.w - from.w) * k, h: from.h + (to.h - from.h) * k });
  let [lo, hi] = [0, 1];
  for (let i = 0; i < 20; i++) {
    const mid = (lo + hi) / 2;
    if (cropFits(at(mid), angle, size)) lo = mid;
    else hi = mid;
  }
  return at(lo);
}
