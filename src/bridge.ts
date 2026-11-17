/**
 * The Tauri invoke bridge for liquid glass surfaces: the thin command
 * wrappers every glass UI needs (menu glass placement, hit rects, warm-up,
 * the reconciliation sweep). Every wrapper degrades to a no-op outside a
 * Tauri webview (the browser preview), so the components work anywhere.
 */
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { parseEasing, type GlassEasing } from "./easing";

/** A menu glass rect, in physical pixels relative to the overlay's top-left. */
export interface MenuGlassRect {
  x: number;
  y: number;
  width: number;
  height: number;
  radius?: number;
}

/* ---- The client-area offset ----
   The DOM's rects are CLIENT-relative, but the native anchors speak
   WINDOW-relative pixels (GetWindowRect-based). Borderless hosts have no
   inset (0/0), but a FRAMED window's webview sits below its title bar and
   inside its borders — the offset must be added to every pushed rect or
   the glass lands displaced by the window chrome. Fetched once per window
   and refreshed on resize (scale-factor changes resize the chrome); moves
   cannot change it. */
const clientOffset = { x: 0, y: 0 };

/** The client area's current offset within its host window (physical px).
    Live-read by the rect pusher. */
export function getClientOffset(): { x: number; y: number } {
  return clientOffset;
}

async function refreshClientOffset(): Promise<void> {
  try {
    const win = getCurrentWindow();
    const [inner, outer] = await Promise.all([
      win.innerPosition(),
      win.outerPosition(),
    ]);
    clientOffset.x = inner.x - outer.x;
    clientOffset.y = inner.y - outer.y;
  } catch {
    // The browser preview does not expose the native window — keep 0.
  }
}

void refreshClientOffset();
window.addEventListener("resize", () => {
  void refreshClientOffset();
});

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

/** A per-surface glass material override: every set field pins that
 *  appearance value onto the surface instead of the app's shared material
 *  (the surface is material-locked, so the global material watch never
 *  clobbers it). Absent fields keep the shared material's value. The
 *  units match the shared material's — texel-unit fields compensate for
 *  capture downscaling on the native side exactly like the settings UI's
 *  values do. */
export interface GlassMaterialOverride {
  refractionStrength?: number;
  blurRadius?: number;
  blurSamples?: number;
  tintStrength?: number;
  tintColor?: [number, number, number];
  vibrancy?: number;
  vibrancySaturation?: number;
  edgeBand?: number;
  chromaticOffset?: number;
  chromaticBlend?: number;
  outlineStrength?: number;
  outlineThickness?: number;
  outlineAngle?: number;
  outlineMirror?: number;
  outlineFade?: number;
  outlineFalloff?: number;
  borderThickness?: number;
  borderOpacity?: number;
  rimShadow?: number;
  /** Interior lens refraction: the middle samples radially outward from
   *  the shape's center, so the backdrop underneath reads zoomed out /
   *  smaller. Fractional field expansion (0.4 = the interior samples a
   *  40% wider backdrop field), eased out across the edge band (0
   *  disables it). */
  lensStrength?: number;
}

/**
 * Renders (or removes, with `rect = null`) the native liquid glass surface
 * behind a menu, keyed by `key`. The rect is in physical pixels relative to
 * the overlay's top-left corner. `local` feeds the surface from the host
 * window's own capture instead of the shared monitor capture: glass that
 * only ever sits over the app's own content (an in-app card, control or
 * panel) needs no screen duplication at all. `shape` selects the surface's
 * own preset beyond the rounded rect. `metaball` renders the surface as a
 * metaball container instead of the shape preset. `material` pins the
 * surface's glass material to custom values (see [`GlassMaterialOverride`]).
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
  material?: GlassMaterialOverride,
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
      material: material ?? null,
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
 * next panel opens instantly. Call while the user is hovering the trigger,
 * buying the capture-session warmup time before the click. `local` pre-warms
 * in-app (local-feed) surfaces instead of the global ones — a pool entry is
 * only reused by pushes of the SAME feed kind, so pre-warm the kind the
 * surfaces will actually use.
 */
export async function prewarmMenuGlass(
  count: number,
  local?: boolean,
): Promise<void> {
  try {
    await invoke("prewarm_menu_glass", { count, local: local ?? false });
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
