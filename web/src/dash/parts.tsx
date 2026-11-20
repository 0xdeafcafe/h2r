// The dashboard's furniture: the orb, the big glass pod, info plates,
// the a/b prompts, the tubes in the corners and the small line glyphs.
// All of it is drawn here or comes from /art (original artwork). The
// shell's styles (background, orb, menu, prompts, transitions) are in
// styles/dash-shell.css.

import { For, Match, Show, Switch, type JSX } from "solid-js";
import { inputMode } from "./input";
import { blobPath } from "./shape";
import "../styles/dash-shell.css";

/** Jelly outlines for the orb, the same on every load. */
const MEMBRANES = {
  back1: blobPath(11, 5, 121, 0.17),
  back2: blobPath(23, 7, 117, 0.13),
  front1: blobPath(5, 6, 107, 0.09),
  front2: blobPath(42, 9, 108, 0.07),
};

/** The orb: a soft yellow-green ball wrapped in drifting jelly. It carries no mark.
 *  `turn` (degrees) turns its skins with the menu's ring; the light on the ball stays put. */
export function Orb(props: { turn?: number }) {
  const turned = () => `rotate(${(props.turn ?? 0).toFixed(2)})`;
  return (
    <div class="dx-orb" aria-hidden="true">
      <i class="dx-orb-halo" />
      <svg class="dx-orb-svg" viewBox="-170 -170 340 340">
        <defs>
          <radialGradient id="dxo-back" gradientUnits="userSpaceOnUse" cx="0" cy="0" r="128">
            <stop offset=".5" stop-color="#0c4a08" stop-opacity=".5" />
            <stop offset=".8" stop-color="#17720f" stop-opacity=".8" />
            <stop offset="1" stop-color="#3aa822" stop-opacity=".62" />
          </radialGradient>
          <radialGradient id="dxo-front" gradientUnits="userSpaceOnUse" cx="0" cy="0" r="120">
            <stop offset=".6" stop-color="#7fe040" stop-opacity="0" />
            <stop offset=".82" stop-color="#58c82c" stop-opacity=".18" />
            <stop offset=".94" stop-color="#7fe644" stop-opacity=".36" />
            <stop offset="1" stop-color="#c8ff90" stop-opacity=".2" />
          </radialGradient>
          <linearGradient id="dxo-ribbon" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stop-color="#6fd838" stop-opacity=".34" />
            <stop offset="1" stop-color="#1c6f10" stop-opacity="0" />
          </linearGradient>
        </defs>
        <g transform={turned()}>
        <g class="dx-orb-ribbons">
          <path d="M-64 78C-120 150 -150 200 -128 268C-96 214 -40 170 30 112Z" fill="url(#dxo-ribbon)" />
          <path d="M40 -98C96 -150 150 -160 214 -128" />
        </g>
        <g class="dx-mem dx-mem--back1"><path d={MEMBRANES.back1} fill="url(#dxo-back)" /><path class="dx-mem-fold" d={MEMBRANES.back1} /></g>
        <g class="dx-mem dx-mem--back2"><path d={MEMBRANES.back2} fill="url(#dxo-back)" /><path class="dx-mem-fold" d={MEMBRANES.back2} /></g>
        </g>
        <image class="dx-orb-core" href="/art/dashboard/orb.webp" x="-114" y="-114" width="228" height="228" />
        <g transform={turned()}>
        <g class="dx-mem dx-mem--front1"><path d={MEMBRANES.front1} fill="url(#dxo-front)" /><path class="dx-mem-fold" d={MEMBRANES.front1} /></g>
        <g class="dx-mem dx-mem--front2"><path d={MEMBRANES.front2} fill="url(#dxo-front)" /></g>
        </g>
      </svg>
    </div>
  );
}

/** A menu pod: a thick, soft, slightly lumpy ring of green jelly round a dark
 *  mottled hole, which fills with pale light when its item is in focus. Each
 *  seed gives a different wobble. Drawn in a 100-unit box, the ring 70 across. */
