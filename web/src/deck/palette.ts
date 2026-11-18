// The dense dashboard's chart colours (docs/live/web-boundary.md §3.3).
// Charts sit in a recessed well (`WELL`); every categorical slot below was
// run through the dataviz validator against it in dark mode:
//   validate_palette.js "#55a82a,#8a7cf0,#c98500,#2b97c9,#e0607a" --mode dark --surface "#08180a"
//   lightness band, chroma floor, adjacent CVD (worst 12.6), normal-vision
//   floor and 3:1 contrast all pass.
// The order is the safety mechanism: assign slots in order, never cycle,
// never generate a sixth. Adjacent-pair forms only (stacks, lines, bars);
// slots 1 and 3 collide under deuteranopia when any pair can touch, so no
// scatter or small multiples past two series.

/** The chart well every plot is drawn on. */
export const WELL = "#08180a";

/** Categorical slots, in fixed order. */
export const SERIES = ["#55a82a", "#8a7cf0", "#c98500", "#2b97c9", "#e0607a"] as const;

/** A single-series mark (one line, one bar list): the dashboard's own lime. */
export const ACCENT = "#9be34e";
/** De-emphasis: sparklines, "other", the rest when one series is the point. */
export const QUIET = "#46703a";

/** Text tokens; contrast on the well: 17:1, 10:1, 5.6:1. Text never wears a series colour. */
export const INK = { primary: "#e6ffc0", secondary: "#a5ce78", muted: "#6f9a4c" } as const;

/** Furniture: hairline grid and the baseline. */
export const GRID = "#163018";
export const BASELINE = "#2e5a26";

/** Status, reserved: never a series. Always with an icon and a word. */
export const STATUS = { good: "#7fe230", warning: "#f2c230", critical: "#ff5a3c" } as const;

/** The slot for the i-th series of a chart; past the last it is a bug, not a new hue. */
export function seriesColor(i: number): string {
  const c = SERIES[i];
  if (!c) throw new Error(`series ${i + 1}: fold the tail into "other" (${SERIES.length} slots)`);
  return c;
}
