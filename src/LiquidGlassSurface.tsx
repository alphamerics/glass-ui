import { useEffect, useId, useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import {
  positionMenuGlass,
  syncMenuGlasses,
} from "./bridge";

const DEFAULT_RADIUS_PX = 20;
const TRACK_INTERVAL_MS = 120;
const SWEEP_INTERVAL_MS = 1000;

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

function pushRect(
  node: HTMLElement,
  radiusPx: number,
  key: string,
  instant?: boolean,
  group?: string,
  glassFade?: boolean,
  spring?: boolean,
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
): void {
  const resolved = transition ?? (instant ? "instant" : "reveal");

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
    // MutationObserver syncs the glass with the web UI the moment the
    // element mounts; the interval covers later reflows as a fallback.
    const observer = new MutationObserver(pushSettled);
    observer.observe(document.body, { childList: true, subtree: true });
    const timer = window.setInterval(push, TRACK_INTERVAL_MS);
    return () => {
      if (frame) {
        cancelAnimationFrame(frame);
      }
      observer.disconnect();
      window.clearInterval(timer);
      unregisterGlassKey(key);
      void positionMenuGlass(key, null);
    };
  }, [active, selector, radiusPx, key, resolved, spring]);
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
  children?: ReactNode;
}) {
  const surfaceId = useId();
  const resolved = transition ?? (instant ? "instant" : "reveal");
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
    const pushNode = () => {
      node ??= document.querySelector<HTMLElement>(selector);
      if (node) {
        const rect = node.getBoundingClientRect();
        if (rect.width < 1 || rect.height < 1) {
          return;
        }
        pushRect(
          node,
          radiusPx,
          surfaceId,
          pushesInstant(resolved),
          group,
          isGlassFade(resolved),
        );
      }
    };
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
      unregisterGlassKey(surfaceId);
      // Unused: delete this menu's own glass instance.
      void positionMenuGlass(surfaceId, null);
    };
  }, [surfaceId, radiusPx, resolved, group]);

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