export function Pod(props: { seed: number }) {
  const id = `dxp${props.seed}`;
  const u = (name: string) => `url(#${id}-${name})`;
  return (
    <span class="dx-pod" aria-hidden="true">
      <svg viewBox="-50 -50 100 100">
        <defs>
          <filter id={`${id}-goo`} x="-25%" y="-25%" width="150%" height="150%">
            <feTurbulence type="fractalNoise" baseFrequency="0.05" numOctaves="2" seed={props.seed * 7 + 3} result="n" />
            <feDisplacementMap in="SourceGraphic" in2="n" scale="8" xChannelSelector="R" yChannelSelector="G" />
            <feGaussianBlur stdDeviation="0.75" />
          </filter>
          <filter id={`${id}-mottle`} x="0" y="0" width="100%" height="100%">
            <feTurbulence type="fractalNoise" baseFrequency="0.085" numOctaves="3" seed={props.seed * 5 + 11} />
            <feColorMatrix type="matrix" values="0 0 0 0 0.1  0 0 0 0 0.34  0 0 0 0 0.07  2.2 0 0 0 -0.95" />
            <feComposite in2="SourceGraphic" operator="in" />
            <feGaussianBlur stdDeviation="0.9" />
          </filter>
          <filter id={`${id}-soft`} x="-40%" y="-40%" width="180%" height="180%"><feGaussianBlur stdDeviation="4" /></filter>
          <filter id={`${id}-haze`} x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="2.2" /></filter>
          <radialGradient id={`${id}-hole`}>
            <stop offset="0" stop-color="#010801" />
            <stop offset=".62" stop-color="#051f06" />
            <stop offset="1" stop-color="#14480d" />
          </radialGradient>
          <radialGradient id={`${id}-rim`} gradientUnits="userSpaceOnUse" cx="-3" cy="-4" r="38">
            <stop offset=".58" stop-color="#1f5f12" stop-opacity=".85" />
            <stop offset=".68" stop-color="#5fa434" stop-opacity=".82" />
            <stop offset=".8" stop-color="#9ccc64" stop-opacity=".8" />
            <stop offset=".9" stop-color="#7ab646" stop-opacity=".76" />
            <stop offset="1" stop-color="#3f8a22" stop-opacity=".55" />
          </radialGradient>
          <radialGradient id={`${id}-lit`}>
            <stop offset="0" stop-color="#f4ffab" />
            <stop offset=".5" stop-color="#d3ee71" />
            <stop offset=".8" stop-color="#a3d04e" stop-opacity=".85" />
            <stop offset="1" stop-color="#86a85a" stop-opacity="0" />
          </radialGradient>
        </defs>
        <circle class="dx-pod-bloom" r="30" filter={u("soft")} />
        <circle r="26" fill={u("hole")} />
        <circle r="26" fill="#fff" filter={u("mottle")} />
        <g filter={u("goo")}>
          <g transform={`rotate(${props.seed * 67})`}>
            <path class="dx-pod-swirl" d="M-11 5A12 12 0 0 1 7 -9M-3 13A9 9 0 0 0 12 1" />
            <circle class="dx-pod-bud" cx="27" cy="23" r="5.5" />
            <circle class="dx-pod-bud" cx="-33" cy="-12" r="3.2" />
          </g>
          <circle r="29.5" fill="none" stroke={u("rim")} stroke-width="11.5" />
          <g transform={`rotate(${props.seed * 113 + 40})`}>
            <path class="dx-pod-fold" d="M26 -8A27.5 27.5 0 0 1 18 20M-31 6A32 32 0 0 0 -18 27M4 33.5A33.5 33.5 0 0 0 20 27" />
          </g>
          <path class="dx-pod-shine" d="M-29 -10A30.5 30.5 0 0 1 -4 -30.5" />
        </g>
        <circle class="dx-pod-lit" r="25.5" fill={u("lit")} filter={u("haze")} />
      </svg>
    </span>
  );
}

