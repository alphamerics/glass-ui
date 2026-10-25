//! The menu-glass manager: one native liquid glass surface per open web-UI
//! menu, keyed by the menu's id, plus the warm recycle pool, grouped
//! reveals, the menu scale spring, per-key hit rects and the reconciliation
//! sweep.
//!
//! Everything here is app-agnostic: it needs only the overlay HWND (per
//! call), the seed shader params (app-provided; the shared global material
//! is merged on top at spawn time) and, optionally, a group-ready emitter.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use liquid_glass::{
    translate_compositor, translate_surface_window, GlassConfig, GlassSurface,
    SharedCompositor, SurfaceAnchor,
};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

use crate::{MENU_GLASS_FADE, MENU_REVEAL, MENU_REVEAL_INSTANT};

/// One native liquid glass surface per open menu, keyed by the menu's id.
/// An instance lives exactly as long as its menu is open: closing the menu
/// parks the surface in the recycle pool, so menus never fight over a shared
/// surface while warm capture sessions are reused.
struct MenuGlass {
    surface: GlassSurface,
    rect: [f32; 4],
    radius: f32,
    /// Small grouped surfaces tone the shader's height-proportional edge
    /// band and outline down — see [`small_surface_params`].
    small: bool,
    /// Fade-style reveal (`glass_fade` = 1 in the surface's params); tracked
    /// so surfaces recycled between the two styles get re-seeded.
    fade: bool,
}

/// The shader's edge band and outline are proportional to the surface
/// height: a wide short pill reads as a thin refractive rim, but small
/// surfaces would spend their whole height (or, for a tall list card,
/// dozens of pixels) inside the band — visually "too strong" and faded.
/// Small surfaces instead get a fixed absolute rim, matching the pill's
/// look at any size.
const SMALL_OUTLINE_SCALE: f32 = 0.55;
/// The absolute refracting rim width for small surfaces, in physical pixels.
const SMALL_EDGE_BAND_PX: f32 = 12.0;

pub fn small_surface_params(
    params: liquid_glass::GlassShaderParams,
    surface_height_px: f32,
) -> liquid_glass::GlassShaderParams {
    let mut scaled = params;
    scaled.outline_strength *= SMALL_OUTLINE_SCALE;
    scaled.outline_thickness_px *= SMALL_OUTLINE_SCALE;
    scaled.border_thickness_px *= SMALL_OUTLINE_SCALE;
    scaled.edge_band = (SMALL_EDGE_BAND_PX / surface_height_px.max(1.0)).clamp(0.0, 0.35);
    scaled
}

/// Applies a surface's seeded material and pins it against the global
/// material watch when the surface derives its own scaled variant: small
/// surfaces scale the rim/border down by size, and a global material push
/// would flatten that derivation (the app re-applies the material itself,
/// with [`MenuGlassManager::seed_params`], on every settings change).
fn seed_surface(
    surface: &mut liquid_glass::GlassSurface,
    params: liquid_glass::GlassShaderParams,
    small: bool,
) {
    let _ = surface.set_shader_params_keeping_corner(params);
    surface.set_material_locked(small);
}

/// See [`MenuGlassManager::pending_group`].
struct PendingGroup {
    /// Keys whose surfaces are still waiting to be revealed.
    keys: HashSet<String>,
    /// The watcher stops waiting past this point and reveals whatever is
    /// ready (a surface whose capture session is very slow should not block
    /// the group forever).
    deadline: std::time::Instant,
}

/// The live squash/stretch state of the menu scale spring. The scale is
/// anchored to the surface's bottom-center in the shader (x overshoots with
/// the width change, y counter-squashes).
#[derive(Default)]
struct MenuSpring {
    scale: (f32, f32),
    velocity: (f32, f32),
    active: bool,
    generation: u64,
}

/// Spring integration constants shared by the native glass spring (this
/// module) and any DOM content spring so both move as one:
/// ω = 2π/duration (duration reads as one full oscillation period), damping
/// ratio ζ = 1 − bounce·0.75 (bounce 1 = very bouncy, 0 = no overshoot).
pub fn spring_constants(bounce: f64, duration_ms: f64) -> (f64, f64) {
    let omega = std::f64::consts::TAU / (duration_ms.max(60.0) / 1000.0);
    let zeta = (1.0 - bounce.clamp(0.0, 1.0) * 0.75).clamp(0.15, 1.0);
    (omega, zeta)
}

