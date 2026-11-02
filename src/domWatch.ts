/**
 * ONE shared MutationObserver over document.body's subtree, fanning out to
 * every subscriber through a single coalesced double-rAF settle.
 *
 * Without this, each surface-tracking hook attaches its own body-subtree
 * observer — every DOM mutation then runs N observers, N rAF chains and N
 * measurement/IPC pushes per frame. The shared lane coalesces them: one
 * mutation batch, one settle, every subscriber runs once.
 *
 * The double-rAF (settle) is deliberate and shared: Radix/Floating UI and
 * app reflows position elements over several frames, and a single-frame
 * read can measure an unsettled rect.
 */
type Subscriber = () => void;

const subscribers = new Set<Subscriber>();
let observer: MutationObserver | null = null;
let frame = 0;

function scheduleSettled(): void {
  if (frame) {
    return;
  }
  frame = requestAnimationFrame(() => {
    frame = requestAnimationFrame(() => {
      frame = 0;
      // Copy: a subscriber may subscribe/unsubscribe while running.
      for (const subscriber of [...subscribers]) {
        subscriber();
      }
    });
  });
}

/**
 * Subscribes a callback to DOM mutations. Returns the unsubscribe function;
 * the observer tears down when the last subscriber leaves.
 */
export function onDomMutations(subscriber: Subscriber): () => void {
  subscribers.add(subscriber);
  if (!observer) {
    observer = new MutationObserver(scheduleSettled);
    observer.observe(document.body, { childList: true, subtree: true });
  }
  return () => {
    subscribers.delete(subscriber);
    if (subscribers.size === 0 && observer) {
      observer.disconnect();
      observer = null;
      if (frame) {
        cancelAnimationFrame(frame);
        frame = 0;
      }
    }
  };
}
