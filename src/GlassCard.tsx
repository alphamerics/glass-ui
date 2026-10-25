import type { ReactNode } from "react";
import {
  LiquidGlassSurface,
  type GlassTransition,
} from "./LiquidGlassSurface";

/** Card roundness, mirroring the glass shapes. */
export type GlassShape = "pill" | "container" | "circle";

const SHAPE_CLASS: Record<GlassShape, string> = {
  pill: "cc-shape-pill",
  container: "cc-shape-container",
  circle: "cc-shape-circle",
};

/**
 * A liquid glass container in one of the three roundness types. Each card
 * carries its own native glass surface. By default (`glass-fade`) the native
 * reveal animation is disabled and the glass dissolves in with an opacity
 * fade while the card's content eases in with it.
 */
export function GlassCard({
  shape = "container",
  className,
  transition = "glass-fade",
  children,
}: {
  shape?: GlassShape;
  className?: string;
  transition?: GlassTransition;
  children: ReactNode;
}) {
  return (
    <LiquidGlassSurface
      className={`cc-card ${SHAPE_CLASS[shape]}${className ? ` ${className}` : ""}`}
      radiusPx={shape === "container" ? 22 : 999}
      transition={transition}
    >
      {children}
    </LiquidGlassSurface>
  );
}