/// Integrates one spring axis toward 1 with a hard ceiling at 1.0: the glass
/// may compress freely (values below 1) but never stretches beyond its
/// surface, so the animation can never clip at the window edges — reaching
/// 1 reflects the velocity with `restitution` instead, which reads as the
/// glass squashing into place and bouncing back.
pub fn spring_step(
    value: f32,
    velocity: f32,
    omega: f64,
    zeta: f64,
    restitution: f32,
    dt: f32,
) -> (f32, f32) {
    let acceleration =
        (omega * omega) * (1.0 - value as f64) - (2.0 * zeta * omega) * velocity as f64;
    let mut velocity = velocity + (acceleration as f32) * dt;
    let mut value = value + velocity * dt;
    if value > 1.0 {
        value = 1.0;
        velocity = -velocity * restitution;
        // Micro-bounce deadzone: reflecting a tiny velocity at the ceiling
        // produces sub-pixel vibration (visible jitter). Below the threshold
        // the axis simply settles at 1.
        if velocity.abs() < 0.08 {
            velocity = 0.0;
        }
    }
    (value, velocity)
}

type GroupEmitter = Box<dyn Fn() + Send + Sync>;

/// The maximum number of warm surfaces parked in the recycle pool.
const RECYCLE_POOL_MAX: usize = 8;

/// The menu-glass machinery. Interior-mutated through mutexes, so it can be
/// shared as Tauri state directly.
pub struct MenuGlassManager {
    /// One surface per open menu key.
    glasses: Arc<Mutex<HashMap<String, MenuGlass>>>,
    /// Collapsed, warm surfaces kept alive between opens: re-anchoring a
    /// warm surface is instant, while a fresh WGC session's first frame
    /// takes hundreds of milliseconds to arrive.
    recycled: Mutex<Vec<MenuGlass>>,
    /// Per-key hit rects in screen coordinates (open menus only), for the
    /// host app's hit-test loop.
    hit_rects: Arc<Mutex<HashMap<String, [i32; 4]>>>,
    /// The app's interactive-element rects (buttons etc.), pushed by the
    /// web UI as overlay-relative physical rects via the
    /// `set_ui_hit_rects` command. The hit-rects API merges these with the
    /// open menus' so a click-through shell overlay can use one merged
    /// list; apps without a click-through overlay simply never push any.
    ui_hit_rects: Mutex<Vec<[i32; 4]>>,
    /// The seed shader params every surface is created with (app-provided;
    /// the shared global material is merged on top — see [`seed_params`]).
    params: Mutex<liquid_glass::GlassShaderParams>,
    /// Resize-spring parameters (squash, bounce, duration_ms).
    spring_anim: Mutex<(f64, f64, f64)>,
    /// A group of surfaces being created together (e.g. a panel's tiles).
    /// Their reveals are held back until every member has presented its
    /// first frame, so the whole group appears at once.
    pending_group: Mutex<Option<PendingGroup>>,
    /// Emits the web UI's `glass-group-ready` signal (set by the host app).
    emitter: Mutex<Option<GroupEmitter>>,
    /// The live menu scale spring.
    spring: Arc<Mutex<MenuSpring>>,
}

unsafe impl Send for MenuGlassManager {}
unsafe impl Sync for MenuGlassManager {}

impl Default for MenuGlassManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MenuGlassManager {
    pub fn new() -> Self {
        Self {
            glasses: Arc::new(Mutex::new(HashMap::new())),
            recycled: Mutex::new(Vec::new()),
            hit_rects: Arc::new(Mutex::new(HashMap::new())),
            ui_hit_rects: Mutex::new(Vec::new()),
            params: Mutex::new(liquid_glass::GlassShaderParams::default()),
            spring_anim: Mutex::new((0.4, 0.5, 320.0)),
            pending_group: Mutex::new(None),
            emitter: Mutex::new(None),
            spring: Arc::new(Mutex::new(MenuSpring::default())),
        }
    }

    /// Sets the callback invoked when a pending group is revealed (the host
    /// app forwards this to the web UI, e.g. Tauri's
    /// `app.emit("glass-group-ready", ())`).
    pub fn set_group_emitter(&self, emit: impl Fn() + Send + Sync + 'static) {
        *self.emitter.lock().expect("emitter lock poisoned") = Some(Box::new(emit));
    }

