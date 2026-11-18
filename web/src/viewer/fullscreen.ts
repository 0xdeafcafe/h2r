// Full screen for the game page's viewer: the Fullscreen API on its box
// where the browser has it (Escape leaves, the browser's own way), else
// the box laid over the whole window (an iPhone has no Fullscreen API for
// anything but video), which Escape leaves too. No three.js, no DOM
// globals: the document is passed in, so it can be tested.

/** The parts of a document full screen needs. */
export interface FullscreenDoc {
  fullscreenElement?: Element | null;
  fullscreenEnabled?: boolean;
  exitFullscreen?: () => Promise<void>;
  /** Safari before 16.4. */
  webkitFullscreenElement?: Element | null;
  webkitExitFullscreen?: () => void;
}

/** The parts of an element full screen needs. */
export interface FullscreenEl {
  requestFullscreen?: (opts?: FullscreenOptions) => Promise<void>;
  webkitRequestFullscreen?: () => void;
}

/** How the box is full screen: through the browser, laid over the
 *  window, or not. */
export type FullscreenState = "api" | "window" | null;

/** Whether the browser can put this element full screen itself. */
export function canFullscreen(el: FullscreenEl, doc: FullscreenDoc): boolean {
  if (doc.fullscreenEnabled === false) return false;
  return typeof el.requestFullscreen === "function" || typeof el.webkitRequestFullscreen === "function";
}

/** The element the browser has full screen now, if any. */
export function fullscreenElement(doc: FullscreenDoc): Element | null {
  return doc.fullscreenElement ?? doc.webkitFullscreenElement ?? null;
}

/** Puts `el` full screen, or takes it out: through the API when the
 *  browser has one for it (and it agrees), else over the window. Returns
 *  the state it is in now. */
export async function setFullscreen(el: FullscreenEl & Element, on: boolean, doc: FullscreenDoc, now: FullscreenState): Promise<FullscreenState> {
  if (!on) {
    if (now === "api" && fullscreenElement(doc) === el) {
      try {
        if (doc.exitFullscreen) await doc.exitFullscreen();
        else doc.webkitExitFullscreen?.();
      } catch { /* already out */ }
    }
    return null;
  }
  if (canFullscreen(el, doc)) {
    try {
      if (el.requestFullscreen) await el.requestFullscreen({ navigationUI: "hide" });
      else el.webkitRequestFullscreen!();
      return "api";
    } catch { /* refused (no user gesture, a policy): over the window instead */ }
  }
  return "window";
}

/** The state after the browser says full screen changed: an `api` box
 *  the browser let go of (Escape, its own button) is out. */
export function afterChange(el: Element, doc: FullscreenDoc, now: FullscreenState): FullscreenState {
  if (now === "api" && fullscreenElement(doc) !== el) return null;
  return now;
}
