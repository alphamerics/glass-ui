/**
 * The Tauri invoke bridge for liquid glass surfaces: the thin command
 * wrappers every glass UI needs (menu glass placement, hit rects, warm-up,
 * the reconciliation sweep). Every wrapper degrades to a no-op outside a
 * Tauri webview (the browser preview), so the components work anywhere.
 */
import { invoke } from "@tauri-apps/api/core";
import { parseEasing, type GlassEasing } from "./easing";

/** A menu glass rect, in physical pixels relative to the overlay's top-left. */
export interface MenuGlassRect {
  x: number;
  y: number;
  width: number;
  height: number;
  radius?: number;
}

/** Replaces the app's interactive-element rects (overlay-relative physical
 * pixels) for native hit testing. Only meaningful for shell overlays whose
 * host window is click-through except at pushed rects. */
export async function setUiHitRects(rects: MenuGlassRect[]): Promise<void> {
  try {
    await invoke("set_ui_hit_rects", { rects });
  } catch {
    // The browser preview does not expose native hit testing.
  }
}

/** The surface's own shape preset, mirrored from the engine's `GlassShape`:
 *  "rounded" draws the corner-radius rounded rect (the default), and
 *  "superellipse" draws the squircle of `exponent` (2 = ellipse, 4..6 the
 *  iOS-style squircle, higher approaches a box). Pill/circle looks need no
 *  wire shape — they are rounded rects with a full-height corner radius. */
export interface GlassShapeSpec {
  kind: "rounded" | "superellipse";
  exponent?: number;
}

/** A metaball element's field payload: the element's own ball radius plus
 *  the WHOLE shared field (every metaball element's ball, window-relative
 *  physical px, each `[x, y, extentX, extentY, weight, exponent]` — the
 *  ball's superellipse half-extents and shape exponent, exponent 2 with
 *  equal extents the plain circle), so metaball surfaces merge with each
 *  other. */
export interface MetaballField {
  radius: number;
  balls: Array<[number, number, number, number, number, number]>;
  tension: number;
}

/**
 * Renders (or removes, with `rect = null`) the native liquid glass surface
 * behind a menu, keyed by `key`. The rect is in physical pixels relative to
 * the overlay's top-left corner. `local` feeds the surface from the host
 * window's own capture instead of the shared monitor capture: glass that
 * only ever sits over the app's own content (an in-app card, control or
 * panel) needs no screen duplication at all. `shape` selects the surface's
 * own preset beyond the rounded rect. `metaball` renders the surface as a
 * metaball container instead of the shape preset.
 */
export async function positionMenuGlass(
  key: string,
  rect: MenuGlassRect | null,
  instant?: boolean,
  group?: string,
  glassFade?: boolean,
  spring?: boolean,
  local?: boolean,
  metaball?: MetaballField,
  shape?: GlassShapeSpec,
): Promise<void> {
  try {
    await invoke("position_menu_glass", {
      key,
      rect: rect
        ? { x: rect.x, y: rect.y, width: rect.width, height: rect.height }
        : null,
      rectRadius: rect?.radius ?? null,
      instant: instant ?? false,
      group: group ?? null,
      glassFade: glassFade ?? false,
      spring: spring ?? false,
      local: local ?? false,
      shapeKind: shape?.kind === "superellipse" ? 1 : null,
      shapeExponent:
        shape?.kind === "superellipse" ? (shape.exponent ?? 5) : null,
      metaball: metaball != null,
      metaballRadius: metaball?.radius ?? null,
      metaballBalls: metaball?.balls ?? null,
      metaballTension: metaball?.tension ?? null,
    });
  } catch {
    // The browser preview does not expose native glass.
  }
}

/** Forwards a message to the native side's log (diagnostics). */
export async function logUi(message: string): Promise<void> {
  try {
    await invoke("log_message", { message });
  } catch {
    // Ignore logging failures.
  }
}

/**
 * Pre-creates warm liquid glass surfaces in the native recycle pool, so the
 * next grouped panel opens instantly. Call while the user is hovering the
 * trigger, buying the capture-session warmup time before the click.
 */
export async function prewarmMenuGlass(count: number): Promise<void> {
  try {
    await invoke("prewarm_menu_glass", { count });
  } catch {
    // The browser preview does not expose native glass.
  }
}

/**
 * Reconciliation sweep: reports every menu-glass key the webview currently
 * tracks. The native side closes any surface whose key is missing, so a
 * lost close push can never leave a glass visible forever.
 */
export async function syncMenuGlasses(keys: string[]): Promise<void> {
  try {
    await invoke("sync_menu_glasses", { keys });
  } catch {
    // The browser preview does not expose native glass.
  }
}

/** The wire form of a CSS-style glass transform animation: `scale` factors,
 *  `translate` in PHYSICAL pixels (center-anchored, like CSS's default
 *  transform origin), animated over `durationMs` after `delayMs` through
 *  the cubic-bezier `easing`. See [`animateMenuGlass`]. */
export interface GlassTransformAnimationSpec {
  durationMs?: number;
  delayMs?: number;
  easing?: GlassEasing;
  scale: [number, number];
  translate: [number, number];
}

/**
 * Animates a menu glass's transform with CSS semantics: the drawn shape
 * transitions from wherever it visually is into the spec's target through
 * the spec's bezier curve (one IPC call — the compositor integrates the
 * tween at the render loop's rate). The spec is HELD per key: surfaces
 * created for the key afterwards snap to it, and re-stating an identical
 * target never restarts a running animation. No-ops outside a Tauri
 * webview (the browser preview).
 */
export async function animateMenuGlass(
  key: string,
  spec: GlassTransformAnimationSpec,
): Promise<void> {
  try {
    await invoke("animate_menu_glass", {
      key,
      animation: {
        durationMs: spec.durationMs ?? 240,
        delayMs: spec.delayMs ?? 0,
        easing: parseEasing(spec.easing),
        scale: spec.scale,
        translate: spec.translate,
      },
    });
  } catch {
    // The browser preview does not expose native glass.
  }
}