    /// Caches the resize-spring parameters (applied from settings by the
    /// host app on startup and on every settings change).
    pub fn set_spring_params(&self, squash: f64, bounce: f64, duration_ms: f64) {
        *self.spring_anim.lock().expect("spring anim lock poisoned") =
            (squash, bounce, duration_ms);
    }

    fn spring_anim_params(&self) -> (f64, f64, f64) {
        *self.spring_anim.lock().expect("spring anim lock poisoned")
    }

    /// Replaces the seed shader params; every surface created afterwards
    /// (menus, pooled) seeds from these. Existing surfaces are re-seeded
    /// (each keeps its own fade/small derivation).
    pub fn set_params(&self, params: liquid_glass::GlassShaderParams) {
        *self.params.lock().expect("params lock poisoned") = params;
        let params = params;
        for glass in self
            .glasses
            .lock()
            .expect("menu glasses lock poisoned")
            .values_mut()
        {
            // Keep each surface's fade style; the shared settings must not
            // clobber the glass_fade flag stamped at creation.
            let mut scaled = if glass.small {
                small_surface_params(params, glass.rect[3])
            } else {
                params
            };
            scaled.glass_fade = if glass.fade { 1.0 } else { 0.0 };
            // A failure on ONE surface must never abort the sweep and leave
            // the rest stale (mismatched glass).
            seed_surface(&mut glass.surface, scaled, glass.small);
        }
        for glass in self
            .recycled
            .lock()
            .expect("recycled glasses lock poisoned")
            .iter_mut()
        {
            let mut scaled = if glass.small {
                small_surface_params(params, glass.rect[3])
            } else {
                params
            };
            scaled.glass_fade = if glass.fade { 1.0 } else { 0.0 };
            seed_surface(&mut glass.surface, scaled, glass.small);
        }
    }

    /// Seeds a surface's material from the manager's params with the
    /// surface's fade style stamped in. Must run BEFORE `set_reveal`, which
    /// stamps its own turbulence/wobble onto the params. The global material
    /// file is merged in first: another glass app may have restyled the
    /// shared material since our last apply, and a freshly spawned surface
    /// must never come up stale.
    fn seed_params(&self, small: bool, height_px: f32, fade: bool) -> liquid_glass::GlassShaderParams {
        let mut params = self
            .params
            .lock()
            .expect("params lock poisoned")
            .clone();
        let material = liquid_glass::broker::read_material();
        material.apply_onto(&mut params, 1.0);
        params.glass_fade = if fade { 1.0 } else { 0.0 };
        if small {
            small_surface_params(params, height_px)
        } else {
            params
        }
    }

    /// Pre-creates collapsed menu glass surfaces directly into the recycle
    /// pool, so the next grouped panel opens from warm capture sessions
    /// instead of paying the create-and-warmup cost while the user is
    /// already looking at the panel. Called when the user merely hovers the
    /// trigger, buying the warmup time before the click.
    pub fn prewarm(&self, overlay: HWND, count: usize) -> Result<(), String> {
        let mut recycled = self
            .recycled
            .lock()
            .expect("recycled glasses lock poisoned");
        let room = RECYCLE_POOL_MAX.saturating_sub(recycled.len());
        for _ in 0..count.min(room) {
            let config = GlassConfig {
                corner_radius_px: 20.0,
                surface_anchor: SurfaceAnchor::Rect {
                    offset_x: 0.0,
                    offset_y: 0.0,
                    width: 1.0,
                    height: 1.0,
                },
                // The app's own persistent surface already styled the host
                // overlay; re-styling it on every spawn would burn
                // SetWindowLong / DwmSetWindowAttribute / SetWindowPos churn
                // per card.
                style_host_window: false,
                ..Default::default()
            };
            match GlassSurface::create(overlay.0 as isize, config) {
                Ok(mut surface) => {
                    let _ = surface.set_shader_params_keeping_corner(
                        *self.params.lock().expect("params lock poisoned"),
                    );
                    surface.set_frame_cap(Some(60.0));
                    surface.set_reveal(false, MENU_REVEAL_INSTANT);
                    // Render one frame to warm the capture session, then
                    // stop: pooled surfaces cost nothing while parked.
                    surface.pause_after_next_present();
                    recycled.push(MenuGlass {
                        surface,
                        rect: [0.0, 0.0, 1.0, 1.0],
                        radius: 20.0,
                        small: false,
                        fade: false,
                    });
                }
                Err(error) => {
                    eprintln!("glass-ui pre-warm failed: {error}");
                    break;
                }
            }
        }
        Ok(())
    }

