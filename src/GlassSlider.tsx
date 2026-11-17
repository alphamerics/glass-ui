import { useCallback, useEffect, useRef, useState } from "react";
import type { CSSProperties, PointerEvent as ReactPointerEvent, ReactNode } from "react";
import { prewarmMenuGlass } from "./bridge";
import { LiquidGlassSurface } from "./LiquidGlassSurface";
import type { GlassTransform } from "./LiquidGlassSurface";
import {
  DRAG_THRESHOLD_PX,
  GLASS_TIMING,
  HOLD_MS,
  SETTLE_MS,
  SETTLE_TIMING,
  THUMB_MATERIAL,
  useTwoFrameGrow,
} from "./thumbGlass";

const clamp = (value: number, min: number, max: number) =>
  Math.min(max, Math.max(min, value));

/** Thumb geometry (CSS px — keep in sync with styles.css). The plain thumb
 *  is a small white stadium; HOLDING grows the glass into the big one. */
const THUMB_W = 22;
const THUMB_H = 16;
const HELD_W = 34;
const HELD_H = 22;
/** The held glass spawns thumb-sized, then tweens to the held stadium size
 *  (center-anchored, so it blooms out of the small thumb). */
const GROW_SCALE: [number, number] = [HELD_W / THUMB_W, HELD_H / THUMB_H];

/**
 * A macOS-style slider that works like GlassSwitch: the resting thumb is a
 * small white circle; press and HOLD (or start dragging) and the thumb
 * grows into a liquid glass stadium that follows the pointer; on release it
 * snaps to the value, shrinks back to the small thumb as it settles, and
 * then hands off to the plain thumb. A quick press anywhere on the track
 * just jumps the value (no glass spawned). Thin accent-filled track with
 * optional leading/trailing icons (e.g. dim/sun). Full keyboard support:
 * arrows step, PageUp/PageDown steps x10, Home/End. Controlled via
 * `value`/`onChange` with `onCommit` firing once per gesture.
 */
