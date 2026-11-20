import type { Meta } from "./api";

/** EXIF orientations that mirror the picture, which turns a clockwise roll the other way. */
const MIRRORED = [2, 4, 5, 7];

/** How far (degrees clockwise, seen from behind the camera) the camera was turned from the
 * nearest upright or sideways hold, which is how its photo is shown: a frame shot in portrait
 * with a roll of 91 degrees is tilted 1. Turning the photo by quarter turns leaves it as it is. */
export function tiltOf(meta: Meta | null | undefined): number | null {
  const roll = meta?.level?.roll;
  if (roll == null) return null;
  const tilt = roll - 90 * Math.round(roll / 90);
  return MIRRORED.includes(meta!.orientation) ? -tilt : tilt;
}