    /// Registers a freshly created surface in the pending reveal group.
    fn register_group_member(&self, key: &str) {
        let mut pending = self
            .pending_group
            .lock()
            .expect("pending group lock poisoned");
        match pending.as_mut() {
            Some(existing) => {
                existing.keys.insert(key.to_owned());
            }
            None => {
                *pending = Some(PendingGroup {
                    keys: HashSet::from([key.to_owned()]),
                    // A capture session that never warms up must not block
                    // the group forever.
                    deadline: std::time::Instant::now() + std::time::Duration::from_millis(2500),
                });
            }
        }
    }

    /// Drops a closed surface from the pending reveal group.
    fn remove_group_member(&self, key: &str) {
        let mut pending = self
            .pending_group
            .lock()
            .expect("pending group lock poisoned");
        if let Some(group) = pending.as_mut() {
            group.keys.remove(key);
        }
    }

    fn group_member_pending(&self, key: &str) -> bool {
        self.pending_group
            .lock()
            .expect("pending group lock poisoned")
            .as_ref()
            .is_some_and(|group| group.keys.contains(key))
    }

    /// Called each host-app tick: reveals the whole pending group the moment
    /// every member has presented its first frame (or once the deadline
    /// passes), so grouped glass appears in a single instant. Also fires the
    /// group emitter so the web UI can fade its content in with the glass.
    pub fn check_pending_group(&self) {
        let (group_keys, deadline) = {
            let guard = self
                .pending_group
                .lock()
                .expect("pending group lock poisoned");
            match guard.as_ref() {
                Some(group) => (group.keys.clone(), group.deadline),
                None => return,
            }
        };
        let mut all_ready = true;
        {
            let mut glasses = self
                .glasses
                .lock()
                .expect("menu glasses lock poisoned");
            for key in &group_keys {
                let presented = glasses
                    .get_mut(key)
                    // A member that was closed meanwhile does not block.
                    .map(|glass| glass.surface.has_presented())
                    .unwrap_or(true);
                if !presented {
                    all_ready = false;
                    break;
                }
            }
        }
        if !all_ready && std::time::Instant::now() < deadline {
            return;
        }
        {
            let mut glasses = self
                .glasses
                .lock()
                .expect("menu glasses lock poisoned");
            for key in &group_keys {
                if let Some(glass) = glasses.get_mut(key) {
                    // Resume rendering right as the reveal starts.
                    glass.surface.set_paused(false);
                    glass.surface.set_reveal(true, MENU_REVEAL_INSTANT);
                }
            }
        }
        *self
            .pending_group
            .lock()
            .expect("pending group lock poisoned") = None;
        if let Some(emit) = self.emitter.lock().expect("emitter lock poisoned").as_ref() {
            emit();
        }
    }

