//! The menu-glass manager: one native liquid glass surface per open web-UI
//! menu, keyed by the menu's id, plus the warm recycle pool, grouped
//! reveals, the menu scale spring, per-key hit rects and the reconciliation
//! sweep.
//!
//! Everything here is app-agnostic: it needs only the overlay HWND (per
//! call), the seed shader params (app-provided; the shared global material
//! is merged on top at spawn time) and, optionally, a group-ready emitter.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use liquid_glass::{
    translate_compositor, translate_surface_window, GlassConfig, GlassShape, GlassSurface,
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
    /// The menu key this instance currently serves (pooled surfaces get
    /// re-keyed on reuse).
    key: String,
    /// The host window this surface was created for. The reconciliation
    /// sweep is scoped to it: two webview windows share one manager, and
    /// each runs its own 1 s sweep — without the host scoping, every
    /// sweep would despawn the OTHER window's glass (a 1 Hz
    /// despawn/respawn war between the windows).
    host: isize,
    rect: [f32; 4],
    /// The rect the SURFACE window actually covers: the element's rect,
    /// except for metaball surfaces whose canvas extends beyond the tiny
    /// anchored element (see [`metaball_canvas`]).
    canvas: [f32; 4],
    radius: f32,
    /// Small grouped surfaces tone the shader's height-proportional edge
    /// band and outline down — see [`small_surface_params`].
    small: bool,
    /// Fade-style reveal (`glass_fade` = 1 in the surface's params); tracked
    /// so surfaces recycled between the two styles get re-seeded.
    fade: bool,
    /// Which feed paints this surface (see [`liquid_glass::BackdropSource`]):
    /// the global monitor capture, or a local per-window capture of the
    /// host app's own content. Recycled surfaces only ever re-enter menus
    /// of their own kind — and their own host window.
    local: bool,
    /// The surface's own shape preset (the web UI's `shape` prop):
    /// `None` is the default rounded rect, `Some(Superellipse)` draws the
    /// squircle. Applied onto the surface when it changes (recycled
    /// surfaces carry their previous life's value for the dedup).
    shape: Option<GlassShape>,
    /// Metaball state: when set, the surface draws a metaball field instead
    /// of its shape preset. The field is SHARED between every metaball
    /// element of the window (the web UI pushes the full ball set with each
    /// element's push), so metaball containers merge with each other — and
    /// only with each other — while plain shapes stay plain.
    metaball: Option<MetaballFieldState>,
    /// The metaball FIELD GENERATION this surface's field copy belongs to.
    /// Every metaball push carries the whole shared field, so all of the
    /// host's metaball surfaces advance one generation per field change —
    /// each in its own (separately queued) push. The present-sync gate uses
    /// the per-surface generation to hold a just-updated surface's present
    /// back until every sibling holds the same generation's field, so the
    /// shared blob's territory halves always compose from the SAME field
    /// (a loop tick between two sibling pushes would otherwise present one
    /// new half against an old one, opening a mid-frame gap in the neck).
    metaball_generation: u64,
    /// The transform target last applied through the animate API (the
    /// engine-units conversion of the key's held spec), for dedup: a
    /// re-stated identical target must not restart a running tween (CSS
    /// transition semantics — no value change, no restart).
    animated_transform: Option<[f32; 4]>,
}

/// One metaball surface's field state, deduplicated against incoming pushes.
#[derive(Clone, PartialEq)]
struct MetaballFieldState {
    /// The element's own ball radius (physical px) — the element itself may
    /// be a tiny anchor dot far smaller than its blob.
    ball_radius: f32,
    /// The full shared field: every metaball element's ball, in
    /// window-relative physical pixels (x, y, extentX, extentY, weight,
    /// exponent). Exponent 2 with equal extents is the plain circle.
    balls: Vec<[f32; 6]>,
    /// The field's tension exponent (2 = the classic blend).
    tension: f32,
}

/// How far a metaball surface's canvas extends beyond its anchored element:
/// the ball silhouette's own reach (the blob must render around the tiny
/// element that anchors it) plus slack for the merged neck reaching toward
/// neighbor balls. Neighbor canvases overlap across this slack, so together
/// they cover the whole merged silhouette.
const METABALL_CANVAS_SLACK_PX: f32 = 48.0;

/// How long the present-sync gate holds a just-updated metaball surface
/// back from presenting when a sibling still shows the previous field
/// generation. Normally the siblings' same-generation pushes arrive
/// microseconds later (every metaball element pushes the whole shared
/// field whenever any ball moves) and release the gate in the same event
/// pump; the deadline is the safety net for a lost or slow push — two-plus
/// render-loop ticks of hold reads as at most the pre-fix skew prior to it.
const METABALL_GATE_HOLD: std::time::Duration = std::time::Duration::from_millis(40);

/// How far a ball's silhouette reaches from its center: the plain circle's
/// radius, or a shaped ball's superellipse corner distance (the
/// conservative bounding-disc radius of its silhouette).
fn metaball_ball_reach(ball: [f32; 6]) -> f32 {
    let circle = ball[5] == 2.0 && ball[2] == ball[3];
    if circle {
        ball[2]
    } else {
        (ball[2] * ball[2] + ball[3] * ball[3]).sqrt()
    }
}

/// The canvas slack for a field state: the OWN ball's silhouette reach
/// (its field copy carries its own ball first) — a circle ball keeps the
/// classic exact radius, a shaped one the corner distance.
fn metaball_reach(field: &MetaballFieldState) -> f32 {
    field
        .balls
        .first()
        .map(|ball| metaball_ball_reach(*ball))
        .unwrap_or(field.ball_radius)
}

