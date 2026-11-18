import { onCleanup, onMount } from "solid-js";
import { keyAction, watchGamepads, watchInput, type Action } from "../input";

interface Center { x: number; y: number }

// Prefer the next control in the same horizontal band. The rail and main
// panels also share this rule when the layout stacks on a phone.
export function horizontalNeighbor(points: readonly Center[], from: number, dir: 1 | -1): number {
  const origin = points[from];
  if (!origin) return from;
  let best = from, score = Infinity;
  points.forEach((point, index) => {
    const dx = (point.x-origin.x)*dir, dy = Math.abs(point.y-origin.y);
    if (dx < 4 || dy > dx) return;
    const distance = dx + dy*3;
    if (distance < score) { best = index; score = distance; }
  });
  return best;
}

export function useControls(root: () => HTMLElement, back: () => void, compare: (dir: number) => void) {
  function act(action: Action) {
    if (action === "back") { back(); return; }
    if (action === "pageUp" || action === "pageDown") { compare(action === "pageDown" ? 1 : -1); return; }
    const items = Array.from(root().querySelectorAll<HTMLElement>("[data-nav]")).filter(el => el.getClientRects().length && !el.matches(":disabled"));
    const current = document.activeElement as HTMLElement;
    if (action === "select") { if (current instanceof HTMLInputElement) current.form?.requestSubmit(); else current?.click(); return; }
    const index = items.indexOf(current);
    if (index === -1) { items[0]?.focus(); return; }
    // In a scrollable table the stick reads more rows or columns first.
    // At its edge control passes back to the surrounding menu.
    if (current.hasAttribute("data-scroll")) {
      const { scrollLeft, scrollTop } = current;
      if (action === "down" || action === "up") current.scrollTop += action === "down" ? 48 : -48;
      if (action === "right" || action === "left") current.scrollLeft += action === "right" ? 80 : -80;
      if (current.scrollTop !== scrollTop || current.scrollLeft !== scrollLeft) return;
    }
    let next = index;
    if (action === "up" || action === "down") {
      next = Math.min(items.length - 1, Math.max(0, index + (action === "down" ? 1 : -1)));
    } else if (action === "left" || action === "right") {
      const centers = items.map(el => {
        const r = el.getBoundingClientRect();
        return { x: r.left+r.width/2, y: r.top+r.height/2 };
      });
      next = horizontalNeighbor(centers, index, action === "right" ? 1 : -1);
    }
    if (next !== index) items[next]?.focus();
  }
  onMount(() => {
    const key = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || e.isComposing) return;
      if (e.target instanceof HTMLInputElement && !["Escape","ArrowDown","ArrowUp"].includes(e.key)) return;
      const action = keyAction(e.key);
      if (!action || action === "alt") return;
      e.preventDefault();
      act(action);
    };
    window.addEventListener("keydown", key);
    const stopMode = watchInput(), stopPad = watchGamepads(act);
    onCleanup(() => { window.removeEventListener("keydown", key); stopMode(); stopPad(); });
  });
}
