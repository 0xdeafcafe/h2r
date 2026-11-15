// The dashboard's furniture: the orb, the big glass pod, info plates,
// the a/b prompts, the tubes in the corners and the small line glyphs.
// All of it is drawn here or comes from /art (original artwork).

import { For, Show, type JSX } from "solid-js";

/** The orb: a glowing core inside drifting jelly shells. It carries no mark. */
export function Orb() {
  return (
    <div class="dx-orb" aria-hidden="true">
      <i class="dx-wisp dx-wisp--1" />
      <i class="dx-wisp dx-wisp--2" />
      <i class="dx-orb-halo" />
      <i class="dx-orb-shell dx-orb-shell--1" />
      <i class="dx-orb-shell dx-orb-shell--2" />
      <i class="dx-orb-shell dx-orb-shell--3" />
      <i class="dx-orb-art" />
      <i class="dx-orb-art dx-orb-art--swirl" />
      <i class="dx-orb-art dx-orb-art--swirl dx-orb-art--swirl2" />
      <i class="dx-orb-core" />
      <i class="dx-orb-gloss" />
      <i class="dx-wisp dx-wisp--3" />
    </div>
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

export function Prompts(props: { onA?: () => void; onB?: () => void; aLabel?: string }) {
  return (
    <>
      <Show when={props.onB}>
        <button class="dx-prompt dx-prompt--b" onClick={() => props.onB?.()}><span class="dx-btn dx-btn--b" aria-hidden="true">b</span><span class="dx-prompt-label">back</span></button>
      </Show>
      <Show when={props.onA}>
        <button class="dx-prompt dx-prompt--a" onClick={() => props.onA?.()}><span class="dx-btn dx-btn--a" aria-hidden="true">a</span><span class="dx-prompt-label">{props.aLabel ?? "select"}</span></button>
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

export type GlyphName = "record" | "history" | "download" | "exit" | "compare" | "career" | "key" | "sign" | "store" | "server" | "globe" | "empty";

const GLYPHS: Record<GlyphName, string> = {
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
  empty: "M8 12H16",
};

export function Glyph(props: { name: GlyphName }) {
  return (
    <svg class={`dx-glyph dx-glyph--${props.name}`} viewBox="0 0 24 24" aria-hidden="true">
      <path d={GLYPHS[props.name]} />
    </svg>
  );
}