/// Grows an element rect into a metaball surface's canvas.
///
/// The grown rect is snapped onto the window's integer pixel grid — the
/// exact truncation [`SurfaceAnchor::Rect`] applies when the host places
/// the surface (`as i32` per component, geometry.rs). The field's ball
/// coordinates are derived from this canvas, so the snap must match the
/// placement exactly: a canvas the field assumes at x.6 but that renders
/// at x.0 shifts this surface's whole silhouette by up to a pixel against
/// its neighbors' — the merged blob's halves then disagree about where the
/// territory bisector falls, opening a 1px hollow seam between two merging
/// metaballs (or a double-composited dark seam, on the other side of the
/// fraction). The truncation costs at most one pixel of the 48px slack.
fn metaball_canvas(rect: [f32; 4], reach: f32) -> [f32; 4] {
    let slack = reach + METABALL_CANVAS_SLACK_PX;
    [
        (rect[0] - slack).trunc(),
        (rect[1] - slack).trunc(),
        (rect[2] + 2.0 * slack).trunc(),
        (rect[3] + 2.0 * slack).trunc(),
    ]
}

/// Converts window-relative balls into canvas-relative ones (the surface
/// spans exactly the canvas). Position and extents translate; weight and
/// exponent ride along.
fn metaball_balls_in_canvas(canvas: [f32; 4], balls: &[[f32; 6]]) -> Vec<[f32; 6]> {
    balls
        .iter()
        .map(|ball| {
            [
                ball[0] - canvas[0],
                ball[1] - canvas[1],
                ball[2],
                ball[3],
                ball[4],
                ball[5],
            ]
        })
        .collect()
}

/// Whether every OTHER metaball surface of `host` already holds at least
/// `generation`'s field — i.e. the push arriving now is the LAST one of its
/// field change, the group is coherent, and the presents can fire together
/// (one composition beat carries every half at the same field). A parked
/// (paused) member is not required: it presents nothing until revived, and
/// the revive path re-stamps its generation.
fn metaball_group_coherent(
    glasses: &HashMap<String, MenuGlass>,
    updated: &str,
    host: isize,
    generation: u64,
) -> bool {
    glasses.values().all(|other| {
        other.key == updated
            || other.host != host
            || other.metaball.is_none()
            || other.metaball_generation >= generation
            || other.surface.is_paused()
    })
}

/// Releases the present-sync holds and force-presents every metaball
/// surface of `host` holding at least `generation`'s field — called in the
/// LAST member's push (the coherent preview matched), so the whole field
/// group renders back to back on ONE composition beat: the shared blob
/// composes whole, with no half at a younger field.
fn release_and_present_metaball_group(
    glasses: &mut HashMap<String, MenuGlass>,
    overlay: HWND,
    generation: u64,
) {
    let host = overlay.0 as isize;
    for glass in glasses.values_mut() {
        if glass.host == host
            && glass.metaball.is_some()
            && glass.metaball_generation >= generation
        {
            glass.surface.release_frame_gate();
            let _ = glass.surface.present_now();
        }
    }
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
    /// Keys whose surfaces are still waiting to be revealed, in
    /// registration order (the stagger releases them in this order).
    keys: Vec<String>,
    /// The watcher stops waiting past this point and reveals whatever is
    /// ready (a surface whose capture session is very slow should not block
    /// the group forever).
    deadline: std::time::Instant,
    /// Members already released to their reveal.
    released: usize,
    /// When the last member was released (the stagger's cadence clock).
    last_release: Option<std::time::Instant>,
}

/// The stagger between grouped reveal releases. Revealing every member on
/// the same tick is the engine's peak GPU moment — several large panels
/// animating noise fields simultaneously saturates integrated GPUs hard
/// enough to trigger driver resets. A 35 ms cadence flattens the spike
/// while staying well inside the content fade, so the group still reads as
/// one reveal.
const GROUP_STAGGER: std::time::Duration = std::time::Duration::from_millis(35);

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
    /// The metaball field GENERATION AND CONTENT per host window: the
    /// generation each metaball push joins (or advances), with a canonical
    /// copy of the content it labels so sibling pushes of one field change
    /// (identical field content, different own-first ball order) reuse the
    /// same generation. The present-sync gate compares each surface's copy
    /// generation against this (see [`MenuGlass::metaball_generation`]).
    metaball_fields: Mutex<HashMap<isize, (u64, (Vec<[f32; 6]>, f32))>>,
    /// The transform spec held per menu key (the `animate_menu_glass`
    /// API's storage): the key's CURRENT transform target, so a surface
    /// freshly created or recycled for the key snaps to it — the transform
    /// is a property of the menu, not of one surface life. Removed when
    /// the menu closes.
    transforms: Mutex<HashMap<String, GlassTransformAnimation>>,
}

unsafe impl Send for MenuGlassManager {}
unsafe impl Sync for MenuGlassManager {}

impl Default for MenuGlassManager {
    fn default() -> Self {
        Self::new()
    }
}

/// One metaball element's push payload (see
/// [`MenuGlassManager::position_menu_glass`]).
///
/// The `balls` field must carry the pushing element's OWN ball FIRST
/// (index 0) — each metaball surface renders the shared field only where
/// its own ball dominates, so every surface needs to know which ball is
/// its own. The web side orders each surface's field copy that way.
#[derive(Clone, Debug)]
pub struct MetaballPush {
    /// The element's own ball radius, physical pixels — the anchored
    /// element itself may be a tiny dot far smaller than its blob.
    pub ball_radius: f32,
    /// The full shared field: every metaball element's ball, in
    /// window-relative physical pixels (x, y, extentX, extentY, weight,
    /// exponent). Exponent 2 with equal extents is the plain circle; any
    /// other exponent or unequal extents shapes the ball's silhouette.
    /// Every metaball surface receives the WHOLE field, so metaball
    /// containers merge with each other (and only with each other).
    pub balls: Vec<[f64; 6]>,
    /// The field's tension exponent (2 = the classic blend).
    pub tension: f32,
}

