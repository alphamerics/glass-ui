import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { Check } from "lucide-react";

export interface LiquidMenuItem {
  label: string;
  kind?: "item" | "checkbox" | "separator";
  checked?: boolean;
  disabled?: boolean;
  onSelect?: () => void;
  onCheckedChange?: (checked: boolean) => void;
}

interface MenuPrimitives {
  Item: typeof DropdownMenu.Item;
  CheckboxItem: typeof DropdownMenu.CheckboxItem;
  ItemIndicator: typeof DropdownMenu.ItemIndicator;
  Separator: typeof DropdownMenu.Separator;
}

/**
 * Renders a list of menu items with the shared liquid glass styling (hover
 * highlight, check indicator, separators) for any Radix menu namespace —
 * DropdownMenu, ContextMenu, and so on. Items fade in sequentially on
 * mount (each ~45ms after the previous, capped so long lists stay snappy).
 */
export function LiquidMenuItems({
  items,
  primitives = DropdownMenu,
}: {
  items: LiquidMenuItem[];
  primitives?: MenuPrimitives;
}) {
  const delay = (index: number) => ({
    animationDelay: `${Math.min(index * 45, 320)}ms`,
  });
  return (
    <>
      {items.map((item, index) => {
        const key = `${item.label}-${index}`;
        if (item.kind === "separator") {
          return (
            <primitives.Separator
              key={key}
              className="liquid-menu-divider menu-item-fade"
              style={delay(index)}
            />
          );
        }
        if (item.kind === "checkbox") {
          return (
            <primitives.CheckboxItem
              key={key}
              className="liquid-menu-item"
              style={delay(index)}
              checked={item.checked}
              disabled={item.disabled}
              onCheckedChange={(checked) => item.onCheckedChange?.(checked === true)}
            >
              <span>{item.label}</span>
              <primitives.ItemIndicator className="liquid-menu-indicator">
                <Check size={14} strokeWidth={2.4} />
              </primitives.ItemIndicator>
            </primitives.CheckboxItem>
          );
        }
        return (
          <primitives.Item
            key={key}
            className="liquid-menu-item"
            style={delay(index)}
            disabled={item.disabled}
            onSelect={item.onSelect}
          >
            <span className="liquid-menu-label">{item.label}</span>
          </primitives.Item>
        );
      })}
    </>
  );
}
