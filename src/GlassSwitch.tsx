import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties, PointerEvent as ReactPointerEvent } from "react";
import { prewarmMenuGlass } from "./bridge";
import { LiquidGlassSurface } from "./LiquidGlassSurface";
import type { GlassTransform } from "./LiquidGlassSurface";
import {
  DRAG_THRESHOLD_PX,
  GLASS_TIMING,
  GLASS_SCALE,
  HOLD_MS,
  SETTLE_MS,
  SETTLE_TIMING,
  THUMB_MATERIAL,
} from "./thumbGlass";

/** Track/thumb geometry (CSS px — keep in sync with styles.css). */
const TRACK_W = 42;
const THUMB_W = 24;
const THUMB_H = 14;
const PAD = 2;
const TRAVEL = TRACK_W - THUMB_W - PAD * 2;

const clamp = (value: number, min: number, max: number): number =>
  Math.min(max, Math.max(min, value));

/**
 * A macOS-style toggle whose thumb is liquid glass. A quick press just
 * toggles (thumb slides, track recolors — no glass spawned). Press and HOLD
 * (or start dragging) and the thumb grows into a local liquid glass thumb
 * that follows the pointer; on release it snaps to the nearer position —
 * the glass shrinking and sliding back to thumb size as it settles — and
 * then hands off to the plain thumb. A hold that ends without movement is a
 * click: it toggles to the other side (the glass slides there as it
 * settles), with the track previewing the toggle while held.
 *
 * Controlled via `checked`/`onChange`, uncontrolled with `defaultChecked`.
 */
