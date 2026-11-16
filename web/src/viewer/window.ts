// Where the 3D viewer's window sits on the page. Pure.

export interface Rect { x: number; y: number; w: number; h: number }

export const MIN_W = 320, MIN_H = 260;

/** A window rectangle that fits the viewport: sized between the minimum and
 *  the viewport (less a margin), and moved so all of it is on screen. With
 *  no rectangle yet, a default in the lower right. */
export function placeWindow(r: Rect | null, view: { w: number; h: number }, margin = 8): Rect {
  const maxW = Math.max(MIN_W, view.w - margin * 2), maxH = Math.max(MIN_H, view.h - margin * 2);
  if (!r) {
    const w = Math.min(maxW, 760), h = Math.min(maxH, 520);
    return { x: Math.max(margin, view.w - w - 24), y: Math.max(margin, view.h - h - 24), w, h };
  }
  const w = Math.min(maxW, Math.max(MIN_W, r.w)), h = Math.min(maxH, Math.max(MIN_H, r.h));
  const x = Math.min(Math.max(margin, r.x), Math.max(margin, view.w - w - margin));
  const y = Math.min(Math.max(margin, r.y), Math.max(margin, view.h - h - margin));
  return { x, y, w, h };
}