    /// Renders (or removes) a native liquid glass surface behind one menu,
    /// identified by `key`. The rect is in physical pixels relative to the
    /// overlay's top-left. Each key owns its own surface instance; passing
    /// `None` collapses the surface and parks it in the recycle pool (so the
    /// next menu reuses a warm capture session instead of waiting for a
    /// fresh session's first frame). Repeated pushes re-anchor the existing
    /// surface instead of recreating it, so tracking an element stays cheap.
    /// `instant` skips the reveal animation, so many surfaces appearing
    /// together pop in at once. `glass_fade` goes further: the reveal
    /// animation is disabled entirely and the glass itself dissolves in with
    /// an opacity fade. `spring` plays the scale-up bounce instead of the
    /// wobble reveal.
    pub fn position_menu_glass(
        &self,
        overlay: HWND,
        key: &str,
        rect: Option<[f32; 4]>,
        radius: f32,
        instant: bool,
        glass_fade: bool,
        group: Option<&str>,
        spring: bool,
    ) -> Result<(), String> {
        let reveal = if glass_fade {
            MENU_GLASS_FADE
        } else if instant || spring {
            // Spring menus skip the wobble reveal: the glass appears open
            // and the scale spring plays the bounce.
            MENU_REVEAL_INSTANT
        } else {
            MENU_REVEAL
        };
        // Closing fades nothing back: fade, instant and spring surfaces
        // collapse immediately (their DOM is already gone), only true reveal
        // menus play the reversed bubble.
        let close_reveal = if glass_fade || instant || spring {
            MENU_REVEAL_INSTANT
        } else {
            MENU_REVEAL
        };
        let mut glasses = self
            .glasses
            .lock()
            .expect("menu glasses lock poisoned");
        match rect {
            None => {
                // Unused: collapse this menu's glass and park it for reuse.
                // The collapse animation renders to the end (an immediate
                // pause would freeze the last visible frame on screen), and
                // once fully transparent the surface stops rendering.
                if let Some(mut glass) = glasses.remove(key) {
                    glass.surface.set_reveal(false, close_reveal);
                    glass.surface.collapse_then_pause();
                    let mut recycled = self
                        .recycled
                        .lock()
                        .expect("recycled glasses lock poisoned");
                    if recycled.len() < RECYCLE_POOL_MAX {
                        recycled.push(glass);
                    }
                }
                self.hit_rects
                    .lock()
                    .expect("menu hit rects lock poisoned")
                    .remove(key);
                // Always drop the key from a pending reveal group: the
                // frontend's cleanup pushes carry no group, and a stale key
                // here would let the group reveal resurrect glass for a view
                // that no longer exists.
                self.remove_group_member(key);
                return Ok(());
            }
            Some(rect) => {
                if let Some(glass) = glasses.get_mut(key) {
                    let moved = !glass
                        .rect
                        .iter()
                        .zip(rect.iter())
                        .all(|(a, b)| (a - b).abs() < 2.0);
                    if moved {
                        glass.surface.set_anchor(SurfaceAnchor::Rect {
                            offset_x: rect[0],
                            offset_y: rect[1],
                            width: rect[2],
                            height: rect[3],
                        })?;
                        glass.rect = rect;
                        // The SDF's corner radius is normalized to the
                        // surface height, which may have changed with the
                        // new rect; re-apply it.
                        glass.surface.set_corner_radius(glass.radius)?;
                        Self::update_hit_rect(overlay, &self.hit_rects, key, &rect);
                    }
                    // Radius changes reconfigure the existing GPU surface in
                    // place; tearing down and rebuilding the surface (with a
                    // fresh D3D device and capture session) while the old
                    // one is still animating hangs or crashes the
                    // compositor.
                    if (glass.radius - radius).abs() >= 0.5 {
                        glass.surface.set_corner_radius(radius)?;
                        glass.radius = radius;
                    }
                    // A surface switching between fade styles gets its
                    // material re-seeded (glass_fade lives in the shader
                    // params).
                    if glass.fade != glass_fade {
                        glass.fade = glass_fade;
                        let params = self.seed_params(glass.small, rect[3], glass_fade);
                        seed_surface(&mut glass.surface, params, glass.small);
                    }
                    // While the surface waits for its reveal group it stays
                    // hidden; the group reveal shows it with its siblings.
                    if !self.group_member_pending(key) {
                        glass.surface.set_reveal(true, reveal);
                    }
                    return Ok(());
                }
                // Prefer a warm, collapsed surface from the recycle pool: it
                // re-anchors instantly, while a fresh capture session needs
                // hundreds of milliseconds before its first frame arrives.
                let mut glass = match self
                    .recycled
                    .lock()
                    .expect("recycled glasses lock poisoned")
                    .pop()
                {
                    Some(mut recycled) => {
                        recycled
                            .surface
                            .set_anchor(SurfaceAnchor::Rect {
                                offset_x: rect[0],
                                offset_y: rect[1],
                                width: rect[2],
                                height: rect[3],
                            })
                            .map_err(|error| {
                                eprintln!("glass-ui menu glass re-anchor failed: {error}");
                                error
                            })
                            .ok();
                        // Back from the pool: resume rendering.
                        recycled.surface.set_paused(false);
                        recycled
                    }
                    None => {
                        let config = GlassConfig {
                            corner_radius_px: radius,
                            surface_anchor: SurfaceAnchor::Rect {
                                offset_x: rect[0],
                                offset_y: rect[1],
                                width: rect[2],
                                height: rect[3],
                            },
                            // The app's own persistent surface already
                            // styled the host overlay; menu spawns must not
                            // re-style it.
                            style_host_window: false,
                            ..Default::default()
                        };
                        let mut surface = match GlassSurface::create(overlay.0 as isize, config) {
                            Ok(surface) => surface,
                            Err(error) => {
                                eprintln!("glass-ui menu glass failed: {error}");
                                return Err(error);
                            }
                        };
                        let small = group.is_some();
                        // Seed the fresh surface with the saved glass
                        // settings (plus this surface's fade style) BEFORE
                        // the reveal is configured — set_reveal stamps its
                        // own turbulence/wobble onto the params, and seeding
                        // afterwards would visibly snap the material as the
                        // user's values land.
                        let _ = seed_surface(
                            &mut surface,
                            self.seed_params(small, rect[3], glass_fade),
                            small,
                        );
                        // A blurred backdrop does not need full refresh
                        // rate: cap menu surfaces for content-driven frames
                        // (reveal animations run at tick rate).
                        surface.set_frame_cap(Some(60.0));
                        // Start the reveal immediately with the chosen
                        // animation; the shader defaults would otherwise
                        // play the first frames from the wrong corner with
                        // the wrong turbulence.
                        surface.set_reveal(true, reveal);
                        MenuGlass {
                            surface,
                            rect,
                            radius,
                            small,
                            fade: glass_fade,
                        }
                    }
                };
                // The recycled surface's height changed, so its normalized
                // corner radius must be re-applied either way.
                glass.surface.set_corner_radius(radius)?;
                // A pooled surface may be re-used by a different panel size
                // class or fade style than its previous life; re-seed its
                // material when either changed.
                let small = group.is_some();
                if glass.small != small || glass.fade != glass_fade {
                    glass.small = small;
                    glass.fade = glass_fade;
                    seed_surface(
                        &mut glass.surface,
                        self.seed_params(small, rect[3], glass_fade),
                        small,
                    );
                }
                // Grouped surfaces hold their reveal until every member has
                // presented its first frame (see check_pending_group), so a
                // freshly opened panel's glass all appears at once. They
                // render exactly one (invisible) frame to prove their
                // capture session is warm, then stop until the group reveal.
                if group.is_some() {
                    glass.surface.set_reveal(false, reveal);
                    glass.surface.pause_after_next_present();
                } else {
                    glass.surface.set_reveal(true, reveal);
                }
                glass.rect = rect;
                glass.radius = radius;
                glasses.insert(key.to_owned(), glass);
                Self::update_hit_rect(overlay, &self.hit_rects, key, &rect);
                if group.is_some() {
                    self.register_group_member(key);
                }
                // Spring menus play the scale-up bounce instead of the
                // wobble reveal. The spring may outlive the glass map entry
                // (menu closed mid-animation), hence the Arc'd map.
                if spring && group.is_none() {
                    let (squash, bounce, duration_ms) = self.spring_anim_params();
                    self.trigger_menu_spring(
                        key,
                        rect[2] as f64 * 0.9,
                        rect[2] as f64,
                        squash,
                        bounce,
                        duration_ms,
                    );
                }
            }
        }
        Ok(())
    }

