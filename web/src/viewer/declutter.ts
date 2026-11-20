// Place the viewer's projected name tags. See docs/live/viewer-overlay.md, "Name tags".
export interface TagIn {
  id: number;
  x: number;
  y: number;
  w: number;
  h: number;
  depth: number;
  rank: number;
}

export type TagLod = "full" | "lifted" | "pip";

export interface TagState {
  lod: TagLod;
  pending: TagLod | null;
  since: number;
}

const MAX_TAGS = 64;
const GAP = 2;
const LIFT_GAP = 3;
const HOLD_MS = 250;
const FULL_TOLERANCE = 4;

interface Rect {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

function rectangle(tag: TagIn, lifted: boolean): Rect {
  const bottom = tag.y - (lifted ? tag.h + LIFT_GAP : 0);
  return {
    left: tag.x - tag.w / 2,
    right: tag.x + tag.w / 2,
    top: bottom - tag.h,
    bottom,
  };
}

function placement(full: Rect, lifted: Rect, placed: readonly Rect[], heldFull: boolean): TagLod {
  let fullClear = true;
  let liftedClear = true;
  for (const other of placed) {
    const x = Math.min(full.right, other.right) - Math.max(full.left, other.left);
    if (x <= -GAP) continue;
    const y = Math.min(full.bottom, other.bottom) - Math.max(full.top, other.top);
    if (heldFull ? Math.min(x, y) >= FULL_TOLERANCE : y > -GAP) fullClear = false;
    const liftedY = Math.min(lifted.bottom, other.bottom) - Math.max(lifted.top, other.top);
    if (liftedY > -GAP) liftedClear = false;
    if (!fullClear && !liftedClear) return "pip";
  }
  return fullClear ? "full" : liftedClear ? "lifted" : "pip";
}

function hold(wanted: TagLod, prev: TagState | undefined, now: number): TagState {
  if (!prev || wanted === prev.lod) return { lod: wanted, pending: null, since: now };
  if (wanted !== prev.pending) return { lod: prev.lod, pending: wanted, since: now };
  if (now - prev.since < HOLD_MS) return prev;
  return { lod: wanted, pending: null, since: now };
}

export function declutter(
  tags: readonly TagIn[],
  prev: ReadonlyMap<number, TagState>,
  now: number,
): Map<number, TagState> {
  const ranked = [...tags].sort((a, b) => a.rank - b.rank || a.depth - b.depth || a.id - b.id);
  const placed: Rect[] = [];
  const next = new Map<number, TagState>();
  for (const [index, tag] of ranked.entries()) {
    if (index >= MAX_TAGS) {
      next.set(tag.id, { lod: "pip", pending: null, since: now });
      continue;
    }
    const full = rectangle(tag, false);
    const lifted = rectangle(tag, true);
    const before = prev.get(tag.id);
    const wanted = placement(full, lifted, placed, before?.lod === "full");
    const state = hold(wanted, before, now);
    next.set(tag.id, state);
    if (state.lod === "full") placed.push(full);
    if (state.lod === "lifted") placed.push(lifted);
  }
  return next;
}