/** The tendril from a pod to its bar: a faint wire that wanders up off the
 *  pod with a few kinks and side shoots, then drops (as far as the bar hangs)
 *  onto the corner of the bar's bracket. */
export function Vine() {
  return (
    <span class="dx-vine" aria-hidden="true">
      <svg viewBox="0 0 36 26">
        <path d="M2 23L6 17L5 11L10 7L16 8L18 4L24 5L28 9V14" />
        <path d="M10 7L9 2H13M18 4L21 1M24 5L31 4L33 7M5 11L1 10" />
        <path class="dx-vine-node" d="M4.5 16.5H7.5V19.5H4.5ZM22.5 3.5H25.5V6.5H22.5Z" />
      </svg>
      <i />
    </span>
  );
}

/** The open stepped bracket that sits behind a bar's top left corner, outline only. */
export function Bracket() {
  return (
    <svg class="dx-bracket" viewBox="-12 -13 124 56" aria-hidden="true">
      <path d="M-7 24V-5L-3 -9H86L95 0" />
      <path d="M-7 28V33L-2 38H26" />
      <path class="dx-bracket-in" d="M-2 -4.5H80" />
    </svg>
  );
}

/** Latitude and longitude lines of a sphere, seen slightly from above. */
function WireSphere() {
  const longs = [8, 22, 35, 45];
  const lats = [-36, -24, -12, 0, 12, 24, 36];
  return (
    <svg class="dx-wiresphere" viewBox="-50 -50 100 100" aria-hidden="true">
      <For each={longs}>{(rx) => <ellipse cx="0" cy="0" rx={rx} ry="47" />}</For>
      <line x1="0" y1="-47" x2="0" y2="47" />
      <For each={lats}>{(y) => {
        const half = Math.sqrt(47 * 47 - y * y);
        return <ellipse cx="0" cy={y} rx={half} ry={half * 0.16} />;
      }}</For>
    </svg>
  );
}

export function BigPod(props: { children: JSX.Element; wire?: boolean }) {
  return (
    <div class="dx-bigpod" classList={{ "dx-bigpod--wire": props.wire }}>
      <i class="dx-bigpod-back" aria-hidden="true" />
      <Show when={props.wire}><WireSphere /></Show>
      <div class="dx-bigpod-in">{props.children}</div>
      <i class="dx-bigpod-glass" aria-hidden="true" />
    </div>
  );
}

export function Plate(props: { small: string; title: string; main?: boolean; children: JSX.Element }) {
  return (
    <section class="dx-plate" classList={{ "dx-plate--main": props.main }}>
      <i class="dx-plate-frame" aria-hidden="true" />
      <small>{props.small}</small>
      <h2>{props.title}</h2>
      <div class="dx-plate-body">{props.children}</div>
    </section>
  );
}

const PROMPT_KEYS = { a: "enter", b: "esc", x: "x" };

/** The prompt's face follows the last input device (see input.ts). */
export function PromptLabel(props: { pad: keyof typeof PROMPT_KEYS; children: JSX.Element }) {
  return (
    <>
      <span class={`dx-btn dx-btn--${props.pad} dx-btn--${inputMode()}`} aria-hidden="true">
        <Switch>
          <Match when={inputMode() === "gamepad"}><b>{props.pad}</b></Match>
          <Match when={inputMode() === "keyboard"}><b>{PROMPT_KEYS[props.pad]}</b></Match>
          <Match when={inputMode() === "pointer"}>{props.pad === "x" ? <b>x</b> : <i class="dx-btn-arrow" />}</Match>
        </Switch>
      </span>
      <span class="dx-prompt-label">{props.children}</span>
    </>
  );
}