export function GlassSwitch({
  checked,
  defaultChecked = false,
  onChange,
  disabled = false,
  label,
  className,
  style,
}: {
  checked?: boolean;
  defaultChecked?: boolean;
  onChange?: (checked: boolean) => void;
  disabled?: boolean;
  /** Accessible name + hover tooltip. */
  label?: string;
  className?: string;
  style?: CSSProperties;
}) {
  const controlled = checked !== undefined;
  const [innerChecked, setInnerChecked] = useState(defaultChecked);
  const value = controlled ? (checked as boolean) : innerChecked;
  const [dragging, setDragging] = useState(false);
  const [dragX, setDragX] = useState(0);
  // The release settle: the glass shrinks back to thumb size while sliding
  // to the snapped position, then hands off to the plain thumb.
  const [settling, setSettling] = useState<{ x: number; dx: number } | null>(
    null,
  );
  // The grow tween must arrive at the native side AFTER the surface's first
  // rect push (two frames), or the held spec would snap the glass to full
  // size instead of tweening the grow.
  const [grown, setGrown] = useState(false);

  const trackRef = useRef<HTMLSpanElement>(null);
  const dragModeRef = useRef(false);
  const startXRef = useRef(0);
  // The drag's start position and whether the pointer actually moved: a
  // hold that ends without movement is a TOGGLE (the glass slides to the
  // other side), a drag releases to the nearer snap position.
  const dragStartXRef = useRef(0);
  const movedRef = useRef(false);
  const holdTimerRef = useRef(0);
  const settleTimerRef = useRef(0);
  const pointerIdRef = useRef(-1);
  // Mirrors `settling != null` for the event handlers (which see the last
  // committed render): a press DURING the settle reuses the live surface.
  const settlingRef = useRef(false);

  const commit = useCallback(
    (next: boolean) => {
      if (!controlled) {
        setInnerChecked(next);
      }
      onChange?.(next);
    },
    [controlled, onChange],
  );

  const enterDrag = useCallback((clientX: number) => {
    if (dragModeRef.current) {
      return;
    }
    dragModeRef.current = true;
    window.clearTimeout(holdTimerRef.current);
    // A press during the release settle keeps the live glass surface: the
    // transform tween simply reverses (shrink → grow) with no respawn.
    window.clearTimeout(settleTimerRef.current);
    const wasSettling = settlingRef.current;
    settlingRef.current = false;
    setSettling(null);
    const rect = trackRef.current?.getBoundingClientRect();
    const x = clamp(
      rect ? clientX - rect.left - THUMB_W / 2 : (TRACK_W - THUMB_W) / 2,
      PAD,
      PAD + TRAVEL,
    );
    dragStartXRef.current = x;
    movedRef.current = false;
    setDragX(x);
    setDragging(true);
    // The two-frame grow wait exists only for a FRESH spawn (the grow spec
    // must not land before the surface's first rect push, or the spawn
    // snaps the glass to full size). A live surface's rect is already
    // pushed, so its grow spec goes out at once and the settle tween
    // retargets straight into the grow (CSS semantics). Waiting the two
    // frames here lets the next rect push re-anchor first and re-apply the
    // STALE held settle spec onto the new position — the glass visibly
    // slips toward the middle of the track before the grow catches it
    // (the halt-then-snap glitch on a quick re-press).
    if (wasSettling) {
      setGrown(true);
    }
  }, []);

  const onPointerDown = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (disabled || !event.isPrimary) {
      return;
    }
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    pointerIdRef.current = event.pointerId;
    startXRef.current = event.clientX;
    holdTimerRef.current = window.setTimeout(() => {
      enterDrag(event.clientX);
    }, HOLD_MS);
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (pointerIdRef.current !== event.pointerId || disabled) {
      return;
    }
    if (!dragModeRef.current) {
      if (Math.abs(event.clientX - startXRef.current) >= DRAG_THRESHOLD_PX) {
        enterDrag(event.clientX);
      }
      return;
    }
    const rect = trackRef.current?.getBoundingClientRect();
    if (rect) {
      const x = clamp(
        event.clientX - rect.left - THUMB_W / 2,
        PAD,
        PAD + TRAVEL,
      );
      if (Math.abs(x - dragStartXRef.current) > 1) movedRef.current = true;
      setDragX(x);
    }
  };

  const endPress = (event: ReactPointerEvent<HTMLButtonElement>, cancel: boolean) => {
    if (pointerIdRef.current !== event.pointerId) {
      return;
    }
    pointerIdRef.current = -1;
    window.clearTimeout(holdTimerRef.current);
    if (dragModeRef.current) {
      dragModeRef.current = false;
      // A hold that ended without movement reads as a click: release
      // toggles to the other side (the glass shrinks and slides there).
      // A drag releases to whichever toggle the thumb sits nearer to.
      const next = movedRef.current ? dragX + THUMB_W / 2 > TRACK_W / 2 : !value;
      const snapX = next ? PAD + TRAVEL : PAD;
      if (!cancel) {
        commit(next);
      }
      settlingRef.current = true;
      setSettling({ x: dragX, dx: snapX - dragX });
      setDragging(false);
      window.clearTimeout(settleTimerRef.current);
      settleTimerRef.current = window.setTimeout(() => {
        settlingRef.current = false;
        setSettling(null);
      }, SETTLE_MS);
      return;
    }
    if (!cancel) {
      commit(!value);
    }
  };

  useEffect(
    () => () => {
      window.clearTimeout(holdTimerRef.current);
      window.clearTimeout(settleTimerRef.current);
    },
    [],
  );

  // The grow tween: the transform spec goes out only once the surface's
  // first rect push has landed (two frames), so the native side tweens the
  // scale instead of snapping a held spec onto the fresh surface.
  useEffect(() => {
    if (!dragging) {
      setGrown(false);
      return;
    }
    let inner = 0;
    const outer = requestAnimationFrame(() => {
      inner = requestAnimationFrame(() => setGrown(true));
    });
    return () => {
      cancelAnimationFrame(outer);
      cancelAnimationFrame(inner);
    };
  }, [dragging]);

  // Warm one local pool surface per switch so the FIRST hold spawns its
  // glass instantly (a fresh capture session needs hundreds of ms to its
  // first frame; a parked warm surface re-anchors in one step).
  useEffect(() => {
    void prewarmMenuGlass(1, true);
  }, []);

  const glassActive = dragging || settling != null;
  const glassX = dragging ? dragX : settling ? settling.x : 0;
  const thumbX = value ? PAD + TRAVEL : PAD;
  // The track's preview: a drag previews the side the thumb sits nearer
  // to; a pure hold previews the toggle it will land on at release.
  const wouldCheck = dragging
    ? movedRef.current
      ? dragX + THUMB_W / 2 > TRACK_W / 2
      : !value
    : value;
  const glassTransform: GlassTransform | undefined = dragging
    ? grown
      ? { scale: GLASS_SCALE }
      : undefined
    : settling
      ? { scale: 1, translate: [settling.dx, 0] }
      : undefined;

  return (
    <button
      type="button"
      role="switch"
      aria-checked={value}
      aria-label={label}
      title={label}
      data-on={value || undefined}
      data-dragging={dragging || undefined}
      data-preview={dragging ? (wouldCheck ? "on" : "off") : undefined}
      className={className ? `glass-switch ${className}` : "glass-switch"}
      style={style}
      disabled={disabled}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={(event) => endPress(event, false)}
      onPointerCancel={(event) => endPress(event, true)}
      onKeyDown={(event) => {
        if (!disabled && (event.key === " " || event.key === "Enter")) {
          event.preventDefault();
          commit(!value);
        }
      }}
    >
      <span ref={trackRef} className="glass-switch-track" />
      {glassActive ? (
        <LiquidGlassSurface
          className="glass-switch-thumb glass-switch-thumb-glass"
          radiusPx={THUMB_H / 2}
          shape="pill"
          backdrop="local"
          transition="instant"
          material={THUMB_MATERIAL}
          transform={glassTransform}
          transformTiming={dragging ? GLASS_TIMING : SETTLE_TIMING}
          style={{ left: glassX, transition: "none" }}
        />
      ) : (
        <span className="glass-switch-thumb" style={{ left: thumbX }} />
      )}
    </button>
  );
}
