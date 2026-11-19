import { TICKS_PER_SECOND } from "../state/frames";
import { inTickRange, type TickRange } from "../state/replay";

export interface ClipMarks { from_tick: number | null; to_tick: number | null }

export const MIN_CLIP_TICKS = 30 * TICKS_PER_SECOND;
export const MAX_CLIP_TICKS = 5 * 60 * TICKS_PER_SECOND;

/** Check the marks before posting them. The server checks the same limits. */
export function clipRangeError(marks: ClipMarks, bounds: TickRange): string | null {
  const { from_tick: from, to_tick: to } = marks;
  if (from === null || to === null) return "Mark an in point and an out point.";
  if (!Number.isSafeInteger(from) || !Number.isSafeInteger(to) || !inTickRange(from, bounds) || !inTickRange(to, bounds)) {
    return "Choose both points within the recording.";
  }
  if (to <= from) return "The out point must come after the in point.";
  const length = to - from;
  if (length < MIN_CLIP_TICKS || length > MAX_CLIP_TICKS) return "Clips must be 30 seconds to 5 minutes long.";
  return null;
}