/// A CSS-style animated transform for a menu glass's drawn shape, expressed
/// the way a web author writes it: a scale factor and a translate in
/// PHYSICAL pixels (both center-anchored, like CSS's default
/// `transform-origin: center`), animated over `duration_ms` after
/// `delay_ms` through a cubic-bezier curve (`easing` = the `[x1, y1, x2,
/// y2]` control points — the CSS timing function). The glass transitions
/// from wherever it visually is into the target (CSS transition
/// semantics), and the target persists until changed or the menu closes.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GlassTransformAnimation {
    /// The animation's duration in milliseconds (default 240).
    pub duration_ms: f64,
    /// The animation's start delay in milliseconds (default 0).
    pub delay_ms: f64,
    /// The cubic-bezier control points `[x1, y1, x2, y2]` (default
    /// `[0.25, 0.1, 0.25, 1]` — CSS `ease`). x coordinates are clamped to
    /// [0, 1] (CSS's validation); y may overshoot for back-out curves.
    pub easing: [f64; 4],
    /// The scale factors `(x, y)` (default 1 = unscaled).
    pub scale: [f64; 2],
    /// The translation in physical pixels `(x, y)`, center-anchored
    /// (default none).
    pub translate: [f64; 2],
}

impl Default for GlassTransformAnimation {
    fn default() -> Self {
        Self {
            duration_ms: 240.0,
            delay_ms: 0.0,
            easing: [0.25, 0.1, 0.25, 1.0],
            scale: [1.0, 1.0],
            translate: [0.0, 0.0],
        }
    }
}

