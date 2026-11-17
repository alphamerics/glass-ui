import { useEffect, useId, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import {
  animateMenuGlass,
  getClientOffset,
  positionMenuGlass,
  syncMenuGlasses,
  type GlassMaterialOverride,
  type GlassShapeSpec,
  type MetaballField,
} from "./bridge";
import { easingCss, parseEasing, type GlassEasing } from "./easing";
import { onDomMutations } from "./domWatch";

const DEFAULT_RADIUS_PX = 20;
const DEFAULT_TRANSFORM_DURATION_MS = 240;
const TRACK_INTERVAL_MS = 120;
const SWEEP_INTERVAL_MS = 1000;

/** A CSS-style transform for the glass (and, for the component form, its
 *  wrapper content): `scale` factors (a uniform number or `[x, y]`) and a
 *  `translate` in CSS pixels — both center-anchored, like CSS's default
 *  transform origin. Scale factors must be positive (the engine clamps at
 *  a tiny positive epsilon); the translate rides at CSS px and is
 *  converted to physical pixels for the native side. */
export interface GlassTransform {
  scale?: number | [number, number];
  translate?: [number, number];
}

/** The transition timing applied when the transform target changes —
 *  CSS `transition` semantics: duration, delay and a [`GlassEasing`]
 *  curve (keywords, `cubic-bezier(...)` strings or control-point
 *  arrays). Defaults: 240ms, no delay, `ease`. */
export interface GlassTransformTiming {
  durationMs?: number;
  delayMs?: number;
  easing?: GlassEasing;
}

/** Resolves a [`GlassTransform`] into uniform numbers (null = no
 *  transform). Scale clamps to a tiny positive epsilon on both the DOM
 *  and native sides so the two layers never disagree. */
function resolveTransform(
  transform?: GlassTransform,
): { scale: [number, number]; translate: [number, number] } | null {
  if (!transform) {
    return null;
  }
  const rawScale = Array.isArray(transform.scale)
    ? transform.scale
    : [transform.scale ?? 1, transform.scale ?? 1];
  const translate = transform.translate ?? [0, 0];
  const finiteOr = (value: number | undefined, fallback: number): number =>
    Number.isFinite(value) ? (value as number) : fallback;
  return {
    scale: [
      Math.max(finiteOr(rawScale[0], 1), 1e-4),
      Math.max(finiteOr(rawScale[1], finiteOr(rawScale[0], 1)), 1e-4),
    ],
    translate: [finiteOr(translate[0], 0), finiteOr(translate[1], 0)],
  };
}

/**
 * The shared transform animation: pushes the CSS-style target to the
 * native compositor whenever it changes (CSS transition semantics — the
 * glass glides from wherever it visually is into the target through the
 * bezier curve), and returns the wrapper's inline style that plays the
 * IDENTICAL curve on the DOM side, so glass and content move as one.
 * Returns undefined when no transform is set (and, after one was set and
 * then removed, the identity style with the transition kept, so the DOM
 * eases back instead of snapping).
 */
export function useGlassTransform(
  key: string,
  transform?: GlassTransform,
  timing?: GlassTransformTiming,
): CSSProperties | undefined {
  const resolved = resolveTransform(transform);
  // Primitive descriptors keep the effects stable across re-renders (an
  // inline object prop would re-fire them every render; the native side
  // dedups, but the DOM style churn is avoidable noise).
  const [scaleX, scaleY] = resolved?.scale ?? [1, 1];
  const [translateX, translateY] = resolved?.translate ?? [0, 0];
  const durationMs = timing?.durationMs ?? DEFAULT_TRANSFORM_DURATION_MS;
  const delayMs = timing?.delayMs ?? 0;
  const easing = parseEasing(timing?.easing);
  const targetKey = resolved
    ? `${scaleX},${scaleY},${translateX},${translateY}`
    : "";
  const timingKey = `${durationMs},${delayMs},${easing.join(",")}`;

  const everActiveRef = useRef(resolved != null);
  useEffect(() => {
    if (resolved) {
      everActiveRef.current = true;
    }
  });

  useEffect(() => {
    // An empty key means "inactive" (the useLiquidGlassSurface hook gates
    // closed menus through it): no surface exists to animate.
    if (!key) {
      return;
    }
    if (!resolved) {
      // The prop was removed: transition back to the identity — but only
      // when a transform was actually held (a surface mounted without one
      // has nothing to ease back, and no IPC to spend on it). The native
      // side dedups when it already sits at the identity.
      if (!everActiveRef.current) {
        return;
      }
      void animateMenuGlass(key, {
        durationMs,
        delayMs,
        easing,
        scale: [1, 1],
        translate: [0, 0],
      });
      return;
    }
    // The native translate speaks physical pixels (like the rect pushes).
    const deviceScale = window.devicePixelRatio || 1;
    void animateMenuGlass(key, {
      durationMs,
      delayMs,
      easing,
      scale: [scaleX, scaleY],
      translate: [translateX * deviceScale, translateY * deviceScale],
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- the primitive
    // descriptors above ARE the dependency set (object props re-memo).
  }, [key, targetKey, timingKey]);

  if (!resolved) {
    // Keep the transition alive after a removal so the DOM eases back to
    // the identity instead of snapping.
    return everActiveRef.current
      ? {
          transform: "none",
          transition: transformTransition(durationMs, delayMs, easing),
        }
      : undefined;
  }
  return {
    transform: `translate(${translateX}px, ${translateY}px) scale(${scaleX}, ${scaleY})`,
    transition: transformTransition(durationMs, delayMs, easing),
    willChange: "transform",
  };
}

/**
 * The wrapper's inline `transition` while a transform is active: the
 * transform rides the spec's curve — and the stylesheet's own opacity
 * fade (the fade/glass-fade content transitions) is preserved alongside,
 * since an inline `transition` would otherwise replace it.
 */
function transformTransition(
  durationMs: number,
  delayMs: number,
  easing: [number, number, number, number],
): string {
  return `transform ${durationMs}ms ${easingCss(easing)} ${delayMs}ms, opacity 160ms ease`;
}

/**
 * The wrapper's LAYOUT rect (transforms excluded), derived from its visual
 * rect and the computed CSS transform matrix. When the component drives
 * the wrapper's transform, the native glass applies the same transform
 * itself — the pushed rect must be the untransformed box, or the two
 * would compound (the tracking loop follows the visual rect otherwise).
 * Exact for translate + scale about any origin (the matrix components
 * carry both); bails out to the visual rect for anything else.
 */
function layoutRect(node: HTMLElement): {
  left: number;
  top: number;
  width: number;
  height: number;
} {
  const rect = node.getBoundingClientRect();
  const matrix = getComputedStyle(node).transform;
  const match = matrix.match(/^matrix\((.+)\)$/);
  if (!match) {
    return rect;
  }
  const [a, b, c, d, e, f] = match[1].split(",").map(Number);
  if (
    ![a, b, c, d, e, f].every(Number.isFinite) ||
    a <= 0 ||
    d <= 0 ||
    b !== 0 ||
    c !== 0
  ) {
    // Non-finite entries, a collapsed axis, or rotation/skew/mirror — the
    // untransform is not exact, so best-effort follow the visual rect.
    return rect;
  }
  const width = rect.width / Math.abs(a);
  const height = rect.height / Math.abs(d);
  // A scale about the center keeps the center fixed, so the layout center
  // is the visual center minus the translation — no un-scaling of the
  // coordinate (dividing by a/d here would shrink the center coordinate by
  // the scale factor and misplace the box).
  const centerX = rect.left + rect.width / 2 - e;
  const centerY = rect.top + rect.height / 2 - f;
  return {
    left: centerX - width / 2,
    top: centerY - height / 2,
    width,
    height,
  };
}

/* ---- The shared metaball field ----
 * Every metaball-flagged surface registers a live getter for its own ball
 * (its element's silhouette: center, superellipse half-extents, shape
 * exponent). Each metaball surface pushes the WHOLE field to its own
 * native surface, so metaball containers merge with each other — and only
 * with each other; plain shapes never join the field. `metaballVersion`
 * bumps whenever any ball may have moved, so the other metaball surfaces
 * re-push their (now stale) field copy; each surface dedups its pushes
 * against the version it last sent. */
interface MetaballBall {
  x: number;
  y: number;
  /** The ball's superellipse half-extents (CSS px) — the element's
   *  silhouette scaled to the ball radius. Equal extents with exponent 2
   *  is the plain circle. */
  ex: number;
  ey: number;
  /** Contribution weight (always 1 today). */
  w: number;
  /** The ball's superellipse exponent (2 = circle/ellipse). */
  e: number;
}

const metaballBalls = new Map<string, () => MetaballBall | null>();
const metaballListeners = new Set<() => void>();
let metaballVersion = 0;

/**
 * Re-pushes every metaball surface's shared field. Call after moving a
 * metaball element outside the surfaces' own tracking (e.g. from a drag
 * handler that positions the element directly): the native field is a
 * constant-buffer update per surface, so blobs track the pointer at frame
 * rate with no window resizes.
 */
export function notifyMetaballs(): void {
  metaballVersion += 1;
  for (const listener of [...metaballListeners]) {
    listener();
  }
}

/**
 * The registry of menu-glass keys the webview currently tracks, driving the
 * periodic reconciliation sweep: the native side closes any surface whose
 * key is gone, so a lost close push (e.g. during a settings change) can
 * never leave a glass visible forever. The sweep timer starts with the
 * first registered key and runs for the page's lifetime.
 */
const activeGlassKeys = new Set<string>();
let sweepStarted = false;

function registerGlassKey(key: string): void {
  activeGlassKeys.add(key);
  if (!sweepStarted) {
    sweepStarted = true;
    window.setInterval(() => {
      // An empty registry has nothing to reconcile: skip the IPC instead of
      // sending one sync per second for the page's lifetime.
      if (activeGlassKeys.size === 0) {
        return;
      }
      void syncMenuGlasses([...activeGlassKeys]);
    }, SWEEP_INTERVAL_MS);
  }
}

function unregisterGlassKey(key: string): void {
  activeGlassKeys.delete(key);
}

/** How the surface appears:
 *  - `reveal`      the native liquid reveal plays (the default),
 *  - `instant`     no transition at all — the glass and content just appear,
 *  - `fade`        the glass appears instantly and the content eases in,
 *  - `glass-fade`  the native reveal is disabled and the glass itself
 *                  dissolves in with an opacity fade; the content eases in
 *                  with it. */
export type GlassTransition = "reveal" | "instant" | "fade" | "glass-fade";

function pushesInstant(transition: GlassTransition): boolean {
  return transition !== "reveal";
}

/** True for transitions whose glass plays the native opacity fade. */
function isGlassFade(transition: GlassTransition): boolean {
  return transition === "glass-fade";
}

/** True for transitions whose wrapped content eases in. */
function fadesContent(transition: GlassTransition): boolean {
  return transition === "fade" || transition === "glass-fade";
}

/** Which feed paints the surface's backdrop:
 *  - `global` the shared capture of the entire monitor (the default; the
 *    glass refracts everything on screen),
 *  - `local`  the host app window's own capture — glass that only ever
 *    sits over the app's own content needs no screen duplication. */
export type GlassBackdrop = "global" | "local";

/** The surface's own shape — one prop declares the glass's silhouette,
 *  and `metaball` is the orthogonal merge toggle (a merging surface's
 *  ball DERIVES from this shape, so the shape is declared exactly once):
 *  - `"container"` the rounded rect of `radiusPx` (the default look),
 *  - `"pill"`      the full capsule (round ends; `"circle"` is its alias),
 *  - `{ superellipse: n }` the squircle of exponent `n` (2 = ellipse,
 *    4..6 the iOS-style squircle, higher approaches a box). */
export type GlassSurfaceShape =
  | "container"
  | "pill"
  | "circle"
  | { superellipse: number };

function pushesLocal(backdrop?: GlassBackdrop): boolean {
  return backdrop === "local";
}

/** The wire shape spec (rounded | superellipse): pill/circle are rounded
 *  rects with a full-height corner radius, so they ride on `radiusPx`. */
function wireShape(
  shapeKind: "container" | "pill" | "superellipse",
  shapeExponent: number | undefined,
): GlassShapeSpec | undefined {
  return shapeKind === "superellipse"
    ? { kind: "superellipse", exponent: shapeExponent ?? 5 }
    : undefined;
}

/** The metaball ball's superellipse exponent, derived from the surface's
 *  own declared shape (and its rect for the fitted looks):
 *  - `{ superellipse: n }` is taken verbatim,
 *  - `"pill"`: a square element's capsule IS a circle (exponent 2); wide
 *    ones fit a boxier superellipse (a stadium is not a superellipse —
 *    this is the documented silhouette approximation),
 *  - `"container"`: the corner-radius ratio maps through the standard
 *    squircle fit (ratio ~0.22, the iOS icon, lands near 5.5; a full
 *    capsule collapses to the circle). */
function ballExponent(
  shapeKind: "container" | "pill" | "superellipse",
  shapeExponent: number | undefined,
  radiusPx: number,
  width: number,
  height: number,
): number {
  if (shapeKind === "superellipse") {
    return shapeExponent ?? 5;
  }
  const major = Math.max(width, height);
  const minor = Math.min(width, height);
  if (shapeKind === "pill") {
    return major > 0 ? 2 + 4 * (1 - minor / major) : 2;
  }
  const half = minor / 2;
  const ratio = half > 0 ? Math.min(radiusPx, half) / half : 1;
  return Math.min(32, 1 + 1 / Math.max(ratio, 1 / 31));
}

function pushRect(
  rect: { left: number; top: number; width: number; height: number },
  radiusPx: number,
  key: string,
  instant?: boolean,
  group?: string,
  glassFade?: boolean,
  spring?: boolean,
  local?: boolean,
  metaball?: MetaballField,
  shape?: GlassShapeSpec,
  material?: GlassMaterialOverride,
): void {
  const scale = window.devicePixelRatio || 1;
  const chrome = getClientOffset();
  void positionMenuGlass(
    key,
    {
      // The client area sits inset within a framed host window (title bar,
      // borders); the native anchors are window-relative, so the offset
      // rides along (0/0 on borderless hosts).
      x: rect.left * scale + chrome.x,
      y: rect.top * scale + chrome.y,
      width: rect.width * scale,
      height: rect.height * scale,
      // The rect speaks physical pixels and the native side normalizes the
      // radius against the surface's PHYSICAL height — so the radius rides
      // in the same units, or capsules go square at DPI scaling.
      radius: radiusPx * scale,
    },
    instant,
    group,
    glassFade,
    spring,
    local,
    metaball,
    shape,
    material,
  );
}

/**
 * Keeps a native liquid glass surface fitted to the last element matching
 * `selector` while `active` is true. The surface continuously inherits the
 * element's size and position, so it covers the whole popup even as it
 * animates or reflows. `key` names this menu's own glass instance; when the
 * menu closes the instance is deleted. `transition` selects the appearance
 * (see [`GlassTransition`]); the legacy `instant` boolean is kept as an alias.
 *
 * `material` pins the surface's glass material to per-surface custom
 * properties (see [`GlassMaterialOverride`]): the surface stops following
 * the app's shared material for every set field.
 *
 * `transform`/`transformTiming` animate the GLASS's shape with CSS
 * semantics (see [`GlassTransform`]). This hook animates the native glass
 * only — the tracked element is the caller's, so animating its DOM is the
 * caller's move (apply the same CSS transition to the element and the two
 * layers move as one; the [`LiquidGlassSurface`] component form does this
 * automatically).
 */
export function useLiquidGlassSurface(
  active: boolean,
  selector: string,
  radiusPx: number,
  key: string,
  instant?: boolean,
  transition?: GlassTransition,
  spring?: boolean,
  backdrop?: GlassBackdrop,
  shape?: GlassShapeSpec,
  transform?: GlassTransform,
  transformTiming?: GlassTransformTiming,
  material?: GlassMaterialOverride,
): void {
  const resolved = transition ?? (instant ? "instant" : "reveal");
  const local = pushesLocal(backdrop);
  // A closed menu has no surface to animate: gate the transform effect
  // through an empty key.
  const transformStyle = useGlassTransform(active ? key : "", transform, transformTiming);
  // The material override's primitive descriptor: the effect's dependency
  // (an inline object prop would re-run the push effect every render).
  const materialKey = material ? JSON.stringify(material) : "";
  // The push loop reads this to measure the tracked element's UNTRANSFORMED
  // layout box while a transform is active (the native glass applies the
  // same transform itself, so following the visual rect would compound the
  // two). A ref keeps the push effect's dependencies free of the transform.
  const transformActiveRef = useRef(transformStyle != null);
  transformActiveRef.current = transformStyle != null;

  useEffect(() => {
    registerGlassKey(key);
    if (!active) {
      void positionMenuGlass(
        key,
        null,
        pushesInstant(resolved),
        undefined,
        isGlassFade(resolved),
      );
      return () => {
        unregisterGlassKey(key);
        void positionMenuGlass(
          key,
          null,
          pushesInstant(resolved),
          undefined,
          isGlassFade(resolved),
        );
      };
    }
    const push = () => {
      const nodes = document.querySelectorAll<HTMLElement>(selector);
      const node = nodes.length > 0 ? nodes[nodes.length - 1] : null;
      if (node) {
        const rect = transformActiveRef.current
          ? layoutRect(node)
          : node.getBoundingClientRect();
        if (rect.width < 1 || rect.height < 1) {
          return;
        }
        pushRect(
          rect,
          radiusPx,
          key,
          pushesInstant(resolved),
          undefined,
          isGlassFade(resolved),
          spring,
          local,
          undefined,
          shape,
          material,
        );
      }
    };
    let frame = 0;
    // Radix/Floating UI position the popup after it mounts; pushing the rect
    // immediately would reveal the glass at the unsettled position. Wait two
    // frames so the glass only ever follows the settled rect.
    const pushSettled = () => {
      if (frame) {
        return;
      }
      frame = requestAnimationFrame(() => {
        frame = requestAnimationFrame(() => {
          frame = 0;
          push();
        });
      });
    };
    pushSettled();
    // The shared DOM lane (one body-subtree observer for every consumer in
    // the app, with its own double-rAF settle) syncs the glass with the web
    // UI the moment the element mounts; the interval covers later reflows
    // as a fallback.
    const unsubscribe = onDomMutations(push);
    const timer = window.setInterval(push, TRACK_INTERVAL_MS);
    return () => {
      if (frame) {
        cancelAnimationFrame(frame);
      }
      unsubscribe();
      window.clearInterval(timer);
      unregisterGlassKey(key);
      // The close push carries the surface's OWN transition style, so an
      // instant/glass-fade surface also CLOSES instantly — a bare null push
      // would close with the default wobble reveal.
      void positionMenuGlass(
        key,
        null,
        pushesInstant(resolved),
        undefined,
        isGlassFade(resolved),
      );
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps -- materialKey IS
  // material's dependency (the closure picks the object up on re-run).
  }, [active, selector, radiusPx, key, resolved, spring, local, shape, materialKey]);
}

/**
 * Wraps arbitrary HTML content and renders a native liquid glass surface
 * behind it. The glass inherits the wrapper's rendered size and position and
 * follows it as it changes. `transition` controls the appearance (see
 * [`GlassTransition`]); the legacy `instant` boolean is kept as an alias.
 *
 * `transform`/`transformTiming` give the glass a CSS-style animated
 * transform (see [`GlassTransform`]): the native shape glides through the
 * bezier curve at the compositor's rate while the wrapper element plays
 * the IDENTICAL curve as a real CSS transition — glass and content move as
 * one. The transform target persists (CSS transition semantics: a changed
 * target glides from wherever the glass visually is); removing the prop
 * eases back to the identity. While a transform is active the pushed glass
 * rect is the wrapper's UNTRANSFORMED layout box (the native side applies
 * the same transform itself, so the two never compound), and the inline
 * `transform`/`transition` styles override the `style` prop's own.
 */
export function LiquidGlassSurface({
  className,
  style,
  radiusPx = DEFAULT_RADIUS_PX,
  transition,
  instant,
  group,
  backdrop,
  shape = "container",
  metaball = false,
  metaballRadiusPx,
  metaballTension = 2,
  transform,
  transformTiming,
  material,
  children,
}: {
  className?: string;
  style?: CSSProperties;
  radiusPx?: number;
  transition?: GlassTransition;
  /** Legacy alias for `transition="instant"`. */
  instant?: boolean;
  /** Surfaces sharing a `group` appear together once all are warm. */
  group?: string;
  /** Which feed paints the backdrop (see [`GlassBackdrop`]). */
  backdrop?: GlassBackdrop;
  /** The surface's own silhouette (see [`GlassSurfaceShape`]): one prop
   *  declares the shape, `metaball` is the orthogonal merge toggle. */
  shape?: GlassSurfaceShape;
  /** The CSS-style animated transform (see [`GlassTransform`]): the glass
   *  and this wrapper's content transition together. */
  transform?: GlassTransform;
  /** The transform transition's timing (see [`GlassTransformTiming`]). */
  transformTiming?: GlassTransformTiming;
  /** The surface's own glass material properties (see
   *  [`GlassMaterialOverride`]): every set field pins the glass's look to
   *  this value instead of the app's shared material — tint, outline,
   *  border, blur, refraction, chroma, vibrancy, rim shadow. Absent
   *  fields keep the shared material's values; a set override makes the
   *  surface ignore later material changes for those fields. */
  material?: GlassMaterialOverride;
  /** Renders this surface as a METABALL container: the drawn shape becomes
   *  the shared metaball field (a liquid blob whose ball inherits THIS
   *  surface's `shape` and silhouette) instead of the shape preset.
   *  Metaball containers merge with each other — and only with each
   *  other — as their elements approach; plain shapes never join the
   *  field. The anchored element may be far smaller than its blob (a tiny
   *  draggable dot): the native canvas grows around it so the blob and
   *  its merged necks render in full. */
  metaball?: boolean;
  /** This element's ball radius in CSS pixels — the minor-axis half-extent
   *  the ball's silhouette scales to. Defaults to half the element's
   *  smaller dimension (the element circumscribed: the ball IS the
   *  element's shape). */
  metaballRadiusPx?: number;
  /** The field's tension exponent (2 = the classic metaball blend; higher
   *  values merge tighter and rounder, lower ones stretch long necks). */
  metaballTension?: number;
  children?: ReactNode;
}) {
  const surfaceId = useId();
  const resolved = transition ?? (instant ? "instant" : "reveal");
  const local = pushesLocal(backdrop);
  // The transform's DOM style (undefined = inactive) and a flag the push
  // loop reads to measure the wrapper's UNTRANSFORMED layout box while a
  // transform is active (the native glass applies the same transform, so
  // following the visual rect would compound the two).
  const transformStyle = useGlassTransform(surfaceId, transform, transformTiming);
  const transformActiveRef = useRef(transformStyle != null);
  transformActiveRef.current = transformStyle != null;
  // The tracking effect's push, exposed for the frame-rate loop below (the
  // loop lives in its own effect so transform changes never re-run the
  // tracking effect).
  const pushNodeRef = useRef<() => void>(() => {});
  const transformActive = transformStyle != null;
  // The material override's primitive descriptor: the effect's dependency
  // (an inline object prop would re-run it every render).
  const materialKey = material ? JSON.stringify(material) : "";
  // Stable primitive shape descriptors: the effect's dependencies (an
  // inline `{ superellipse: n }` object would re-run it every render).
  const shapeKind: "container" | "pill" | "superellipse" =
    typeof shape === "object"
      ? "superellipse"
      : shape === "circle"
        ? "pill"
        : shape;
  const shapeExponent = typeof shape === "object" ? shape.superellipse : undefined;
  // The pushed corner radius: a pill is a rounded rect whose radius the
  // native side clamps to the full capsule.
  const pushedRadius = shapeKind === "pill" ? 1e6 : radiusPx;
  // The wire shape spec: pill/container ride on the corner radius, only
  // the superellipse preset needs the engine's kind switch.
  const shapeSpec = wireShape(shapeKind, shapeExponent);
  // Content-fading modes: the content starts invisible and eases in once
  // mounted (the double frame wait guarantees the hidden state paints first).
  const [hidden, setHidden] = useState(fadesContent(resolved));
  useEffect(() => {
    if (!fadesContent(resolved)) {
      setHidden(false);
      return;
    }
    setHidden(true);
    let inner = 0;
    const outer = requestAnimationFrame(() => {
      inner = requestAnimationFrame(() => setHidden(false));
    });
    return () => {
      cancelAnimationFrame(outer);
      cancelAnimationFrame(inner);
    };
  }, [resolved, surfaceId]);

  useEffect(() => {
    registerGlassKey(surfaceId);
    const selector = `[data-liquid-glass-surface="${surfaceId}"]`;
    let node: HTMLElement | null = null;
    // Metaball push bookkeeping: the ball identity of the last push (own
    // movement bumps the shared field version) and the version last sent
    // to the native side (skips redundant field pushes).
    let pendingPush = 0;
    let lastBallKey = "";
    let lastPushedVersion = -1;
    let lastRectKey = "";
    const pushNode = () => {
      node ??= document.querySelector<HTMLElement>(selector);
      if (!node) {
        return;
      }
      // While a transform is active the glass rect must be the wrapper's
      // UNTRANSFORMED layout box: the native glass applies the same
      // transform itself, so following the visual (transformed) rect
      // would compound the two. Metaball balls follow the element's
      // VISUAL silhouette instead.
      const visualRect = node.getBoundingClientRect();
      const rect = transformActiveRef.current
        ? layoutRect(node)
        : visualRect;
      if (rect.width < 1 || rect.height < 1) {
        return;
      }
      if (!metaball) {
        // The frame-rate tracking loop (below) runs while a transform is
        // active; a rect dedup keeps stationary frames free of IPC.
        const rectKey = [
          rect.left.toFixed(1),
          rect.top.toFixed(1),
          rect.width.toFixed(1),
          rect.height.toFixed(1),
        ].join(",");
        if (rectKey === lastRectKey) {
          return;
        }
        lastRectKey = rectKey;
        pushRect(
          rect,
          pushedRadius,
          surfaceId,
          pushesInstant(resolved),
          group,
          isGlassFade(resolved),
          undefined,
          local,
          undefined,
          shapeSpec,
          material,
        );
        return;
      }
      // Metaball path: the shared field rides on every push. The element's
      // own movement bumps the shared version, so sibling metaball
      // surfaces refresh their (now stale) field copies; each surface
      // dedups against the version it last sent.
      const minorHalf = Math.min(visualRect.width, visualRect.height) / 2;
      const ballRadius = metaballRadiusPx ?? minorHalf;
      const ballKey = [
        visualRect.left.toFixed(1),
        visualRect.top.toFixed(1),
        visualRect.width.toFixed(1),
        visualRect.height.toFixed(1),
        ballRadius.toFixed(1),
      ].join(",");
      const ownBallMoved = ballKey !== lastBallKey;
      if (ownBallMoved) {
        lastBallKey = ballKey;
        metaballVersion += 1;
      }
      if (metaballVersion === lastPushedVersion) {
        return;
      }
      lastPushedVersion = metaballVersion;
      const deviceScale = window.devicePixelRatio || 1;
      // Each metaball surface's field copy carries ITS OWN ball FIRST
      // (index 0) — the native territory clip renders the merged blob only
      // where the own ball dominates, so the sharing surfaces tile it
      // exactly instead of double-compositing their canvases' overlap.
      const balls: MetaballField["balls"] = [];
      const ownBall = metaballBalls.get(surfaceId)?.() ?? null;
      if (ownBall) {
        balls.push([
          ownBall.x * deviceScale,
          ownBall.y * deviceScale,
          ownBall.ex * deviceScale,
          ownBall.ey * deviceScale,
          ownBall.w,
          ownBall.e,
        ]);
      }
      for (const [key, getBall] of metaballBalls) {
        if (key === surfaceId) {
          continue;
        }
        const ball = getBall();
        if (ball) {
          balls.push([
            ball.x * deviceScale,
            ball.y * deviceScale,
            ball.ex * deviceScale,
            ball.ey * deviceScale,
            ball.w,
            ball.e,
          ]);
        }
      }
      pushRect(
        visualRect,
        pushedRadius,
        surfaceId,
        pushesInstant(resolved),
        group,
        isGlassFade(resolved),
        undefined,
        local,
        { radius: ballRadius, balls, tension: metaballTension },
        shapeSpec,
        material,
      );
      // This surface's own ball moved: the siblings' field copies are
      // stale NOW, not at their next tracking tick — schedule their
      // pushes too (each coalesces and version-dedups its own).
      if (ownBallMoved) {
        for (const listener of [...metaballListeners]) {
          if (listener !== schedulePush) {
            listener();
          }
        }
      }
    };
    // The shared-field notify path: coalesced to one push per frame (a
    // dragged element bumps the version every pointer move).
    const schedulePush = () => {
      if (pendingPush) {
        return;
      }
      pendingPush = requestAnimationFrame(() => {
        pendingPush = 0;
        pushNode();
      });
    };
    pushNodeRef.current = pushNode;
    if (metaball) {
      metaballBalls.set(surfaceId, () => {
        const current = node ?? document.querySelector<HTMLElement>(selector);
        if (!current) {
          return null;
        }
        const rect = current.getBoundingClientRect();
        if (rect.width < 1 || rect.height < 1) {
          return null;
        }
        const minorHalf = Math.min(rect.width, rect.height) / 2;
        const ballRadius = metaballRadiusPx ?? minorHalf;
        const ballScale = minorHalf > 0 ? ballRadius / minorHalf : 1;
        return {
          x: rect.left + rect.width / 2,
          y: rect.top + rect.height / 2,
          ex: (rect.width / 2) * ballScale,
          ey: (rect.height / 2) * ballScale,
          w: 1,
          e: ballExponent(
            shapeKind,
            shapeExponent,
            radiusPx,
            rect.width,
            rect.height,
          ),
        };
      });
      metaballListeners.add(schedulePush);
    }
    // The wrapper can move before the first push (popup positioning, fonts,
    // reflows); wait two frames so the glass only follows the settled rect.
    const pushSettled = () => {
      requestAnimationFrame(() => requestAnimationFrame(pushNode));
    };
    const observer = new ResizeObserver(pushSettled);
    const attach = window.setTimeout(pushSettled, 0);
    const timer = window.setInterval(pushNode, TRACK_INTERVAL_MS);
    return () => {
      window.clearTimeout(attach);
      observer.disconnect();
      window.clearInterval(timer);
      if (pendingPush) {
        cancelAnimationFrame(pendingPush);
      }
      if (metaball) {
        metaballBalls.delete(surfaceId);
        metaballListeners.delete(schedulePush);
        // The field shrank: siblings must re-push without this ball.
        metaballVersion += 1;
      }
      unregisterGlassKey(surfaceId);
      // The close push carries the surface's OWN transition style, so an
      // instant/glass-fade surface also CLOSES instantly (see the hook form).
      void positionMenuGlass(
        surfaceId,
        null,
        pushesInstant(resolved),
        group,
        isGlassFade(resolved),
      );
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps -- materialKey IS
  // material's dependency (the closure picks the object up on re-run).
  }, [
    surfaceId,
    radiusPx,
    resolved,
    group,
    local,
    metaball,
    metaballRadiusPx,
    metaballTension,
    shapeKind,
    shapeExponent,
    materialKey,
  ]);

  // Frame-rate tracking while a transform is active: a transform-active
  // glass is one being animated (a drag, a hold-grow, a settle). Its
  // wrapper moves through `left`/`top` — position-only changes that fire
  // neither the ResizeObserver nor the shared DOM lane — so the interval's
  // 120ms cadence reads as visible steps. While the transform is held,
  // this loop pushes every changed rect at frame rate; the rect dedup
  // keeps stationary frames free of IPC.
  useEffect(() => {
    if (!transformActive) {
      return;
    }
    let frame = 0;
    const tick = () => {
      pushNodeRef.current();
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [transformActive]);

  return (
    <div
      data-liquid-glass-surface={surfaceId}
      data-glass-hidden={hidden || undefined}
      className={className}
      style={transformStyle ? { ...style, ...transformStyle } : style}
    >
      {children}
    </div>
  );
}