export function GlassSlider({
  value,
  min = 0,
  max = 100,
  step = 1,
  onChange,
  onCommit,
  disabled = false,
  label,
  leadingIcon,
  trailingIcon,
  className,
  style,
}: {
  value: number;
  min?: number;
  max?: number;
  step?: number;
  /** Fires continuously while the value changes (drag, click, keys). */
  onChange?: (value: number) => void;
  /** Fires once when a drag/click gesture ends. */
  onCommit?: (value: number) => void;
  disabled?: boolean;
  label?: string;
  leadingIcon?: ReactNode;
  trailingIcon?: ReactNode;
  className?: string;
  style?: CSSProperties;
}) {
  const trackRef = useRef<HTMLSpanElement>(null);
  const pointerRef = useRef(-1);
  const startXRef = useRef(0);
  const holdTimerRef = useRef(0);
  const settleTimerRef = useRef(0);
  const dragModeRef = useRef(false);
  const settlingRef = useRef(false);
  const valueRef = useRef(value);
  valueRef.current = value;

  const [dragging, setDragging] = useState(false);
  const [grown, setGrown] = useState(false);
  // The glass thumb's center x within the track (CSS px) while dragging.
  const [dragX, setDragX] = useState(0);
  // The measured track width, captured when the glass drag starts.
  const [trackW, setTrackW] = useState(0);
  // The release settle: the glass shrinks back to thumb size while sliding
  // to the value's position, then hands off to the plain thumb.
  const [settling, setSettling] = useState<{ x: number; dx: number } | null>(
    null,
  );
  const glassActive = dragging || settling != null;
  useTwoFrameGrow(dragging, setGrown);

  // Warm one local pool surface so the FIRST hold spawns its glass
  // instantly (a fresh capture session needs hundreds of ms to its first
  // frame; a parked warm surface re-anchors in one step).
  useEffect(() => {
    void prewarmMenuGlass(1, true);
  }, []);

  useEffect(
    () => () => {
      window.clearTimeout(holdTimerRef.current);
      window.clearTimeout(settleTimerRef.current);
    },
    [],
  );

  const pct = (clamp(value, min, max) - min) / (max - min || 1);

  const valueFromClientX = useCallback(
    (clientX: number) => {
      const rect = trackRef.current?.getBoundingClientRect();
      if (!rect || rect.width === 0) {
        return valueRef.current;
      }
      const ratio = clamp((clientX - rect.left) / rect.width, 0, 1);
      const raw = min + ratio * (max - min);
      return clamp(Math.round(raw / step) * step, min, max);
    },
    [min, max, step],
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
    settlingRef.current = false;
    setSettling(null);
    const rect = trackRef.current?.getBoundingClientRect();
    const width = rect?.width ?? 0;
    setTrackW(width);
    setDragX(
      rect ? clamp(clientX - rect.left, 0, width) : (width * pct) || 0,
    );
    setDragging(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (disabled || !event.isPrimary) {
      return;
    }
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    pointerRef.current = event.pointerId;
    startXRef.current = event.clientX;
    holdTimerRef.current = window.setTimeout(() => {
      enterDrag(event.clientX);
    }, HOLD_MS);
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (disabled || pointerRef.current !== event.pointerId) {
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
      setTrackW(rect.width);
      setDragX(clamp(event.clientX - rect.left, 0, rect.width));
    }
    onChange?.(valueFromClientX(event.clientX));
  };

  const endPress = (event: ReactPointerEvent<HTMLDivElement>, cancel: boolean) => {
    if (pointerRef.current !== event.pointerId) {
      return;
    }
    pointerRef.current = -1;
    window.clearTimeout(holdTimerRef.current);
    const next = valueFromClientX(event.clientX);
    if (dragModeRef.current) {
      dragModeRef.current = false;
      if (!cancel) {
        onChange?.(next);
        onCommit?.(next);
      }
      // The glass slides to the value's position and shrinks back to thumb
      // size, then hands off to the plain thumb.
      if (!cancel && trackW > 0) {
        const snapX = ((next - min) / (max - min || 1)) * trackW;
        settlingRef.current = true;
        setSettling({ x: dragX, dx: snapX - dragX });
        settleTimerRef.current = window.setTimeout(() => {
          settlingRef.current = false;
          setSettling(null);
        }, SETTLE_MS);
      }
      setDragging(false);
      return;
    }
    // A quick press: just jump the value (no glass was spawned).
    if (!cancel) {
      onChange?.(next);
      onCommit?.(next);
    }
  };

  const stepBy = (steps: number) => {
    const next = clamp(value + steps * step, min, max);
    if (next !== value) {
      onChange?.(next);
      onCommit?.(next);
    }
  };

  const glassTransform: GlassTransform | undefined = dragging
    ? grown
      ? { scale: GROW_SCALE }
      : undefined
    : settling
      ? { scale: 1, translate: [settling.dx, 0] }
      : undefined;

  return (
    <div
      className={`glass-slider${className ? ` ${className}` : ""}`}
      style={style}
      role="slider"
      aria-label={label}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={value}
      aria-disabled={disabled || undefined}
      data-dragging={dragging || undefined}
      tabIndex={disabled ? -1 : 0}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={(event) => endPress(event, false)}
      onPointerCancel={(event) => endPress(event, true)}
      onKeyDown={(event) => {
        if (disabled) {
          return;
        }
        const page = Math.max(1, Math.round((max - min) / (step * 10)) * step);
        switch (event.key) {
          case "ArrowLeft":
          case "ArrowDown":
            event.preventDefault();
            stepBy(-1);
            break;
          case "ArrowRight":
          case "ArrowUp":
            event.preventDefault();
            stepBy(1);
            break;
          case "PageDown":
            event.preventDefault();
            stepBy(-page);
            break;
          case "PageUp":
            event.preventDefault();
            stepBy(page);
            break;
          case "Home":
            event.preventDefault();
            onChange?.(min);
            onCommit?.(min);
            break;
          case "End":
            event.preventDefault();
            onChange?.(max);
            onCommit?.(max);
            break;
        }
      }}
    >
      {leadingIcon && (
        <span className="glass-slider-icon" aria-hidden>
          {leadingIcon}
        </span>
      )}
      <span ref={trackRef} className="glass-slider-track">
        <span
          className="glass-slider-range"
          style={{ width: `${pct * 100}%` }}
        />
        {glassActive ? (
          <LiquidGlassSurface
            className="glass-slider-thumb glass-slider-thumb-glass"
            radiusPx={THUMB_H / 2}
            shape="pill"
            backdrop="local"
            transition="instant"
            material={THUMB_MATERIAL}
            transform={glassTransform}
            transformTiming={dragging ? GLASS_TIMING : SETTLE_TIMING}
            style={{
              left:
                (trackW > 0
                  ? dragging
                    ? dragX
                    : settling!.x
                  : pct * trackW) -
                THUMB_W / 2,
              transition: "none",
            }}
          />
        ) : (
          <span
            className="glass-slider-thumb"
            style={{ left: `${pct * 100}%` }}
          />
        )}
      </span>
      {trailingIcon && (
        <span className="glass-slider-icon" aria-hidden>
          {trailingIcon}
        </span>
      )}
    </div>
  );
}
