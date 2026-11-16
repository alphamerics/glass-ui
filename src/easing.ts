/**
 * CSS easing for glass transforms: the same timing-function vocabulary the
 * DOM side speaks, parsed once and shipped to the native compositor as
 * cubic-bezier control points. The native glass and the wrapper element's
 * CSS transition use the IDENTICAL curve, so both layers move as one.
 */

/** The CSS `transition-timing-function` keywords glass transforms accept,
 *  or explicit `cubic-bezier(x1, y1, x2, y2)` control points (as a string
 *  or a 4-tuple). The y coordinates may leave [0, 1] for overshoot
 *  (back-out) curves; the x coordinates must stay in [0, 1] (CSS's own
 *  validation). */
export type GlassEasing =
  | "linear"
  | "ease"
  | "ease-in"
  | "ease-out"
  | "ease-in-out"
  | [number, number, number, number]
  | string;

/** The keyword presets, exactly as CSS defines them. */
export const GLASS_EASING_PRESETS: Record<
  string,
  [number, number, number, number]
> = {
  linear: [0, 0, 1, 1],
  ease: [0.25, 0.1, 0.25, 1],
  "ease-in": [0.42, 0, 1, 1],
  "ease-out": [0, 0, 0.58, 1],
  "ease-in-out": [0.42, 0, 0.58, 1],
};

const CUBIC_BEZIER_PATTERN =
  /^cubic-bezier\(\s*(-?[\d.]+(?:e-?\d+)?)\s*,\s*(-?[\d.]+(?:e-?\d+)?)\s*,\s*(-?[\d.]+(?:e-?\d+)?)\s*,\s*(-?[\d.]+(?:e-?\d+)?)\s*\)$/i;

let warnedInvalid = false;

/**
 * Parses a [`GlassEasing`] into the wire control points. Invalid specs
 * warn once and fall back to `ease` (CSS drops an invalid timing function
 * rather than failing the declaration).
 */
export function parseEasing(
  spec: GlassEasing | undefined,
): [number, number, number, number] {
  const fallback = GLASS_EASING_PRESETS.ease;
  try {
    if (spec == null) {
      return fallback;
    }
    if (Array.isArray(spec)) {
      if (spec.length === 4 && spec.every(Number.isFinite)) {
        // CSS validates the x coordinates into [0, 1]; y is free (overshoot).
        return [
          clampUnit(spec[0]),
          spec[1],
          clampUnit(spec[2]),
          spec[3],
        ] as [number, number, number, number];
      }
      throw new Error("expected [x1, y1, x2, y2]");
    }
    const keyword = spec.trim().toLowerCase();
    if (keyword in GLASS_EASING_PRESETS) {
      return GLASS_EASING_PRESETS[keyword];
    }
    const match = CUBIC_BEZIER_PATTERN.exec(keyword);
    if (match) {
      const points = match.slice(1).map(Number);
      if (points.every(Number.isFinite)) {
        return [
          clampUnit(points[0]),
          points[1],
          clampUnit(points[2]),
          points[3],
        ] as [number, number, number, number];
      }
    }
    throw new Error("not a keyword or a cubic-bezier(...) string");
  } catch (error) {
    if (!warnedInvalid) {
      warnedInvalid = true;
      console.warn(
        `glass-ui: invalid easing ${JSON.stringify(spec)} (${error}); falling back to "ease"`,
      );
    }
    return fallback;
  }
}

/** The control points back as a CSS timing function string, for the DOM
 *  transition that rides along with the native glass. */
export function easingCss(points: [number, number, number, number]): string {
  const [x1, y1, x2, y2] = points;
  return `cubic-bezier(${x1}, ${y1}, ${x2}, ${y2})`;
}

function clampUnit(value: number): number {
  return Math.min(1, Math.max(0, value));
}