    /// Starts the menu's squash-and-stretch scale spring from `from_w` to
    /// `to_w` physical pixels. The spring may outlive the menu (closed
    /// mid-flight), hence the Arc'd map.
    fn trigger_menu_spring(
        &self,
        key: &str,
        from_w: f64,
        to_w: f64,
        squash: f64,
        bounce: f64,
        duration_ms: f64,
    ) {
        let to = to_w.max(1.0);
        let ratio = (from_w / to).clamp(0.5, 2.0);
        // Velocity-proportional overshoot: the spring starts with a velocity
        // kick proportional to the size change, so a bigger resize rebounds
        // harder off the ceiling clamp (the "squash into place" bounce) or
        // dips proportionally deeper on a shrink.
        let (omega, zeta) = spring_constants(bounce, duration_ms);
        let delta = (ratio - 1.0).abs();
        let (x_initial, vx_initial) = if ratio <= 1.0 {
            (ratio as f32, (delta * omega) as f32)
        } else {
            (1.0_f32, -((delta * omega) as f32))
        };
        // Compressed start on y (never above 1 — above 1 would clip): the
        // vertical squash mirrors the width displacement, scaled by the
        // squash amount.
        let amplitude = (ratio - 1.0).abs().clamp(0.0, 0.35);
        let y_initial = (1.0 - amplitude * (0.5 + 0.5 * squash)) as f32;
        let restitution = (bounce.clamp(0.0, 1.0) * 0.6) as f32;
        let (spring, glasses) = (
            std::sync::Arc::clone(&self.spring),
            std::sync::Arc::clone(&self.glasses),
        );
        let menu_key = key.to_owned();
        let generation = {
            let mut state = spring.lock().expect("menu spring lock poisoned");
            state.generation += 1;
            state.scale = (x_initial, y_initial);
            state.velocity = (vx_initial, 0.0);
            state.active = true;
            state.generation
        };
        std::thread::spawn(move || {
            // Hard cap so a pathologically underdamped spring can't tick
            // forever: four full oscillation periods.
            let max_ticks = ((duration_ms.max(60.0) / 8.0) * 4.0) as u32;
            for _ in 0..max_ticks {
                // Integrate with the REAL inter-tick interval: sleep(8ms)
                // actually sleeps ~15.6ms on Windows (timer granularity),
                // and feeding the claimed 8ms into the integrator while
                // twice the real time passes desyncs the glass from any DOM
                // spring — visible as jitter.
                let tick_start = std::time::Instant::now();
                std::thread::sleep(std::time::Duration::from_millis(8));
                let dt = tick_start.elapsed().as_secs_f32().min(0.05);
                let (scale, settled) = {
                    let mut state = spring.lock().expect("menu spring lock poisoned");
                    if state.generation != generation || !state.active {
                        return;
                    }
                    let (x, vx) = spring_step(
                        state.scale.0,
                        state.velocity.0,
                        omega,
                        zeta,
                        restitution,
                        dt,
                    );
                    let (y, vy) = spring_step(
                        state.scale.1,
                        state.velocity.1,
                        omega,
                        zeta,
                        restitution,
                        dt,
                    );
                    let settled = (x - 1.0).abs() < 0.002
                        && (y - 1.0).abs() < 0.002
                        && vx.abs() < 0.01
                        && vy.abs() < 0.01;
                    if settled {
                        state.scale = (1.0, 1.0);
                        state.velocity = (0.0, 0.0);
                        state.active = false;
                        ((1.0, 1.0), true)
                    } else {
                        state.scale = (x, y);
                        state.velocity = (vx, vy);
                        ((x, y), false)
                    }
                };
                {
                    let mut glasses = glasses.lock().expect("menu glasses lock poisoned");
                    if let Some(glass) = glasses.get_mut(&menu_key) {
                        glass.surface.set_pill_scale_xy(scale.0, scale.1);
                    }
                }
                if settled {
                    return;
                }
            }
            // Cap reached: snap to rest and stop.
            let mut state = spring.lock().expect("menu spring lock poisoned");
            if state.generation == generation {
                state.scale = (1.0, 1.0);
                state.velocity = (0.0, 0.0);
                state.active = false;
            }
            let mut glasses = glasses.lock().expect("menu glasses lock poisoned");
            if let Some(glass) = glasses.get_mut(&menu_key) {
                glass.surface.set_pill_scale_xy(1.0, 1.0);
            }
        });
    }

