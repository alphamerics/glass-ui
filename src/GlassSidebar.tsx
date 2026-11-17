import { Search, X } from "lucide-react";
import { useId, useMemo, useState, type ReactNode } from "react";

/** One navigation entry of the sidebar. */
export interface GlassSidebarItem {
  id: string;
  label: string;
  /** Any icon markup (a lucide icon, a colored tile, an image...). */
  icon?: ReactNode;
  /** Red circular badge on the trailing edge — a count, "!" or similar. */
  badge?: string | number;
  /** Items sharing a group are rendered together, under an optional
      caption, with a hairline divider between groups. */
  group?: string;
  /** Extra search terms (beyond the label) for the built-in search. */
  keywords?: string[];
}

export interface GlassSidebarProps {
  items: GlassSidebarItem[];
  activeId?: string;
  onSelect?: (id: string) => void;
  /** Controlled search query. Omit for internal (uncontrolled) state. */
  search?: string;
  /** Fires on every keystroke so the host can drive its content area. */
  onSearchChange?: (query: string) => void;
  placeholder?: string;
  /** Overrides the default case-insensitive label/keyword match. */
  filter?: (item: GlassSidebarItem, query: string) => boolean;
  /** Content pinned under the list (a profile row, footer links...). */
  header?: ReactNode;
  footer?: ReactNode;
  className?: string;
}

/** Groups consecutive same-group items while keeping their order. */
function collectGroups(items: GlassSidebarItem[]) {
  const groups: Array<{ name?: string; items: GlassSidebarItem[] }> = [];
  for (const item of items) {
    const current = groups[groups.length - 1];
    if (current && current.name === item.group) {
      current.items.push(item);
    } else {
      groups.push({ name: item.group, items: [item] });
    }
  }
  return groups;
}

/**
 * A macOS System Settings-style navigation sidebar: rounded search field on
 * top, then groups of items with icons and trailing badges; the active item
 * is a filled accent pill. No window chrome. The built-in search filters the
 * list and reports every keystroke via `onSearchChange` (or behaves fully
 * controlled when `search` is passed) so the host can filter the content
 * page it navigates.
 */
export function GlassSidebar({
  items,
  activeId,
  onSelect,
  search,
  onSearchChange,
  placeholder = "Search",
  filter,
  header,
  footer,
  className,
}: GlassSidebarProps) {
  const [internalQuery, setInternalQuery] = useState("");
  const query = search ?? internalQuery;
  const listId = useId();

  const setQuery = (next: string) => {
    if (search === undefined) {
      setInternalQuery(next);
    }
    onSearchChange?.(next);
  };

  const matches = (item: GlassSidebarItem) => {
    if (filter) {
      return filter(item, query);
    }
    const haystack = [item.label, ...(item.keywords ?? [])]
      .join(" ")
      .toLowerCase();
    return query
      .toLowerCase()
      .split(/\s+/)
      .every((term) => haystack.includes(term));
  };

  const visible = useMemo(
    () => (query.trim() ? items.filter(matches) : items),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [items, query, filter],
  );
  const groups = useMemo(() => collectGroups(visible), [visible]);

  return (
    <aside className={`glass-sidebar${className ? ` ${className}` : ""}`}>
      <label className="glass-sidebar-search glass-input">
        <Search size={15} strokeWidth={2.2} aria-hidden />
        <input
          type="search"
          role="searchbox"
          aria-label="Search"
          aria-controls={listId}
          value={query}
          onChange={(event) => setQuery(event.currentTarget.value)}
          placeholder={placeholder}
        />
        {query.length > 0 && (
          <button
            type="button"
            className="glass-sidebar-search-clear"
            aria-label="Clear search"
            onClick={() => setQuery("")}
          >
            <X size={12} strokeWidth={2.2} aria-hidden />
          </button>
        )}
      </label>

      {header}

      <nav className="glass-sidebar-groups" id={listId} aria-label="Pages">
        {groups.map((group, groupIndex) => (
          <div
            className="glass-sidebar-group"
            key={group.name ?? `group-${groupIndex}`}
            role="presentation"
          >
            {group.name && (
              <span className="glass-sidebar-group-label">{group.name}</span>
            )}
            {group.items.map((item) => (
              <button
                key={item.id}
                type="button"
                className="glass-sidebar-item"
                data-active={item.id === activeId}
                aria-current={item.id === activeId || undefined}
                onClick={() => onSelect?.(item.id)}
              >
                {item.icon && (
                  <span className="glass-sidebar-item-icon" aria-hidden>
                    {item.icon}
                  </span>
                )}
                <span className="glass-sidebar-item-label">{item.label}</span>
                {item.badge !== undefined && item.badge !== "" && (
                  <span className="glass-sidebar-item-badge">{item.badge}</span>
                )}
              </button>
            ))}
          </div>
        ))}
        {groups.length === 0 && (
          <p className="glass-sidebar-empty">No results</p>
        )}
      </nav>

      {footer}
    </aside>
  );
}
