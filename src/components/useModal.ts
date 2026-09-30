import { useEffect, useRef, type RefObject } from 'react';

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Open modals, topmost last. Only the topmost one reacts to Escape / Tab. */
const stack: symbol[] = [];
let scrollLocks = 0;

function scrollRoot(): HTMLElement | null {
  return document.querySelector<HTMLElement>('[data-scroll-root]');
}

function lockScroll(): () => void {
  const el = scrollRoot();
  if (scrollLocks++ === 0 && el) {
    el.dataset.prevOverflow = el.style.overflowY;
    el.style.overflowY = 'hidden';
  }
  return () => {
    if (--scrollLocks === 0) {
      const root = scrollRoot();
      if (root) {
        root.style.overflowY = root.dataset.prevOverflow ?? '';
        delete root.dataset.prevOverflow;
      }
    }
  };
}

export function focusables(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => el.tabIndex >= 0 && !el.hasAttribute('disabled') && el.getAttribute('aria-hidden') !== 'true',
  );
}

/**
 * Modal behaviour for a dialog element: focus moves into it on open (to `initialFocus` when
 * given, else the first control, else the dialog itself), Tab / Shift+Tab stay inside, Escape
 * closes the topmost modal only, the page behind cannot scroll, and focus returns to the
 * previously focused element on close.
 */
export function useModal(
  open: boolean,
  onClose: () => void,
  dialogRef: RefObject<HTMLElement | null>,
  initialFocus?: RefObject<HTMLElement | null>,
): void {
  const closeRef = useRef(onClose);
  useEffect(() => {
    closeRef.current = onClose;
  });

  useEffect(() => {
    if (!open) return;
    const id = Symbol('modal');
    stack.push(id);
    const previous = document.activeElement as HTMLElement | null;
    const unlock = lockScroll();
    const dialog = dialogRef.current;
    if (dialog) {
      const target = initialFocus?.current ?? focusables(dialog)[0] ?? dialog;
      target.focus({ preventScroll: true });
    }

    const onKey = (e: KeyboardEvent) => {
      if (stack[stack.length - 1] !== id) return;
      if (e.key === 'Escape') {
        e.preventDefault();
        closeRef.current();
        return;
      }
      if (e.key !== 'Tab' || !dialogRef.current) return;
      const items = focusables(dialogRef.current);
      if (items.length === 0) {
        e.preventDefault();
        dialogRef.current.focus();
        return;
      }
      const first = items[0]!;
      const last = items[items.length - 1]!;
      const active = document.activeElement;
      if (!dialogRef.current.contains(active)) {
        e.preventDefault();
        (e.shiftKey ? last : first).focus();
      } else if (e.shiftKey && (active === first || active === dialogRef.current)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('keydown', onKey);
      const i = stack.indexOf(id);
      if (i >= 0) stack.splice(i, 1);
      unlock();
      if (previous && document.contains(previous)) previous.focus({ preventScroll: true });
    };
  }, [open, dialogRef, initialFocus]);
}
