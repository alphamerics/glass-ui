/**
 * The Tauri invoke bridge for liquid glass surfaces: the thin command
 * wrappers every glass UI needs (menu glass placement, hit rects, warm-up,
 * the reconciliation sweep). Every wrapper degrades to a no-op outside a
 * Tauri webview (the browser preview), so the components work anywhere.
 */
import { invoke } from "@tauri-apps/api/core";

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

/**
 * Renders (or removes, with `rect = null`) the native liquid glass surface
 * behind a menu, keyed by `key`. The rect is in physical pixels relative to
 * the overlay's top-left corner.
 */
export async function positionMenuGlass(
  key: string,
  rect: MenuGlassRect | null,
  instant?: boolean,
  group?: string,
  glassFade?: boolean,
  spring?: boolean,
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