/// Converts a [`GlassTransformAnimation`] into the engine's shape
/// transform for a surface whose shape region spans `canvas` (x, y, w, h,
/// physical pixels; the identity region the menu surfaces use).
///
/// The engine's transform is `(scale_x, scale_y, offset_x, offset_y)` in
/// shape-region units anchored to the shape's BOTTOM-CENTER, while the CSS
/// model scales and translates around the element's CENTER. With the
/// shader mapping `s' = (s - offset - anchor) / scale + anchor` (anchor =
/// `(0, 0.5)`, s in units of the canvas height), the center-anchored
/// equivalent is:
///
/// - `offset_x = dx / h`
/// - `offset_y = dy / h - 0.5 + 0.5 * scale_y`
///
/// (the `±0.5` terms re-anchor the scale from bottom-center to center; the
/// offset is applied unscaled by the shader, so the translate lands
/// exactly regardless of the scale).
fn css_transform_to_engine(
    canvas: [f32; 4],
    scale: [f64; 2],
    translate: [f64; 2],
) -> [f32; 4] {
    let height = canvas[3].max(1.0);
    let scale_x = scale[0] as f32;
    // CSS scale(0) would collapse the shape onto its anchor; the shader's
    // own `max(…, 1e-4)` guard matches.
    let scale_y = (scale[1] as f32).max(1e-4);
    let offset_x = (translate[0] as f32) / height;
    let offset_y = (translate[1] as f32) / height - 0.5 + 0.5 * scale_y;
    [scale_x, scale_y, offset_x, offset_y]
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
            metaball_fields: Mutex::new(HashMap::new()),
            transforms: Mutex::new(HashMap::new()),
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

    /// Joins (or advances) the host's shared metaball field generation for
    /// a push's field content. A push that re-states the generation's known
    /// content reuses that generation — every metaball element pushes the
    /// WHOLE shared field when any ball moves, so the sibling pushes of one
    /// field change arrive with identical field content and MUST share one
    /// generation for the present-sync gate to pair them (only a real
    /// content change advances it). Stored canonically (balls sorted) since
    /// each surface's copy carries its own ball first.
    fn metaball_generation_for(
        &self,
        host: isize,
        field: &MetaballFieldState,
    ) -> u64 {
        let mut canon = field.balls.clone();
        // f32 has no Ord; compare element-wise with the IEEE total order so
        // the canonical copy is a stable key (bit pattern, NaN included).
        canon.sort_unstable_by(|a, b| {
            for (left, right) in a.iter().zip(b.iter()) {
                let ordering = left.total_cmp(right);
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            }
            std::cmp::Ordering::Equal
        });
        let canon_field = (canon, field.tension);
        let mut fields = self
            .metaball_fields
            .lock()
            .expect("metaball field generation lock poisoned");
        let entry = fields.entry(host).or_insert((0, canon_field.clone()));
        if entry.1 == canon_field {
            return entry.0;
        }
        entry.0 += 1;
        entry.1 = canon_field;
        entry.0
    }

    /// Replaces the seed shader params; every surface created afterwards
    /// (menus, pooled) seeds from these. Existing surfaces are re-seeded
    /// (each keeps its own fade/small derivation).
    pub fn set_params(&self, params: liquid_glass::GlassShaderParams) {
        *self.params.lock().expect("params lock poisoned") = params;
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
    fn seed_params(
        &self,
        small: bool,
        height_px: f32,
        fade: bool,
    ) -> liquid_glass::GlassShaderParams {
        let mut params = *self.params.lock().expect("params lock poisoned");
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
    pub fn prewarm(&self, overlay: HWND, count: usize, local: bool) -> Result<(), String> {
        let mut recycled = self
            .recycled
            .lock()
            .expect("recycled glasses lock poisoned");
        let room = RECYCLE_POOL_MAX.saturating_sub(recycled.len());
        for _ in 0..count.min(room) {
            let config = Self::menu_config(
                20.0,
                SurfaceAnchor::Rect {
                    offset_x: 0.0,
                    offset_y: 0.0,
                    width: 1.0,
                    height: 1.0,
                },
                local,
            );
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
                        key: String::new(),
                        host: overlay.0 as isize,
                        rect: [0.0, 0.0, 1.0, 1.0],
                        canvas: [0.0, 0.0, 1.0, 1.0],
                        radius: 20.0,
                        small: false,
                        fade: false,
                        local,
                        shape: None,
                        metaball: None,
                        metaball_generation: 0,
                        animated_transform: None,
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

    /// The creation config for one menu glass surface. Global surfaces keep
    /// the Glassbar overlay sandwich (beneath a transparent host, excluded
    /// from captures); local surfaces are in-app glass stacked above the
    /// host's own content, fed by that window's local feed — and may stay
    /// visible to screen recorders, since their window capture excludes
    /// overlapping windows on its own.
    fn menu_config(radius: f32, anchor: SurfaceAnchor, local: bool) -> GlassConfig {
        GlassConfig {
            corner_radius_px: radius,
            surface_anchor: anchor,
            // The app's own persistent surface already styled the host
            // overlay; menu spawns must not re-style it.
            style_host_window: false,
            backdrop_source: if local {
                liquid_glass::BackdropSource::Local
            } else {
                liquid_glass::BackdropSource::Global
            },
            placement: if local {
                liquid_glass::SurfacePlacement::AboveHost
            } else {
                liquid_glass::SurfacePlacement::BeneathHost
            },
            exclude_from_capture: !local,
            ..Default::default()
        }
    }

    /// Registers a freshly created surface in the pending reveal group.
    fn register_group_member(&self, key: &str) {
        let mut pending = self
            .pending_group
            .lock()
            .expect("pending group lock poisoned");
        match pending.as_mut() {
            Some(existing) => {
                existing.keys.push(key.to_owned());
            }
            None => {
                *pending = Some(PendingGroup {
                    keys: vec![key.to_owned()],
                    // A capture session that never warms up must not block
                    // the group forever.
                    deadline: std::time::Instant::now() + std::time::Duration::from_millis(2500),
                    released: 0,
                    last_release: None,
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
            group.keys.retain(|existing| existing != key);
        }
    }

    fn group_member_pending(&self, key: &str) -> bool {
        self.pending_group
            .lock()
            .expect("pending group lock poisoned")
            .as_ref()
            .is_some_and(|group| group.keys.iter().any(|existing| existing == key))
    }

    /// Called each host-app tick: reveals the whole pending group the moment
    /// every member has presented its first frame (or once the deadline
    /// passes), so grouped glass appears in a single instant. Also fires the
    /// group emitter so the web UI can fade its content in with the glass.
    pub fn check_pending_group(&self) {
        let (group_keys, deadline, mut released, mut last_release) = {
            let guard = self
                .pending_group
                .lock()
                .expect("pending group lock poisoned");
            match guard.as_ref() {
                Some(group) => (
                    group.keys.clone(),
                    group.deadline,
                    group.released,
                    group.last_release,
                ),
                None => return,
            }
        };
        let mut all_ready = true;
        {
            let mut glasses = self.glasses.lock().expect("menu glasses lock poisoned");
            for key in &group_keys[released.min(group_keys.len())..] {
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
        // Release members one at a time on the stagger cadence (a member
        // closed meanwhile is skipped without counting against the pace).
        let now = std::time::Instant::now();
        let deadline_passed = now >= deadline;
        if let Some(last) = last_release {
            if now.duration_since(last) < GROUP_STAGGER {
                return;
            }
        }
        {
            let mut glasses = self.glasses.lock().expect("menu glasses lock poisoned");
            if released < group_keys.len() {
                let key = &group_keys[released];
                released += 1;
                last_release = Some(now);
                if let Some(glass) = glasses.get_mut(key) {
                    // Resume rendering right as the reveal starts.
                    glass.surface.set_paused(false);
                    glass.surface.set_reveal(true, MENU_REVEAL_INSTANT);
                }
                // The web UI's content fade fires with the FIRST release so
                // its eases ride the staggered reveals.
                if released == 1 {
                    if let Some(emit) = self.emitter.lock().expect("emitter lock poisoned").as_ref()
                    {
                        emit();
                    }
                }
                // One release per tick: the next member goes on a later
                // tick (the caller polls this on its supervision loop).
            }
        }
        if released >= group_keys.len() || deadline_passed {
            *self
                .pending_group
                .lock()
                .expect("pending group lock poisoned") = None;
        } else {
            let mut guard = self
                .pending_group
                .lock()
                .expect("pending group lock poisoned");
            if let Some(group) = guard.as_mut() {
                group.released = released;
                group.last_release = last_release;
            }
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
    /// wobble reveal. `local` feeds the surface from its host window's own
    /// capture instead of the shared monitor capture (in-app glass).
    ///
    /// `metaball` turns the surface into a metaball container: its drawn
    /// shape becomes the shared metaball field instead of the shape preset,
    /// and the anchored canvas grows beyond the element rect so the blob
    /// (and its merged necks) render around the — possibly tiny — element.
    /// Every metaball element pushes the SAME full field, so their blobs
    /// merge; a plain shape never merges with a field.
    ///
    /// `shape` selects the surface's own preset beyond the rounded rect:
    /// `Some(GlassShape::Superellipse)` draws the squircle (its exponent
    /// carries over as the metaball ball's shape when merging).
    #[allow(clippy::too_many_arguments)]
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
        local: bool,
        shape: Option<GlassShape>,
        metaball: Option<MetaballPush>,
    ) -> Result<(), String> {
        let metaball = metaball.map(|push| MetaballFieldState {
            ball_radius: push.ball_radius,
            balls: push
                .balls
                .iter()
                .map(|ball| {
                    [
                        ball[0] as f32,
                        ball[1] as f32,
                        ball[2] as f32,
                        ball[3] as f32,
                        ball[4] as f32,
                        ball[5] as f32,
                    ]
                })
                .collect(),
            tension: push.tension,
        });
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
        // The metaball field's present-sync generation: advances only when
        // the FIELD CONTENT changes — the sibling pushes of one field change
        // re-state the same content and join the same generation (the gate
        // below uses that to pair them). `None` for plain shapes.
        let metaball_generation = metaball
            .as_ref()
            .map(|field| self.metaball_generation_for(overlay.0 as isize, field));
        // The present-sync coherence preview: the push arriving now is the
        // LAST one of its field change when every other metaball member of
        // the host already holds the generation's field. Then (after the
        // lock) the whole group releases + force-presents in this single op,
        // and this push's own apply presents in step instead of gating —
        // one composition beat carries every half at the same field. In any
        // other push order the group stays incoherent: the just-updated
        // surface holds its present through the compositor's gate, and the
        // LAST member's op above presents everyone together.
        let group_coherent = match (&metaball, metaball_generation) {
            (Some(_), Some(generation)) => {
                let glasses = self.glasses.lock().expect("menu glasses lock poisoned");
                metaball_group_coherent(&glasses, key, overlay.0 as isize, generation)
            }
            _ => false,
        };
        let mut glasses = self.glasses.lock().expect("menu glasses lock poisoned");
        if group_coherent {
            if let Some(generation) = metaball_generation {
                release_and_present_metaball_group(&mut glasses, overlay, generation);
            }
        }
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
                // The menu is gone: its held transform spec goes with it
                // (a re-open starts fresh; the web UI re-sends its props).
                self.transforms
                    .lock()
                    .expect("transforms lock poisoned")
                    .remove(key);
                // Always drop the key from a pending reveal group: the
                // frontend's cleanup pushes carry no group, and a stale key
                // here would let the group reveal resurrect glass for a view
                // that no longer exists.
                self.remove_group_member(key);
                return Ok(());
            }
            Some(rect) => {
                // A surface switching feeds (global ↔ local) — or switching
                // between a plain shape and a metaball field (a different
                // canvas geometry and shape kind) — cannot be re-anchored into
                // the other kind: park it and let the creation path below
                // build the right one.
                if glasses.get(key).is_some_and(|glass| {
                    glass.local != local || glass.metaball.is_some() != metaball.is_some()
                }) {
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
                }
                if let Some(glass) = glasses.get_mut(key) {
                    // Metaball surfaces anchor a canvas grown beyond the
                    // element (the blob renders around the tiny element);
                    // plain surfaces anchor the element rect itself.
                    let surface_rect = metaball
                        .as_ref()
                        .map_or(rect, |metaball| metaball_canvas(rect, metaball_reach(metaball)));
                    let moved = !glass
                        .canvas
                        .iter()
                        .zip(surface_rect.iter())
                        .all(|(a, b)| (a - b).abs() < 2.0);
                    if moved {
                        // KEEP the surface's z slot: this re-anchor happens
                        // per frame while a metaball is dragged, and the
                        // full re-pin would insert the surface at the
                        // bottom of the host's surface stack — burying the
                        // dragged blob beneath the host's other glass
                        // mid-drag. Z repair belongs to `sync_bounds`
                        // (host events), which re-pins in a deliberate
                        // order.
                        glass.surface.set_anchor_keep_z(SurfaceAnchor::Rect {
                            offset_x: surface_rect[0],
                            offset_y: surface_rect[1],
                            width: surface_rect[2],
                            height: surface_rect[3],
                        })?;
                        glass.rect = rect;
                        glass.canvas = surface_rect;
                        // The SDF's corner radius is normalized to the
                        // surface height, which may have changed with the
                        // new rect; re-apply it.
                        glass.surface.set_corner_radius(glass.radius)?;
                        // A held transform spec is expressed relative to the
                        // canvas, so a canvas change re-applies it (CSS
                        // semantics: the transform rides the element, not
                        // one canvas size).
                        if let Some(spec) =
                            self.transforms
                                .lock()
                                .expect("transforms lock poisoned")
                                .get(key)
                                .cloned()
                        {
                            Self::apply_transform_spec(glass, &spec, true);
                        }
                        Self::update_hit_rect(overlay, &self.hit_rects, key, &rect);
                    }
                    // The metaball field rides on top as pure constant-buffer
                    // updates — dragged balls re-target at the render loop's
                    // rate with no window operations at all. `balls[0]` is
                    // this element's own ball (the web orders each push that
                    // way), so the territory clip splits the merged blob
                    // between the sharing surfaces instead of rendering it
                    // on every one of them.
                    if glass.metaball != metaball {
                        if let (Some(metaball), Some(generation)) = (&metaball, metaball_generation)
                        {
                            // The field MUST be converted against the canvas the
                            // surface window is actually placed at — `glass.canvas`
                            // — never the freshly computed `surface_rect`: when the
                            // `moved` tolerance above suppressed the re-anchor (the
                            // snapped canvas can flip by 1px while the element
                            // merely drifts sub-pixel), the window still sits on
                            // the OLD canvas. Converting against the new one then
                            // shifts this surface's whole silhouette by the
                            // suppressed delta against its neighbors' — their
                            // territory bisectors disagree and the thin merged
                            // neck opens a 1px hollow seam. (When `moved` was
                            // true, `glass.canvas` was just set to the new rect,
                            // so this matches either way.)
                            glass.surface.set_metaballs_field_px(
                                &metaball_balls_in_canvas(glass.canvas, &metaball.balls),
                                metaball.tension,
                                Some(0),
                            )?;
                            glass.metaball_generation = generation;
                            // Present-sync: this window move + field must land on
                            // ONE composition beat (a throttled loop tick 1-2
                            // beats later would compose the moved window with the
                            // OLD blob, stretching the neck with drag speed). A
                            // coherent group needs the LAST member's push — its
                            // coherence op already force-presented everyone — so
                            // only the LAST member presents here, and only while
                            // no hold is outstanding.
                            if group_coherent {
                                let _ = glass.surface.present_now();
                            } else {
                                glass
                                    .surface
                                    .gate_frame(std::time::Instant::now() + METABALL_GATE_HOLD);
                            }
                        }
                        glass.metaball = metaball;
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
                    // Shape-preset changes reconfigure in place too (same
                    // canvas geometry — only the drawn SDF switches).
                    if glass.shape != shape {
                        glass.surface.set_shape(shape.unwrap_or_default())?;
                        glass.shape = shape;
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
                    glass.key = key.to_owned();
                    glass.host = overlay.0 as isize;
                    return Ok(());
                }
                // Prefer a warm, collapsed surface from the recycle pool: it
                // re-anchors instantly, while a fresh capture session needs
                // hundreds of milliseconds before its first frame arrives.
                // Only surfaces of the SAME feed kind AND the SAME host
                // window are reusable — a global surface must never be
                // re-anchored into the local pipeline or vice versa, and a
                // surface's DComp target is bound to the window it was
                // created for.
                let mut glass = {
                    let mut recycled = self
                        .recycled
                        .lock()
                        .expect("recycled glasses lock poisoned");
                    let overlay_host = overlay.0 as isize;
                    let surface_rect = metaball
                        .as_ref()
                        .map_or(rect, |metaball| metaball_canvas(rect, metaball_reach(metaball)));
                    let match_index = recycled.iter().rposition(|pooled| {
                        pooled.local == local
                            && pooled.host == overlay_host
                            && pooled.metaball.is_some() == metaball.is_some()
                    });
                    let pooled = match_index.map(|index| recycled.remove(index));
                    drop(recycled);
                    match pooled {
                        Some(mut recycled) => {
                            recycled
                                .surface
                                .set_anchor(SurfaceAnchor::Rect {
                                    offset_x: surface_rect[0],
                                    offset_y: surface_rect[1],
                                    width: surface_rect[2],
                                    height: surface_rect[3],
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
                            let config = Self::menu_config(
                                radius,
                                SurfaceAnchor::Rect {
                                    offset_x: surface_rect[0],
                                    offset_y: surface_rect[1],
                                    width: surface_rect[2],
                                    height: surface_rect[3],
                                },
                                local,
                            );
                            let mut surface = match GlassSurface::create(overlay.0 as isize, config)
                            {
                                Ok(surface) => surface,
                                Err(error) => {
                                    eprintln!("glass-ui menu glass failed: {error}");
                                    return Err(error);
                                }
                            };
                            // A metaball surface's field must be on before
                            // the reveal so the first visible frame already
                            // shows the blob (not a stale preset shape).
                            if let Some(metaball) = &metaball {
                                surface.set_metaballs_field_px(
                                    &metaball_balls_in_canvas(surface_rect, &metaball.balls),
                                    metaball.tension,
                                    Some(0),
                                )?;
                                if !group_coherent {
                                    surface.gate_frame(
                                        std::time::Instant::now() + METABALL_GATE_HOLD,
                                    );
                                } else {
                                    let _ = surface.present_now();
                                }
                            }
                            let small = group.is_some();
                            // Seed the fresh surface with the saved glass
                            // settings (plus this surface's fade style) BEFORE
                            // the reveal is configured — set_reveal stamps its
                            // own turbulence/wobble onto the params, and seeding
                            // afterwards would visibly snap the material as the
                            // user's values land.
                            seed_surface(
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
                                key: key.to_owned(),
                                host: overlay.0 as isize,
                                rect,
                                canvas: surface_rect,
                                radius,
                                small,
                                fade: glass_fade,
                                local,
                                shape,
                                metaball: metaball.clone(),
                                metaball_generation: metaball_generation.unwrap_or(0),
                                animated_transform: None,
                            }
                        }
                    }
                };
                // The recycled surface's height changed, so its normalized
                // corner radius must be re-applied either way.
                glass.surface.set_corner_radius(radius)?;
                // ...and its shape preset re-applied (a pooled surface may
                // carry another life's kind).
                glass.surface.set_shape(shape.unwrap_or_default())?;
                glass.shape = shape;
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
                // Apply (or refresh) the metaball field against THIS canvas:
                // a pooled surface carries a stale canvas origin, and even a
                // fresh one records its state here for the dedup below.
                let surface_rect = metaball
                    .as_ref()
                    .map_or(rect, |metaball| metaball_canvas(rect, metaball_reach(metaball)));
                if let Some(metaball) = &metaball {
                    if glass.metaball.as_ref() != Some(metaball) || glass.canvas != surface_rect {
                        glass.surface.set_metaballs_field_px(
                            &metaball_balls_in_canvas(surface_rect, &metaball.balls),
                            metaball.tension,
                            Some(0),
                        )?;
                    }
                }
                // The pooled/fresh surface's field generation marker: even a
                // content-hit recycles into the current generation (its copy
                // is the pushed field group's state either way).
                glass.metaball_generation = metaball_generation.unwrap_or(0);
                if let Some(metaball) = &metaball {
                    if glass.metaball.as_ref() != Some(metaball) || glass.canvas != surface_rect {
                        if !group_coherent {
                            glass
                                .surface
                                .gate_frame(std::time::Instant::now() + METABALL_GATE_HOLD);
                        } else {
                            let _ = glass.surface.present_now();
                        }
                        glass.metaball = Some(metaball.clone());
                    }
                } else {
                    glass.metaball = None;
                }
                glass.rect = rect;
                glass.canvas = surface_rect;
                glass.radius = radius;
                glass.key = key.to_owned();
                glass.host = overlay.0 as isize;
                // A fresh (or recycled) surface's life starts at the
                // identity transform; a transform the key holds snaps on
                // immediately (the menu re-opening with its transform is
                // the CSS-authored state, not an animation).
                glass.animated_transform = None;
                if let Some(spec) = self
                    .transforms
                    .lock()
                    .expect("transforms lock poisoned")
                    .get(key)
                    .cloned()
                {
                    Self::apply_transform_spec(&mut glass, &spec, false);
                }
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

    /// Animates (or re-states) a menu glass's CSS-style transform: the
    /// drawn shape transitions from wherever it visually is into the spec's
    /// target through the spec's cubic-bezier curve (see
    /// [`GlassTransformAnimation`]). The spec is HELD per key: surfaces
    /// created (or re-anchored) for the key afterwards snap to it, and a
    /// re-stated identical target never restarts a running animation.
    /// Unknown keys are a no-op (the surface may not exist yet; the held
    /// spec applies on creation).
    pub fn animate_menu_glass(
        &self,
        overlay: HWND,
        key: &str,
        animation: GlassTransformAnimation,
    ) -> Result<(), String> {
        // The overlay is not part of the lookup (keys are unique per
        // surface); the parameter stays for API symmetry with the other
        // manager operations and future host-scoping needs.
        let _ = overlay;
        self.transforms
            .lock()
            .expect("transforms lock poisoned")
            .insert(key.to_owned(), animation.clone());
        let mut glasses = self.glasses.lock().expect("menu glasses lock poisoned");
        if let Some(glass) = glasses.get_mut(key) {
            Self::apply_transform_spec(glass, &animation, true);
        }
        Ok(())
    }

    /// Converts a transform spec against a glass's canvas and applies it to
    /// the surface. `animated` glides from the live transform through the
    /// spec's easing (the CSS transition path); `!animated` snaps (fresh
    /// surfaces, canvas re-anchors). Dedups against the last applied
    /// target so re-stated specs never restart a settled (or running)
    /// animation.
    fn apply_transform_spec(glass: &mut MenuGlass, spec: &GlassTransformAnimation, animated: bool) {
        let target = css_transform_to_engine(glass.canvas, spec.scale, spec.translate);
        if glass.animated_transform == Some(target) {
            return;
        }
        glass.animated_transform = Some(target);
        let duration_secs = (spec.duration_ms.max(1.0) / 1000.0) as f32;
        let delay_secs = (spec.delay_ms.max(0.0) / 1000.0) as f32;
        let easing = [
            spec.easing[0] as f32,
            spec.easing[1] as f32,
            spec.easing[2] as f32,
            spec.easing[3] as f32,
        ];
        let result = if animated {
            glass
                .surface
                .set_shape_transform_eased(target, duration_secs, easing, delay_secs)
        } else {
            glass.surface.set_shape_transform(target)
        };
        if let Err(error) = result {
            eprintln!("glass-ui menu glass transform failed: {error}");
        }
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
    /// closes every live menu glass **of the calling window** (`host`)
    /// whose key the webview no longer tracks. This is the hard guarantee
    /// that a lost close push can never leave a glass visible forever —
    /// the webview's active-key registry is the source of truth.
    ///
    /// The host scoping is load-bearing: several webview windows share one
    /// manager, and each runs its own 1 s sweep. Without the scoping,
    /// window A's sweep (which only knows A's keys) would despawn window
    /// B's glass every second — and B's sweep would repay the favor — a
    /// 1 Hz despawn/respawn war.
    pub fn sync_glasses(&self, keys: &[String], host: isize) {
        let stale: Vec<String> = {
            let glasses = self.glasses.lock().expect("menu glasses lock poisoned");
            glasses
                .iter()
                .filter(|(_, glass)| glass.host == host && !keys.contains(&glass.key.clone()))
                .map(|(key, _)| key.clone())
                .collect()
        };
        if stale.is_empty() {
            return;
        }
        for key in stale {
            // The close path: collapse to the end, then park for reuse.
            let closed = {
                let mut glasses = self.glasses.lock().expect("menu glasses lock poisoned");
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
            self.transforms
                .lock()
                .expect("transforms lock poisoned")
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
    ///
    /// The re-pin order is deliberate: each re-pin inserts the surface
    /// directly above its host, pushing earlier ones higher — so METABALL
    /// surfaces are re-pinned FIRST and end up ABOVE the plain containers.
    /// Without the partition the map's iteration order decided who floated,
    /// and a plain panel's glass could bury a metaball blob (the blobs are
    /// playful floating elements; they read on top or not at all).
    pub fn sync_bounds(&self, overlay: HWND) {
        if let Ok(mut glasses) = self.glasses.lock() {
            let mut ordered: Vec<&mut MenuGlass> = glasses.values_mut().collect();
            ordered.sort_by_key(|glass| match glass.metaball {
                Some(_) => 0,
                None => 1,
            });
            for glass in ordered {
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
        self.transforms
            .lock()
            .expect("transforms lock poisoned")
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{css_transform_to_engine, metaball_ball_reach, metaball_canvas};

    /// The canvas feeds both the surface's anchor rect and the metaball
    /// field's coordinate origin, so every component must survive the
    /// host's integer truncation (`as i32` in anchor_rect) unchanged —
    /// otherwise the rendered silhouette shifts up to a pixel against the
    /// field the shader evaluates, and neighboring metaball surfaces'
    /// territory bisectors disagree (the 1px hollow merge seam).
    #[test]
    fn metaball_canvas_snaps_to_the_host_pixel_grid() {
        // Fractional element rects are the norm: CSS layout subpixels and
        // fractional DPI scales both land here.
        for rect in [
            [10.4, 20.6, 30.2, 40.8],
            [0.25, 0.75, 100.5, 60.25],
            [-3.5, 7.2, 88.9, 12.4],
        ] {
            for reach in [0.0f32, 64.0, 121.5] {
                let canvas = metaball_canvas(rect, reach);
                assert!(
                    canvas.iter().all(|value| value.fract() == 0.0),
                    "canvas {canvas:?} is not on the integer pixel grid"
                );
                // The truncation the host applies must be a no-op.
                let truncated: [f32; 4] = canvas
                    .iter()
                    .map(|value| *value as i32 as f32)
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap();
                assert_eq!(truncated, canvas, "host truncation would move the canvas");
            }
        }
        // The snap must not shrink the canvas meaningfully: the grown rect
        // still covers the ball plus the merge slack.
        let canvas = metaball_canvas([100.9, 100.1, 16.4, 16.6], 64.0);
        assert!((canvas[2] - (16.4 + 2.0 * (64.0 + 48.0))).abs() < 2.0);
        assert!((canvas[3] - (16.6 + 2.0 * (64.0 + 48.0))).abs() < 2.0);
    }

    /// The reach drives the canvas slack, so it must bound the silhouette:
    /// the circle stays exact, and a shaped ball's bounding disc covers its
    /// superellipse (which never leaves the hypot-of-extents disc).
    #[test]
    fn metaball_ball_reach_bounds_the_silhouette() {
        for radius in [0.0f32, 24.0, 64.0] {
            let circle = [0.0, 0.0, radius, radius, 1.0, 2.0];
            assert_eq!(
                metaball_ball_reach(circle),
                radius,
                "a circle ball's reach is exactly its radius"
            );
        }
        let squircle = [0.0, 0.0, 64.0, 40.0, 1.0, 5.0];
        let reach = metaball_ball_reach(squircle);
        assert!(
            reach >= (64.0f32 * 64.0 + 40.0 * 40.0).sqrt() - 1e-3,
            "a shaped ball's reach is its bounding-disc corner distance"
        );
    }

    /// The identity CSS transform must convert to the engine's identity:
    /// no scale, no offset.
    #[test]
    fn css_transform_identity_maps_to_engine_identity() {
        let canvas = [0.0, 0.0, 200.0, 100.0];
        assert_eq!(
            css_transform_to_engine(canvas, [1.0, 1.0], [0.0, 0.0]),
            [1.0, 1.0, 0.0, 0.0]
        );
    }

    /// A pure translate (center-anchored px) maps to an engine offset in
    /// canvas-height units; the bottom-center anchor's ±0.5 terms cancel at
    /// scale 1.
    #[test]
    fn css_translate_maps_to_height_units() {
        let canvas = [0.0, 0.0, 200.0, 100.0];
        // (50, -25) px on a 100px-tall canvas → (+0.5, -0.25) height units.
        let converted = css_transform_to_engine(canvas, [1.0, 1.0], [50.0, -25.0]);
        assert!((converted[2] - 0.5).abs() < 1e-6);
        assert!((converted[3] + 0.25).abs() < 1e-6);
    }

    /// A pure scale (center-anchored) must keep the shape's CENTER at the
    /// canvas center: verified through the shader's own mapping
    /// `s = offset + anchor + (s' - anchor) * scale`.
    #[test]
    fn css_scale_keeps_the_center_anchored() {
        let canvas = [0.0, 0.0, 200.0, 100.0];
        for scale in [0.5_f64, 1.0, 1.4, 2.0] {
            let [_, _, ox, oy] = css_transform_to_engine(canvas, [scale, scale], [0.0, 0.0]);
            // The shape's center in s' is (0, 0); its landing spot in s:
            let center_x = ox; // anchor.x = 0
            let center_y = oy + 0.5 + (0.0 - 0.5) * scale as f32;
            assert!(center_x.abs() < 1e-6, "scale {scale}: x {center_x}");
            assert!(center_y.abs() < 1e-6, "scale {scale}: y {center_y}");
        }
    }

    /// Scale + translate compose: the center lands exactly at the
    /// translated position regardless of the scale (CSS composes
    /// `translate` and `scale` with translate applied after — the offset
    /// is unscaled in the engine too).
    #[test]
    fn css_scale_and_translate_compose() {
        let canvas = [0.0, 0.0, 200.0, 100.0];
        let [_, _, ox, oy] = css_transform_to_engine(canvas, [1.3, 0.8], [10.0, 4.0]);
        let center_x = ox;
        let center_y = oy + 0.5 + (0.0 - 0.5) * 0.8_f32;
        assert!((center_x - 10.0 / 100.0).abs() < 1e-6);
        assert!((center_y - 4.0 / 100.0).abs() < 1e-6);
    }

    /// A zero scale must not divide by zero: the y guard keeps the
    /// conversion finite.
    #[test]
    fn css_zero_scale_is_guarded() {
        let canvas = [0.0, 0.0, 200.0, 100.0];
        let converted = css_transform_to_engine(canvas, [0.0, 0.0], [0.0, 0.0]);
        assert!(converted.iter().all(|value| value.is_finite()));
        assert_eq!(converted[1], 1e-4);
    }
}
