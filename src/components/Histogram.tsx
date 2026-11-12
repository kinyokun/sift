import { useEffect, useState } from "react";
import { api, type CropBox, type Histogram as Data } from "../lib/api";
import { useT } from "../lib/i18n";

const WIDTH = 256;
const HEIGHT = 100;
/** Share of pixels that must be clipped before the corner marker lights up. */
const CLIP_WARNING = 0.005;

/** A filled curve for one channel; `peak` is the count drawn at full height. */
function area(bins: number[], peak: number): string {
  const y = (n: number) => (HEIGHT - Math.min(1, n / peak) * HEIGHT).toFixed(1);
  return `M0,${HEIGHT} ${bins.map((n, i) => `L${i},${y(n)}`).join(" ")} L${WIDTH - 1},${HEIGHT} Z`;
}

/** Evens out the comb that 8-bit processing (a lookup table, a tone curve)
 * leaves behind: neighbouring levels end up alternately empty and doubled. */
function smooth(bins: number[]): number[] {
  return bins.map((n, i) => (i === 0 || i === bins.length - 1 ? n : (bins[i - 1] + 2 * n + bins[i + 1]) / 4));
}

/** The count drawn at full height. A spike must not flatten everything else:
 * clipped pixels pile up in one bin, at the ends or wherever a look with
 * lifted blacks or lowered whites moves them, so the scale follows the bulk
 * of the bins and a spike simply runs off the top. */
function peakOf(channels: number[][]): number {
  const counts = channels.flatMap((bins) => bins.slice(1, -1)).sort((a, b) => a - b);
  const bulk = counts[Math.floor(counts.length * 0.97)] ?? 0;
  return Math.max(1, Math.min(counts[counts.length - 1] ?? 1, bulk * 1.5));
}

type Props = {
  /** File, look and size as in the viewer's image request; `null` while that image is still rendering. */
  request: { path: string; look: string | null; edge: number } | null;
  crop: CropBox | null;
};

export function Histogram({ request, crop }: Props) {
  const t = useT();
  const [data, setData] = useState<Data | null>(null);
  const key = request ? `${request.path}|${request.look}|${request.edge}|${crop?.join(",")}` : "";

  useEffect(() => {
    if (!request) return;
    let stale = false;
    // Flipping through photos should not queue a histogram for each one.
    const timer = setTimeout(() => {
      api
        .histogram(request.path, request.look, request.edge, crop)
        .then((h) => !stale && setData(h))
        .catch(() => !stale && setData(null));
    }, 120);
    return () => {
      stale = true;
      clearTimeout(timer);
    };
  }, [key]);

  if (!data) return <figure className="histogram" aria-label={t.histogram} />;
  const channels = [data.r, data.g, data.b, data.luma].map(smooth);
  const [r, g, b, luma] = channels;
  const peak = peakOf(channels);
  return (
    <figure className="histogram" aria-label={t.histogram}>
      <svg viewBox={`0 0 ${WIDTH - 1} ${HEIGHT}`} preserveAspectRatio="none">
        <path className="r" d={area(r, peak)} />
        <path className="g" d={area(g, peak)} />
        <path className="b" d={area(b, peak)} />
        <path className="l" d={area(luma, peak)} />
      </svg>
      <i className={`clip low ${data.shadows > CLIP_WARNING ? "on" : ""}`} title={`${t.clippedShadows} ${(data.shadows * 100).toFixed(1)}%`} />
      <i className={`clip high ${data.highlights > CLIP_WARNING ? "on" : ""}`} title={`${t.clippedHighlights} ${(data.highlights * 100).toFixed(1)}%`} />
    </figure>
  );
}
