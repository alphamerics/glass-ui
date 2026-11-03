import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import type { ReactNode } from "react";
import { useId } from "react";
import { useLiquidGlassSurface, type GlassBackdrop } from "./LiquidGlassSurface";
import { LiquidMenuItems, type LiquidMenuItem } from "./LiquidMenuItems";

export type LiquidDropdownItem = LiquidMenuItem;

interface LiquidDropdownProps {
  trigger: ReactNode;
  items: LiquidMenuItem[];
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  align?: "start" | "center" | "end";
  width?: number;
  borderRadius?: number;
  /** The glass plays the scale-up spring instead of the wobble. */
  spring?: boolean;
  /** Which feed paints the backdrop (see [`GlassBackdrop`]): the shared
   * monitor capture (the default) or the host window's own capture for
   * in-app glass. */
  backdrop?: GlassBackdrop;
}

export function LiquidDropdown({
  trigger,
  items,
  open,
  onOpenChange,
  align = "end",
  width = 250,
  borderRadius = 20,
  spring = false,
  backdrop,
}: LiquidDropdownProps) {
  // Each dropdown instance owns its own native glass instance, tracked via a
  // per-instance attribute selector so overlapping menus never cross-track.
  const menuId = useId();
  useLiquidGlassSurface(
    open === true,
    `[data-liquid-menu="${menuId}"]`,
    borderRadius,
    menuId,
    undefined,
    undefined,
    spring,
    backdrop,
  );

  return (
    <DropdownMenu.Root open={open} onOpenChange={onOpenChange}>
      <DropdownMenu.Trigger asChild>{trigger}</DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          className="liquid-menu-content"
          data-liquid-menu={menuId}
          side="top"
          align={align}
          sideOffset={13}
          style={{ width, borderRadius }}
        >
          <LiquidMenuItems items={items} />
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
