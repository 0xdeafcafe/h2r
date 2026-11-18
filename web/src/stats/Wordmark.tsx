// The stats site's "h2 | UPLINK" wordmark (STATS_NAME, src/brand.ts), drawn
// here as SVG strokes: a squared chrome "h2" on a 30px cap line, a thin rule,
// and small spaced capitals in pale blue. No font is involved, so it stays
// crisp at any size. Original art: no real logo is traced.

import { createUniqueId, type JSX } from "solid-js";
import { STATS_NAME } from "../brand";

// Centre lines on a grid: ascender 4, x-height 14, baseline 30.
const H = "M3 4V30M3 18Q3 14 7 14H13Q17 14 17 18V30";
const TWO = "M25 6H35Q39 6 39 10V14Q39 18 35 18H29Q25 18 25 22V28H39.500";

/** "UPLINK" in capitals from `t` to `b`, starting at `x`: letters 8 wide
 *  (L 7, I a stroke) with 6.5 between them, small rounded corners. */
function uplink(x: number, t: number, b: number): string {
  const m = (t + b) / 2;
  const at = [0, 14.5, 29, 42.5, 49, 63.5].map((d) => x + d);
  const [u, p, l, i, n, k] = at as [number, number, number, number, number, number];
  return [
    `M${u} ${t}V${b - 2}Q${u} ${b} ${u + 2} ${b}H${u + 6}Q${u + 8} ${b} ${u + 8} ${b - 2}V${t}`,
    `M${p} ${b}V${t}H${p + 6}Q${p + 8} ${t} ${p + 8} ${t + 2}V${m - 2}Q${p + 8} ${m} ${p + 6} ${m}H${p}`,
    `M${l} ${t}V${b}H${l + 7}`,
    `M${i} ${t}V${b}`,
    `M${n} ${b}V${t}L${n + 8} ${b}V${t}`,
    `M${k} ${t}V${b}M${k + 8} ${t}L${k + 1.500} ${m}L${k + 8} ${b}`,
  ].join("");
}

/** The viewBox's width: the word ends at 55 + 71.5, plus half a stroke and a margin. */
export const WORDMARK_WIDTH = 129;

export default function Wordmark(props: { height?: number; class?: string }): JSX.Element {
  const id = createUniqueId();
  const h = () => props.height ?? 34;
  const stroke = { fill: "none", "stroke-linejoin": "round", "stroke-linecap": "butt" } as const;
  return (
    <svg class={`wordmark ${props.class ?? ""}`} viewBox={`0 0 ${WORDMARK_WIDTH} 34`} width={(WORDMARK_WIDTH * h()) / 34} height={h()} role="img" aria-label={STATS_NAME}>
      <defs>
        <linearGradient id={`${id}c`} gradientUnits="userSpaceOnUse" x1="0" y1="3" x2="0" y2="31">
          <stop offset="0" stop-color="#fff" /><stop offset=".47" stop-color="#c9d2dc" /><stop offset=".5" stop-color="#93a0b0" /><stop offset="1" stop-color="#e4e9ef" />
        </linearGradient>
      </defs>
      <path d={`${H}${TWO}`} {...stroke} stroke="#000" stroke-opacity=".7" stroke-width="5.400" transform="translate(0 1.200)" />
      <path d={`${H}${TWO}`} {...stroke} stroke={`url(#${id}c)`} stroke-width="5.400" />
      <path d="M48.500 4V30" {...stroke} stroke="#5d7696" stroke-width="1" />
      <path d={uplink(55, 17.500, 29)} {...stroke} stroke="#9fc6ea" stroke-width="2.200" />
    </svg>
  );
}
