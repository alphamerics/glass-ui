import { useEffect } from "react";
import type { GlassMaterialOverride } from "./bridge";
import type { GlassTransform, GlassTransformTiming } from "./LiquidGlassSurface";

/** Shared constants for controls whose held state spawns a liquid glass
 *  thumb (GlassSwitch, GlassSlider). */

/** How long a press must be held before it becomes a glass drag (ms). */
export const HOLD_MS = 160;
/** Pointer travel (px) that promotes a press to a drag before the hold. */
export const DRAG_THRESHOLD_PX = 5;
/** The held thumb's scale-up. */
export const GLASS_SCALE = 1.5;
/** The grow/shrink tween timing (the glass and the intent share it). */
export const GLASS_TIMING: GlassTransformTiming = {
  durationMs: 180,
  easing: [0.32, 0.72, 0, 1],
};
/** The release settle's tween timing: deliberately slower than the grow —
 *  the glass glides home and shrinks at a relaxed pace before the plain
 *  thumb takes over. */
export const SETTLE_TIMING: GlassTransformTiming = {
  durationMs: 340,
  easing: [0.32, 0.72, 0, 1],
};
/** How long the release shrink tween runs before the glass is dropped. */
export const SETTLE_MS = 400;
/** The thumb glass's pinned material: unlike every other surface it never
 *  follows the app's configurable material — it keeps this look instead:
 *  a very slight white tint, no blur (a sharp backdrop), a thin rim that
 *  bends hard inward (the shared material's wide band smears a thumb this
 *  small; a strong refraction over a narrow band keeps the warp dramatic
 *  but the curvature legible), an interior lens that zooms the backdrop
 *  under the middle out (~30% smaller), and a somewhat thicker outline
 *  whose fade distance is maxed — a negative value disables the
 *  corner-anchor fade entirely, so the whole rim lights by its own bevel
 *  normal (the auto fade is a quarter of the half-perimeter, under 10px
 *  on a shape this size). */
export const THUMB_MATERIAL: GlassMaterialOverride = {
  tintColor: [1, 1, 1],
  tintStrength: 0.1,
  blurRadius: 0,
  refractionStrength: 4,
  edgeBand: 0.35,
  lensStrength: 0.7,
  outlineFade: -1,
  outlineThickness: 3,
};

/** The two-frame grow wait hook: the grow tween must arrive at the native
 *  side AFTER the surface's first rect push (two frames), or the held spec
 *  would snap the glass to full size instead of tweening the grow. */
export function useTwoFrameGrow(
  dragging: boolean,
  setGrown: (value: boolean) => void,
): void {
  useEffect(() => {
    if (!dragging) {
      setGrown(false);
      return;
    }
    let inner = 0;
    const outer = requestAnimationFrame(() => {
      inner = requestAnimationFrame(() => setGrown(true));
    });
    return () => {
      cancelAnimationFrame(outer);
      cancelAnimationFrame(inner);
    };
  }, [dragging, setGrown]);
}

export type { GlassTransform };
