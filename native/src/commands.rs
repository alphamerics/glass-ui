//! Optional Tauri command surface (`features = ["tauri"]`): the command
//! names the glass-ui web components invoke. Register them with
//! `tauri::generate_handler![...]` and manage [`MenuGlassManager`] as Tauri
//! state.
//!
//! ```ignore
//! use glass_ui::{commands, MenuGlassManager};
//!
//! tauri::Builder::default()
//!     .manage(MenuGlassManager::new())
//!     .invoke_handler(tauri::generate_handler![
//!         commands::set_ui_hit_rects,
//!         commands::prewarm_menu_glass,
//!         commands::position_menu_glass,
//!         commands::animate_menu_glass,
//!         commands::sync_menu_glasses,
//!         commands::log_message,
//!     ])
//! ```

use crate::manager::MenuGlassManager;

#[derive(serde::Deserialize, Debug)]
pub struct MenuGlassRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Replaces the app's interactive-element rects (overlay-relative physical
/// pixels). Only meaningful for shell-style overlays whose host window is
/// click-through except at pushed rects; the manager stores them so the
/// host's hit-test loop can fetch them via [`crate::MenuGlassManager`]
/// (see `ui_hit_rects`).
#[tauri::command]
pub fn set_ui_hit_rects(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, MenuGlassManager>,
    rects: Vec<MenuGlassRect>,
) -> Result<(), String> {
    let overlay = window.hwnd().map_err(|error| error.to_string())?.0 as isize;
    let mut host = windows::Win32::Foundation::RECT::default();
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(
            windows::Win32::Foundation::HWND(overlay as *mut _),
            &mut host,
        );
    }
    state.set_ui_hit_rects(
        rects
            .iter()
            .map(|rect| {
                [
                    host.left + rect.x as i32,
                    host.top + rect.y as i32,
                    host.left + (rect.x + rect.width) as i32,
                    host.top + (rect.y + rect.height) as i32,
                ]
            })
            .collect(),
    );
    Ok(())
}

/// Pre-creates warm menu glass surfaces into the recycle pool so the next
/// grouped panel opens instantly. Called on trigger hover, buying the
/// capture-session warmup time before the click. `local` pre-warms
/// in-app (local-feed) surfaces instead of the global ones.
#[tauri::command]
pub fn prewarm_menu_glass(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, MenuGlassManager>,
    count: usize,
    local: Option<bool>,
) -> Result<(), String> {
    state.prewarm(
        windows::Win32::Foundation::HWND(window.hwnd().map_err(|e| e.to_string())?.0 as *mut _),
        count,
        local.unwrap_or(false),
    )
}

/// Renders (or removes) the native liquid glass surface behind a menu. The
/// rect is in physical pixels relative to the overlay's top-left corner.
/// `local` feeds the surface from the host window's own capture instead of
/// the shared monitor capture (in-app glass).
///
/// `shape_kind`/`shape_exponent` select the surface's own preset: 0 (or
/// absent) is the rounded rect of `rect_radius`, 1 draws the superellipse
/// (squircle) of `shape_exponent`.
///
/// `metaball` turns the surface into a metaball container: the drawn shape
/// becomes the shared metaball field instead of the shape preset.
/// `metaball_balls` carries the WHOLE field (every metaball element's ball,
/// window-relative physical px, each `(x, y, extentX, extentY, weight,
/// exponent)` — exponent 2 with equal extents the plain circle) so metaball
/// containers merge with each other; `metaball_radius` is this element's own
/// ball radius (the anchored element may be a tiny dot) and
/// `metaball_tension` the field's merge exponent (2 = the classic blend).
///
/// `material` pins the surface's glass material to per-surface values (the
/// web UI's `material` prop; see [`crate::GlassMaterialOverride`]).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn position_menu_glass(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, MenuGlassManager>,
    key: String,
    rect: Option<MenuGlassRect>,
    rect_radius: Option<f64>,
    instant: Option<bool>,
    glass_fade: Option<bool>,
    group: Option<String>,
    spring: Option<bool>,
    local: Option<bool>,
    shape_kind: Option<f64>,
    shape_exponent: Option<f64>,
    metaball: Option<bool>,
    metaball_radius: Option<f64>,
    metaball_balls: Option<Vec<[f64; 6]>>,
    metaball_tension: Option<f64>,
    material: Option<crate::GlassMaterialOverride>,
) -> Result<(), String> {
    let rect = rect.map(|r| [r.x as f32, r.y as f32, r.width as f32, r.height as f32]);
    let radius = rect_radius.unwrap_or(20.0) as f32;
    let shape = match shape_kind {
        Some(1.0) => Some(liquid_glass::GlassShape::Superellipse {
            exponent: shape_exponent.unwrap_or(5.0) as f32,
        }),
        _ => None,
    };
    let metaball = metaball.unwrap_or(false).then(|| crate::MetaballPush {
        ball_radius: metaball_radius.unwrap_or(24.0) as f32,
        balls: metaball_balls.unwrap_or_default(),
        tension: metaball_tension.unwrap_or(2.0) as f32,
    });
    state.position_menu_glass(
        windows::Win32::Foundation::HWND(window.hwnd().map_err(|e| e.to_string())?.0 as *mut _),
        &key,
        rect,
        radius,
        instant.unwrap_or(false),
        glass_fade.unwrap_or(false),
        group.as_deref(),
        spring.unwrap_or(false),
        local.unwrap_or(false),
        shape,
        metaball,
        material,
    )
}

/// The webview's reconciliation sweep: reports the menu-glass keys it
/// currently tracks. Any native surface OF THIS WINDOW whose key is
/// missing gets closed — a hard guarantee that a lost close push can never
/// leave a glass visible forever. Scoped to the calling window so two
/// webview windows sharing one manager never despawn each other's glass.
#[tauri::command]
pub fn sync_menu_glasses(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, MenuGlassManager>,
    keys: Vec<String>,
) {
    let host = window.hwnd().map(|hwnd| hwnd.0 as isize).unwrap_or(0);
    state.sync_glasses(&keys, host);
}

/// Animates (or re-states) a menu glass's CSS-style transform: the drawn
/// shape transitions into `animation`'s target through its cubic-bezier
/// curve (see [`crate::GlassTransformAnimation`]). The spec is held per
/// key, so surfaces created for the key afterwards snap to it.
#[tauri::command]
pub fn animate_menu_glass(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, MenuGlassManager>,
    key: String,
    animation: crate::GlassTransformAnimation,
) -> Result<(), String> {
    state.animate_menu_glass(
        windows::Win32::Foundation::HWND(window.hwnd().map_err(|e| e.to_string())?.0 as *mut _),
        &key,
        animation,
    )
}

/// Forwards a message from the web UI to the native log.
#[tauri::command]
pub fn log_message(message: String) {
    eprintln!("[ui] {message}");
}
