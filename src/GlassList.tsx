import type { ReactNode } from "react";

/**
 * A titled group of rows rendered as one rounded card — the settings-style
 * list container. Children are `GlassListItem`s (or any row markup); rows
 * are separated by hairlines automatically.
 */
export function GlassList({
  title,
  className,
  children,
}: {
  title?: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <section className="glass-list-group">
      {title ? <h2 className="glass-list-title">{title}</h2> : null}
      <div className={`glass-list${className ? ` ${className}` : ""}`}>
        {children}
      </div>
    </section>
  );
}

/**
 * One row of a `GlassList`: text on the left (label with an optional
 * secondary `detail` line and optional `leading` icon), controls on the
 * right (`children` / `trailing`). Rows inside a list get hairline
 * separators between them.
 */
export function GlassListItem({
  label,
  detail,
  leading,
  children,
  className,
  onClick,
}: {
  /** Primary text (any markup). */
  label: ReactNode;
  /** Secondary line under the label. */
  detail?: ReactNode;
  /** Optional leading slot (an icon, avatar, thumbnail). */
  leading?: ReactNode;
  /** Trailing controls (switch, slider, value...). */
  children?: ReactNode;
  className?: string;
  /** Makes the row interactive (role="button", pointer + keyboard). */
  onClick?: () => void;
}) {
  return (
    <div
      className={`glass-list-item${className ? ` ${className}` : ""}`}
      onClick={onClick}
      role={onClick ? "button" : undefined}
      tabIndex={onClick ? 0 : undefined}
      onKeyDown={
        onClick
          ? (event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                onClick();
              }
            }
          : undefined
      }
    >
      {leading && <span className="glass-list-item-leading">{leading}</span>}
      <span className="glass-list-item-text">
        <span className="glass-list-item-label">{label}</span>
        {detail && <span className="glass-list-item-detail">{detail}</span>}
      </span>
      {children && <span className="glass-list-item-trailing">{children}</span>}
    </div>
  );
}
