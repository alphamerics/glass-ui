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
/// capture-session warmup time before the click.
#[tauri::command]
pub fn prewarm_menu_glass(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, MenuGlassManager>,
    count: usize,
) -> Result<(), String> {
    state.prewarm(
        windows::Win32::Foundation::HWND(window.hwnd().map_err(|e| e.to_string())?.0 as *mut _),
        count,
    )
}

/// Renders (or removes) the native liquid glass surface behind a menu. The
/// rect is in physical pixels relative to the overlay's top-left corner.
#[tauri::command]
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
) -> Result<(), String> {
    let rect = rect.map(|r| [r.x as f32, r.y as f32, r.width as f32, r.height as f32]);
    let radius = rect_radius.unwrap_or(20.0) as f32;
    state.position_menu_glass(
        windows::Win32::Foundation::HWND(window.hwnd().map_err(|e| e.to_string())?.0 as *mut _),
        &key,
        rect,
        radius,
        instant.unwrap_or(false),
        glass_fade.unwrap_or(false),
        group.as_deref(),
        spring.unwrap_or(false),
    )
}

/// The webview's reconciliation sweep: reports the menu-glass keys it
/// currently tracks. Any native surface whose key is missing gets closed —
/// a hard guarantee that a lost close push can never leave a glass visible
/// forever.
#[tauri::command]
pub fn sync_menu_glasses(state: tauri::State<'_, MenuGlassManager>, keys: Vec<String>) {
    state.sync_glasses(&keys);
}

/// Forwards a message from the web UI to the native log.
#[tauri::command]
pub fn log_message(message: String) {
    eprintln!("[ui] {message}");
}
