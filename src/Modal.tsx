import type { ReactNode } from "react";

interface ModalProps {
  open: boolean;
  /** Called for backdrop clicks and Escape — omit to make the modal sticky. */
  onClose?: () => void;
  /** Backdrop clicks close the modal unless this is false. */
  closeOnBackdrop?: boolean;
  title?: ReactNode;
  /** Card width in px. */
  width?: number;
  children?: ReactNode;
  /** Action row rendered at the bottom of the card. */
  footer?: ReactNode;
}

/**
 * A dimmed backdrop with a centered card. Fully presentational: the caller
 * owns the open state and whatever goes into the body and footer.
 */
export function Modal({
  open,
  onClose,
  closeOnBackdrop = true,
  title,
  width = 236,
  children,
  footer,
}: ModalProps) {
  if (!open) {
    return null;
  }
  return (
    <div
      className="modal-backdrop"
      onClick={closeOnBackdrop ? onClose : undefined}
    >
      <div
        className="modal"
        style={{ width }}
        role="dialog"
        aria-modal="true"
        onClick={(event) => event.stopPropagation()}
      >
        {title !== null && title !== undefined && (
          <span className="modal-title">{title}</span>
        )}
        {children !== undefined && <div className="modal-body">{children}</div>}
        {footer !== null && footer !== undefined && (
          <div className="modal-actions">{footer}</div>
        )}
      </div>
    </div>
  );
}
