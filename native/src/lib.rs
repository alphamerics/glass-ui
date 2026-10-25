//! glass-ui — the native half of the glass UI kit.
//!
//! [`MenuGlassManager`] is the generic menu-glass machinery every liquid
//! glass Tauri app needs: one native glass surface per open web-UI menu
//! (keyed by menu id), a warm recycle pool, grouped reveals, the menu scale
//! spring, per-key hit rects and the reconciliation sweep. Everything routes
//! through the reusable `liquid-glass` crate.
//!
//! The core is deliberately Tauri-free (pure Win32 + liquid-glass): command
//! wrappers live behind the `tauri` feature ([`commands`]) so any host app
//! can register them with `tauri::generate_handler!`.
//!
//! Lifecycle in a host app:
//! 1. `tauri::Builder::manage(MenuGlassManager::new())`;
//! 2. register the commands (feature `tauri`);
//! 3. optionally `set_group_emitter` (drives the web UI's `glass-group-ready`
//!    event), `set_params` (app-provided seed params; the shared global
//!    material is always merged in on top) and `set_spring_params`;
//! 4. ship `glass-broker.exe` beside the app executable (the glass pipeline
//!    auto-spawns it).

pub mod manager;

#[cfg(feature = "tauri")]
pub mod commands;

pub use manager::{
    small_surface_params, spring_constants, spring_step, MenuGlassManager,
};
pub use liquid_glass::RevealAnimation;

/// The default menu reveal: a liquid bubble from the bottom center
/// (menus anchored to a bar or trigger at the bottom of the overlay grow
/// upward out of it).
pub const MENU_REVEAL: RevealAnimation = RevealAnimation {
    duration_secs: 0.34,
    wobble: 1.4,
    turbulence_strength: 12.0,
    turbulence_scale: 2.6,
    origin: [0.5, 1.0],
};

/// Reveal that completes on the first frame: used when many glass containers
/// appear together (grouped panels), so they all pop in at once instead of
/// playing staggered wobble animations.
pub const MENU_REVEAL_INSTANT: RevealAnimation = RevealAnimation {
    duration_secs: 0.001,
    wobble: 0.0,
    turbulence_strength: 0.0,
    turbulence_scale: 1.0,
    origin: [0.5, 1.0],
};

/// Fade-style reveal for surfaces whose glass must dissolve in rather than
/// bubble out: the shape is fully open from the first frame (the shader's
/// `glass_fade` uniform) and the reveal progress ramps the glass alpha over
/// the duration. Matched to a 160ms content fade in `glass-ui/styles.css` so
/// glass and content land together.
pub const MENU_GLASS_FADE: RevealAnimation = RevealAnimation {
    duration_secs: 0.16,
    wobble: 0.0,
    turbulence_strength: 0.0,
    turbulence_scale: 1.0,
    origin: [0.5, 1.0],
};