    fn update_hit_rect(
        overlay: HWND,
        hit_rects: &Mutex<HashMap<String, [i32; 4]>>,
        key: &str,
        rect: &[f32; 4],
    ) {
        let mut host_rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(overlay, &mut host_rect);
        }
        let hit_rect = [
            host_rect.left + rect[0] as i32,
            host_rect.top + rect[1] as i32,
            host_rect.left + (rect[0] + rect[2]) as i32,
            host_rect.top + (rect[1] + rect[3]) as i32,
        ];
        hit_rects
            .lock()
            .expect("menu hit rects lock poisoned")
            .insert(key.to_owned(), hit_rect);
    }

    /// The open panels' hit rects in screen coordinates, WITHOUT the host's
    /// merged interactive-element rects (see [`Self::hit_rects`] for the
    /// merged list). Hosts whose wheel/click policies differ between the
    /// panels and the bar's own interactive elements consume this directly.
    pub fn open_hit_rects(&self) -> Vec<[i32; 4]> {
        self.hit_rects
            .lock()
            .expect("menu hit rects lock poisoned")
            .values()
            .copied()
            .collect()
    }

    /// The open menus' hit rects in screen coordinates, for the host app's
    /// hit-test loop. The host merges its own interactive-element rects
    /// (pushed via `set_ui_hit_rects`) with these.
    pub fn hit_rects(&self) -> Vec<[i32; 4]> {
        let mut rects = self
            .hit_rects
            .lock()
            .expect("menu hit rects lock poisoned")
            .values()
            .copied()
            .collect::<Vec<[i32; 4]>>();
        rects.extend(
            self.ui_hit_rects
                .lock()
                .expect("ui hit rects lock poisoned")
                .iter()
                .copied(),
        );
        rects
    }

    /// Replaces the web UI's interactive-element rects (screen coordinates;
    /// the `set_ui_hit_rects` command's storage). Merged into
    /// [`MenuGlassManager::hit_rects`].
    pub fn set_ui_hit_rects(&self, rects: Vec<[i32; 4]>) {
        *self
            .ui_hit_rects
            .lock()
            .expect("ui hit rects lock poisoned") = rects;
    }

    /// Whether any menu glass is currently open (the host may need to stay
    /// visible/topmost while a panel is shown).
    pub fn has_open_glass(&self) -> bool {
        !self
            .glasses
            .lock()
            .expect("menu glasses lock poisoned")
            .is_empty()
    }

    /// The reconciliation sweep behind the web UI's `sync_menu_glasses`:
    /// closes every live menu glass whose key the webview no longer tracks.
    /// This is the hard guarantee that a lost close push can never leave a
    /// glass visible forever — the webview's active-key registry is the
    /// source of truth.
    pub fn sync_glasses(&self, keys: &[String]) {
        let stale: Vec<String> = {
            let glasses = self
                .glasses
                .lock()
                .expect("menu glasses lock poisoned");
            glasses
                .keys()
                .filter(|key| !keys.contains(key))
                .cloned()
                .collect()
        };
        if stale.is_empty() {
            return;
        }
        for key in stale {
            // The close path: collapse to the end, then park for reuse.
            let closed = {
                let mut glasses = self
                    .glasses
                    .lock()
                    .expect("menu glasses lock poisoned");
                glasses.remove(&key)
            };
            if let Some(mut glass) = closed {
                glass.surface.set_reveal(false, MENU_REVEAL_INSTANT);
                glass.surface.collapse_then_pause();
                let mut recycled = self
                    .recycled
                    .lock()
                    .expect("recycled glasses lock poisoned");
                if recycled.len() < RECYCLE_POOL_MAX {
                    recycled.push(glass);
                }
            }
            self.hit_rects
                .lock()
                .expect("menu hit rects lock poisoned")
                .remove(&key);
            self.remove_group_member(&key);
        }
    }

    /// Every surface's window handle and compositor (menus + recycled
    /// pool). The caller must release the returned data before performing
    /// window operations that pump the surface windows' threads.
    pub fn surface_targets(&self) -> Vec<(isize, SharedCompositor)> {
        let mut targets: Vec<(isize, SharedCompositor)> = Vec::new();
        for glass in self
            .glasses
            .lock()
            .expect("menu glasses lock poisoned")
            .values()
        {
            targets.push((glass.surface.hwnd_addr(), glass.surface.compositor()));
        }
        for glass in self
            .recycled
            .lock()
            .expect("recycled glasses lock poisoned")
            .iter()
        {
            targets.push((glass.surface.hwnd_addr(), glass.surface.compositor()));
        }
        targets
    }

    /// Shifts every menu surface vertically (the host app's auto-hide slide
    /// or window move). All anchors are overlay-relative, so translating the
    /// overlay and every surface by the same amount keeps the geometry
    /// intact.
    pub fn translate_surfaces(&self, dy: i32) {
        if dy == 0 {
            return;
        }
        for (hwnd, compositor) in self.surface_targets() {
            let _ = translate_surface_window(hwnd, dy);
            translate_compositor(&compositor, dy);
        }
    }

    /// Re-syncs every menu surface's bounds after the overlay moved or
    /// resized.
    pub fn sync_bounds(&self, overlay: HWND) {
        if let Ok(mut glasses) = self.glasses.lock() {
            for glass in glasses.values_mut() {
                let _ = glass.surface.sync_bounds(overlay.0 as isize);
            }
        }
        if let Ok(mut recycled) = self.recycled.lock() {
            for glass in recycled.iter_mut() {
                let _ = glass.surface.sync_bounds(overlay.0 as isize);
            }
        }
    }

    /// Destroys every menu glass (open + pooled). Called on app exit.
    pub fn destroy(&self) {
        self.glasses
            .lock()
            .expect("menu glasses lock poisoned")
            .clear();
        self.recycled
            .lock()
            .expect("recycled glasses lock poisoned")
            .clear();
        self.hit_rects
            .lock()
            .expect("menu hit rects lock poisoned")
            .clear();
        self.ui_hit_rects
            .lock()
            .expect("ui hit rects lock poisoned")
            .clear();
    }
}
