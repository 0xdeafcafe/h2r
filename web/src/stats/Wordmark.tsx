// The stats site's "h2 live" wordmark, drawn here as SVG strokes: squared
// lower-case letters on a 30px cap line with a split chrome fill. No font is
// involved, so it stays crisp at any size. Three variants are kept so the
// owner can choose; `WORDMARK` is the one the site uses.

import { createUniqueId, type JSX } from "solid-js";

export type WordmarkVariant = "chrome" | "split" | "plate";
export const WORDMARK: WordmarkVariant = "chrome";

// Centre lines on a grid: ascender 4, x-height 14, baseline 30.
const H = "M3 4V30M3 18Q3 14 7 14H13Q17 14 17 18V30";
const TWO = "M25 6H35Q39 6 39 10V14Q39 18 35 18H29Q25 18 25 22V28H39.500";
const lower = (x: number) =>
  `M${x} 4V30M${x + 9} 14V30M${x + 17} 14L${x + 24} 29.500L${x + 31} 14M${x + 39} 22H${x + 51}V18Q${x + 51} 14 ${x + 47} 14H${x + 41}Q${x + 37} 14 ${x + 37} 18V26Q${x + 37} 30 ${x + 41} 30H${x + 51}`;
const dot = (x: number) => `M${x + 9} 4.500V8.500`;
const caps = (x: number, t: number, b: number) => {
  const m = (t + b) / 2;
  return `M${x} ${t}V${b}H${x + 7}M${x + 14} ${t}V${b}M${x + 21} ${t}L${x + 26} ${b}L${x + 31} ${t}M${x + 45} ${t}H${x + 38}V${b}H${x + 45}M${x + 38} ${m}H${x + 44}`;
};

const WIDTH: Record<WordmarkVariant, number> = { chrome: 106, split: 102, plate: 112 };

export default function Wordmark(props: { height?: number; variant?: WordmarkVariant; class?: string }): JSX.Element {
  const id = createUniqueId();
  const v = () => props.variant ?? WORDMARK;
  const h = () => props.height ?? 34;
  const stroke = { fill: "none", "stroke-linejoin": "round", "stroke-linecap": "butt" } as const;
  return (
    <svg class={`wordmark ${props.class ?? ""}`} viewBox={`0 0 ${WIDTH[v()]} 34`} width={(WIDTH[v()] * h()) / 34} height={h()} role="img" aria-label="h2 live">
      <defs>
        <linearGradient id={`${id}c`} gradientUnits="userSpaceOnUse" x1="0" y1="3" x2="0" y2="31">
          <stop offset="0" stop-color="#fff" /><stop offset=".47" stop-color="#c9d2dc" /><stop offset=".5" stop-color="#93a0b0" /><stop offset="1" stop-color="#e4e9ef" />
        </linearGradient>
        <linearGradient id={`${id}b`} gradientUnits="userSpaceOnUse" x1="0" y1="3" x2="0" y2="31">
          <stop offset="0" stop-color="#cfe6fb" /><stop offset=".47" stop-color="#8fbbe3" /><stop offset=".5" stop-color="#5f8fbd" /><stop offset="1" stop-color="#a9cdee" />
        </linearGradient>
      </defs>
      {v() === "chrome" && (
        <>
          <path d={`${H}${TWO}${lower(52)}${dot(52)}`} {...stroke} stroke="#000" stroke-opacity=".7" stroke-width="4.600" transform="translate(0 1.200)" />
          <path d={`${H}${TWO}`} {...stroke} stroke={`url(#${id}c)`} stroke-width="4.600" />
          <path d={`${lower(52)}${dot(52)}`} {...stroke} stroke={`url(#${id}b)`} stroke-width="4.600" />
        </>
      )}
      {v() === "split" && (
        <>
          <path d={`${H}${TWO}`} {...stroke} stroke="#000" stroke-opacity=".7" stroke-width="5.400" transform="translate(0 1.200)" />
          <path d={`${H}${TWO}`} {...stroke} stroke={`url(#${id}c)`} stroke-width="5.400" />
          <path d="M48.500 4V30" {...stroke} stroke="#5d7696" stroke-width="1" />
          <path d={caps(55, 17.500, 29)} {...stroke} stroke="#9fc6ea" stroke-width="2.200" />
        </>
      )}
      {v() === "plate" && (
        <>
          <rect x=".5" y="1.500" width="45" height="31" rx="3.500" fill={`url(#${id}c)`} stroke="#0a1420" />
          <rect x="2" y="3" width="42" height="28" rx="2.500" fill="none" stroke="#fff" stroke-opacity=".55" />
          <path d={`${H}${TWO}`} {...stroke} stroke="#0b1826" stroke-width="4.400" transform="translate(2.500 0)" />
          <path d={`${lower(56)}${dot(56)}`} {...stroke} stroke="#000" stroke-opacity=".7" stroke-width="4.200" transform="translate(0 1.200)" />
          <path d={`${lower(56)}${dot(56)}`} {...stroke} stroke={`url(#${id}c)`} stroke-width="4.200" />
        </>
      )}
    </svg>
  );
}
