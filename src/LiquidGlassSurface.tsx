import { useEffect, useId, useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import {
  positionMenuGlass,
  syncMenuGlasses,
  type MetaballField,
} from "./bridge";
import { onDomMutations } from "./domWatch";

const DEFAULT_RADIUS_PX = 20;
const TRACK_INTERVAL_MS = 120;
const SWEEP_INTERVAL_MS = 1000;

/* ---- The shared metaball field ----
 * Every metaball-flagged surface registers a live getter for its own ball
 * (its element's center + the configured radius). Each metaball surface
 * pushes the WHOLE field to its own native surface, so metaball containers
 * merge with each other — and only with each other; plain shapes never
 * join the field. `metaballVersion` bumps whenever any ball may have
 * moved, so the other metaball surfaces re-push their (now stale) field
 * copy; each surface dedups its pushes against the version it last sent. */
interface MetaballBall {
  x: number;
  y: number;
  r: number;
  w: number;
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

function pushesLocal(backdrop?: GlassBackdrop): boolean {
  return backdrop === "local";
}

function pushRect(
  node: HTMLElement,
  radiusPx: number,
  key: string,
  instant?: boolean,
  group?: string,
  glassFade?: boolean,
  spring?: boolean,
  local?: boolean,
  metaball?: MetaballField,
): void {
  const rect = node.getBoundingClientRect();
  const scale = window.devicePixelRatio || 1;
  void positionMenuGlass(
    key,
    {
      x: rect.left * scale,
      y: rect.top * scale,
      width: rect.width * scale,
      height: rect.height * scale,
      radius: radiusPx,
    },
    instant,
    group,
    glassFade,
    spring,
    local,
    metaball,
  );
}

/**
 * Keeps a native liquid glass surface fitted to the last element matching
 * `selector` while `active` is true. The surface continuously inherits the
 * element's size and position, so it covers the whole popup even as it
 * animates or reflows. `key` names this menu's own glass instance; when the
 * menu closes the instance is deleted. `transition` selects the appearance
 * (see [`GlassTransition`]); the legacy `instant` boolean is kept as an alias.
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
): void {
  const resolved = transition ?? (instant ? "instant" : "reveal");
  const local = pushesLocal(backdrop);

  useEffect(() => {
    registerGlassKey(key);
    if (!active) {
      void positionMenuGlass(key, null);
      return () => {
        unregisterGlassKey(key);
        void positionMenuGlass(key, null);
      };
    }
    const push = () => {
      const nodes = document.querySelectorAll<HTMLElement>(selector);
      const node = nodes.length > 0 ? nodes[nodes.length - 1] : null;
      if (node) {
        const rect = node.getBoundingClientRect();
        if (rect.width < 1 || rect.height < 1) {
          return;
        }
        pushRect(
          node,
          radiusPx,
          key,
          pushesInstant(resolved),
          undefined,
          isGlassFade(resolved),
          spring,
          local,
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
      void positionMenuGlass(key, null);
    };
  }, [active, selector, radiusPx, key, resolved, spring, local]);
}

/**
 * Wraps arbitrary HTML content and renders a native liquid glass surface
 * behind it. The glass inherits the wrapper's rendered size and position and
 * follows it as it changes. `transition` controls the appearance (see
 * [`GlassTransition`]); the legacy `instant` boolean is kept as an alias.
 */
export function LiquidGlassSurface({
  className,
  style,
  radiusPx = DEFAULT_RADIUS_PX,
  transition,
  instant,
  group,
  backdrop,
  metaball = false,
  metaballRadiusPx,
  metaballTension = 2,
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
  /** Renders this surface as a METABALL container: the drawn shape becomes
   *  the shared metaball field (a liquid blob of radius `metaballRadiusPx`
   *  centered on this element) instead of the shape preset. Metaball
   *  containers merge with each other — and only with each other — as
   *  their elements approach; plain shapes never join the field. The
   *  anchored element may be far smaller than its blob (a tiny draggable
   *  dot): the native canvas grows around it so the blob and its merged
   *  necks render in full. */
  metaball?: boolean;
  /** This element's ball radius in CSS pixels. Defaults to half the
   *  element's smaller dimension (the element circumscribed). */
  metaballRadiusPx?: number;
  /** The field's tension exponent (2 = the classic metaball blend; higher
   *  values merge tighter and rounder, lower ones stretch long necks). */
  metaballTension?: number;
  children?: ReactNode;
}) {
  const surfaceId = useId();
  const resolved = transition ?? (instant ? "instant" : "reveal");
  const local = pushesLocal(backdrop);
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
    const pushNode = () => {
      node ??= document.querySelector<HTMLElement>(selector);
      if (!node) {
        return;
      }
      const rect = node.getBoundingClientRect();
      if (rect.width < 1 || rect.height < 1) {
        return;
      }
      if (!metaball) {
        pushRect(
          node,
          radiusPx,
          surfaceId,
          pushesInstant(resolved),
          group,
          isGlassFade(resolved),
          undefined,
          local,
        );
        return;
      }
      // Metaball path: the shared field rides on every push. The element's
      // own movement bumps the shared version, so sibling metaball
      // surfaces refresh their (now stale) field copies; each surface
      // dedups against the version it last sent.
      const ballRadius = metaballRadiusPx ?? Math.min(rect.width, rect.height) / 2;
      const ballKey = [
        rect.left.toFixed(1),
        rect.top.toFixed(1),
        rect.width.toFixed(1),
        rect.height.toFixed(1),
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
      const scale = window.devicePixelRatio || 1;
      // Each metaball surface's field copy carries ITS OWN ball FIRST
      // (index 0) — the native territory clip renders the merged blob only
      // where the own ball dominates, so the sharing surfaces tile it
      // exactly instead of double-compositing their canvases' overlap.
      const balls: Array<[number, number, number, number]> = [];
      const ownBall = metaballBalls.get(surfaceId)?.() ?? null;
      if (ownBall) {
        balls.push([ownBall.x * scale, ownBall.y * scale, ownBall.r * scale, ownBall.w]);
      }
      for (const [key, getBall] of metaballBalls) {
        if (key === surfaceId) {
          continue;
        }
        const ball = getBall();
        if (ball) {
          balls.push([ball.x * scale, ball.y * scale, ball.r * scale, ball.w]);
        }
      }
      pushRect(
        node,
        radiusPx,
        surfaceId,
        pushesInstant(resolved),
        group,
        isGlassFade(resolved),
        undefined,
        local,
        { radius: ballRadius, balls, tension: metaballTension },
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
        return {
          x: rect.left + rect.width / 2,
          y: rect.top + rect.height / 2,
          r: metaballRadiusPx ?? Math.min(rect.width, rect.height) / 2,
          w: 1,
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
      // Unused: delete this menu's own glass instance.
      void positionMenuGlass(surfaceId, null);
    };
  }, [surfaceId, radiusPx, resolved, group, local, metaball, metaballRadiusPx, metaballTension]);

  return (
    <div
      data-liquid-glass-surface={surfaceId}
      data-glass-hidden={hidden || undefined}
      className={className}
      style={style}
    >
      {children}
    </div>
  );
}
