/**
 * glass-ui — reusable liquid glass web UI kit.
 *
 * React components that render native GPU liquid glass surfaces behind the
 * DOM (via the `liquid-glass` native library), the Tauri invoke bridge they
 * speak, and the shared glass design system (SF Pro Display, menu, modal and
 * glass-shape styles).
 *
 * Usage in an app:
 *   1. `import "glass-ui/styles.css"` once at the app root.
 *   2. Register the native commands from `glass-ui`'s Rust crate
 *      (`glass-ui` crate, feature "tauri"): position_menu_glass,
 *      sync_menu_glasses, prewarm_menu_glass, set_ui_hit_rects, log_message.
 *   3. Ship a glass-broker.exe beside the app executable.
 */
export { LiquidGlassSurface, useLiquidGlassSurface } from "./LiquidGlassSurface";
export type { GlassTransition } from "./LiquidGlassSurface";
export { LiquidDropdown } from "./LiquidDropdown";
export type { LiquidDropdownItem } from "./LiquidDropdown";
export { LiquidMenuItems } from "./LiquidMenuItems";
export type { LiquidMenuItem } from "./LiquidMenuItems";
export { Modal } from "./Modal";
export { GlassCard } from "./GlassCard";
export type { GlassShape } from "./GlassCard";
export {
  positionMenuGlass,
  syncMenuGlasses,
  setUiHitRects,
  prewarmMenuGlass,
  logUi,
} from "./bridge";
export type { MenuGlassRect } from "./bridge";