export function Prompts(props: { onA?: () => void; onB?: () => void; onX?: () => void; aLabel?: string; bLabel?: string; xLabel?: string }) {
  return (
    <>
      <Show when={props.onB}>
        <button class="dx-prompt dx-prompt--b" onClick={() => props.onB?.()}><PromptLabel pad="b">{props.bLabel ?? "back"}</PromptLabel></button>
      </Show>
      <Show when={props.onX}>
        <button class="dx-prompt dx-prompt--x" onClick={() => props.onX?.()}><PromptLabel pad="x">{props.xLabel ?? "more"}</PromptLabel></button>
      </Show>
      <Show when={props.onA}>
        <button class="dx-prompt dx-prompt--a" onClick={() => props.onA?.()}><PromptLabel pad="a">{props.aLabel ?? "select"}</PromptLabel></button>
      </Show>
    </>
  );
}

/** Corner ornaments for the list and pod screens: glass tubes from the
 *  art pack, dotted arcs and the thin lines that run to the prompts.
 *  Decoration only; nothing here sits under text. */
export function Tubes() {
  return (
    <div class="dx-orn" aria-hidden="true">
      <i class="dx-orn-art dx-orn-art--twin" />
      <i class="dx-orn-art dx-orn-art--cable" />
      <i class="dx-orn-art dx-orn-art--cable2" />
      <svg class="dx-tubes" viewBox="0 0 1280 720">
        <g class="dx-tubes-dots">
          <path d="M1010 20 C 1110 60 1160 190 1120 330" />
          <path d="M200 700 C 150 600 120 420 170 300" />
        </g>
        <g class="dx-tubes-line">
          <path d="M-160 560 C -20 560 150 590 205 634" />
          <path d="M1440 560 C 1300 560 1100 590 1047 634" />
          <path d="M1076 672 C 1160 704 1260 690 1440 720" />
          <path d="M176 672 C 90 704 -40 690 -160 720" />
        </g>
      </svg>
    </div>
  );
}

export type GlyphName = "disc" | "record" | "history" | "download" | "exit" | "compare" | "career" | "key" | "sign" | "store" | "server" | "globe" | "gear" | "empty";

const GLYPHS: Record<GlyphName, string> = {
  disc: "M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0ZM14 12a2 2 0 1 1-4 0 2 2 0 0 1 4 0ZM6 12a6 6 0 0 1 6-6",
  record: "M5 19V11M10 19V5M15 19V9M20 19V14M3 21H22",
  history: "M12 6V12L16 14M12 3A9 9 0 1 1 3.5 9M3 4V9H8",
  download: "M12 3V15M7 10.5L12 15.5L17 10.5M4 20H20",
  exit: "M10 4H5V20H10M10 12H21M17 8L21 12L17 16",
  compare: "M8 4V20M16 4V20M3 9L8 4L13 9M11 15L16 20L21 15",
  career: "M12 3L14.6 9H21L15.8 12.8L17.8 19L12 15.2L6.2 19L8.2 12.8L3 9H9.4Z",
  key: "M14.5 9.5A4.5 4.5 0 1 1 14.4 9.4M14.5 12H22M19 12V15.5M22 12V14.5",
  sign: "M4 20L8.5 19L20 7.5L16.5 4L5 15.5ZM14 6.5L17.5 10M4 23H20",
  store: "M3 5H21V19H3ZM3 9H21M6 7H6.1M9 7H9.1",
  server: "M4 4H20V10H4ZM4 14H20V20H4ZM7 7H7.1M7 17H7.1M12 10V14",
  globe: "M12 3A9 9 0 1 1 11.9 3M3 12H21M12 3C8 7 8 17 12 21M12 3C16 7 16 17 12 21",
  gear: "M12 8.5A3.5 3.5 0 1 1 11.9 8.5M12 2.5V5.5M12 18.5V21.5M2.5 12H5.5M18.5 12H21.5M5.3 5.3L7.4 7.4M16.6 16.6L18.7 18.7M5.3 18.7L7.4 16.6M16.6 7.4L18.7 5.3",
  empty: "M8 12H16",
};

export function Glyph(props: { name: GlyphName }) {
  return (
    <svg class={`dx-glyph dx-glyph--${props.name}`} viewBox="0 0 24 24" aria-hidden="true">
      <path d={GLYPHS[props.name]} />
    </svg>
  );
}
