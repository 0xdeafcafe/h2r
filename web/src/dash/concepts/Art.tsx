import { For, createUniqueId } from "solid-js";
import type { Section } from "./model";

// Three folded vanes form the relay's own mark.
const vanes = "M7 36 22 8h13L20 36Zm15 7L36 17h12L34 43Zm16-1 10-18h8L46 42Z";
export function Mark() {
  return <svg viewBox="0 0 64 52" fill="currentColor" aria-hidden="true"><path d={vanes}/></svg>;
}

const iconPaths: Record<Section, string> = {
  friends: "M15 14a4 4 0 1 0-8 0 4 4 0 0 0 8 0ZM3 26v-3c0-7 16-7 16 0v3ZM19 9a4 4 0 0 1 0 8m2 2q7 0 7 7h-5",
  messages: "M3 7h26v18H3ZM4 9l12 9L28 9",
  sessions: "M3 7h10v8H3Zm16 0h10v8H19ZM11 21h10v7H11ZM8 15v3h16v-3M16 18v3",
  account: "M2 6h28v21H2Zm12 7a3 3 0 1 0-6 0 3 3 0 0 0 6 0ZM6 22q0-7 10-3l1 3Zm14-9h6m-6 5h6",
  server: "M3 6h26v8H3Zm0 12h26v8H3Zm4-8h2m-2 12h2m5-12h10m-10 12h10",
};
export function Icon(props: { name: Section }) {
  return <svg class="cc-icon" viewBox="0 0 32 32" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round" aria-hidden="true"><path d={iconPaths[props.name]}/></svg>;
}

const range = (n: number) => Array.from({ length: n }, (_, i) => i);
const latitudes = range(13);
const ticks = range(96);
const membranes = [-64, -31, 6, 48, 103];

export function Orb() {
  const id = createUniqueId();
  const fill = (name: string) => `url(#${id}-${name})`;
  return <svg class="cc-orb" viewBox="0 0 400 400" aria-hidden="true">
    <defs>
      <radialGradient id={id+"-halo"}><stop offset=".37" stop-color="#b2ff26" stop-opacity=".48"/><stop offset=".69" stop-color="#83df22" stop-opacity=".22"/><stop offset="1" stop-color="#70db13" stop-opacity="0"/></radialGradient>
      <radialGradient id={id+"-skin"} cx=".39" cy=".34" r=".73">
        <stop stop-color="#d7ff69"/><stop offset=".19" stop-color="#97da29"/><stop offset=".46" stop-color="#4a9714"/><stop offset=".65" stop-color="#1a4c12"/><stop offset=".80" stop-color="#4a8f1b"/><stop offset=".90" stop-color="#d4fd6b"/><stop offset=".94" stop-color="#63a62d"/><stop offset="1" stop-color="#102a0b"/>
      </radialGradient>
      <radialGradient id={id+"-core"} cx=".35" cy=".22" r=".87"><stop stop-color="#f7ffb0"/><stop offset=".33" stop-color="#c4ec4a"/><stop offset=".61" stop-color="#9acc23"/><stop offset=".89" stop-color="#467c10"/><stop offset="1" stop-color="#b6e948"/></radialGradient>
      <linearGradient id={id+"-film"} x2=".8" y2="1"><stop stop-color="#f0ffa4" stop-opacity=".86"/><stop offset=".2" stop-color="#c9ff58" stop-opacity=".1"/><stop offset=".59" stop-color="#548915" stop-opacity=".08"/><stop offset=".82" stop-color="#c9ff68" stop-opacity=".56"/><stop offset="1" stop-color="#f4ffc5" stop-opacity=".95"/></linearGradient>
      <linearGradient id={id+"-etch"} x2=".4" y2="1"><stop stop-color="#294e0f"/><stop offset=".45" stop-color="#658514"/><stop offset="1" stop-color="#f8ffb0"/></linearGradient>
      <clipPath id={id+"-ball"}><circle cx="200" cy="200" r="105"/></clipPath>
    </defs>
    <circle cx="200" cy="200" r="198" fill={fill("halo")}/>
    <g fill="none" stroke="#aeda69">
      <circle cx="200" cy="200" r="167" opacity=".27"/><circle cx="200" cy="200" r="159" stroke-dasharray="145 30 40 420" opacity=".6"/>
      <For each={ticks}>{i => <path d={i % 8 ? "M200 36v3" : "M200 34v8"} transform={`rotate(${i*3.75} 200 200)`} opacity={i % 8 ? ".38" : ".85"}/>}</For>
    </g>
    <g class="cc-orb-light">
      <For each={membranes}>{angle => <path d="M56 192C48 139 137 68 211 70C267 71 298 117 329 158C359 199 341 229 300 253C229 300 156 334 111 296C71 262 61 221 56 192Z" transform={`rotate(${angle} 200 200)`} fill={fill("film")} stroke="#c5ff74" stroke-opacity=".32" stroke-width=".8"/>}</For>
      <circle cx="200" cy="200" r="113" fill={fill("skin")} stroke="#d5ff84" stroke-width="1.5"/>
      <g clip-path={fill("ball")} transform="rotate(-18 200 200)" fill="none" stroke="#e2ff95" opacity=".33">
        <For each={latitudes}>{i => <ellipse cx="200" cy="200" rx={9+i*8} ry="106" stroke-width=".7"/>}</For>
        <For each={latitudes}>{i => <ellipse cx="200" cy={98+i*17} rx="108" ry={11+i*.5} stroke-width=".7"/>}</For>
      </g>
      <circle cx="200" cy="201" r="77" fill={fill("core")} stroke="#efff96" stroke-opacity=".7"/>
      <path d={vanes} transform="translate(130 141) scale(2.15)" fill={fill("etch")} stroke="#d9fa68" stroke-width=".6"/>
      <path d="M100 172C110 105 182 76 238 97C187 88 138 131 129 160Q115 185 100 172Z" fill="#f5ffd1" opacity=".48"/>
      <path d="M262 260Q287 248 296 217Q288 269 251 280Z" fill="#e8ff9c" opacity=".69"/>
      <ellipse cx="142" cy="123" rx="12" ry="4" transform="rotate(-30 142 123)" fill="#fbffdf" opacity=".86"/>
    </g>
  </svg>;
}

export function Cables() {
  return <svg class="cc-cables" viewBox="0 0 1200 500" preserveAspectRatio="none" aria-hidden="true">
    <defs><path id="cc-wire" d="M-30 452C60 442 90 294 195 370S251 516 374 445S482 369 570 402S717 428 800 353S900 272 998 327S1120 428 1240 306 M75 24C170 114 237-30 399 44S597 100 707 76S920 3 1052 70S1141 146 1222 110"/></defs>
    <use href="#cc-wire" fill="none" stroke="#14220c" stroke-width="6"/>
    <use href="#cc-wire" fill="none" stroke="#779b4d" stroke-width="2.2"/>
    <use class="cc-wire-glint" href="#cc-wire" fill="none" stroke="#cfee8a" stroke-width="1" stroke-dasharray="15 210"/>
  </svg>;
}

export function ChromeFrame() {
  const shell = "M9 76C10 8 65 5 123 14C242 30 267 68 453 50S806 0 948 25Q989 33 992 77L986 409Q984 486 929 489C813 499 692 449 526 456S213 511 70 484Q14 476 12 421Z";
  const lip = "M26 85C28 30 75 21 126 32C246 56 287 87 461 68S808 19 946 41Q971 48 975 81L969 409Q965 469 925 471C805 480 706 430 524 438S204 490 74 465Q32 459 29 412Z";
  // Independent caps keep a long portrait panel from stretching its lip over the title.
  return <div class="cc-chrome-frame" aria-hidden="true">
    <svg viewBox="0 0 1000 90" preserveAspectRatio="none">
    <defs>
      <linearGradient id="cc-metal" x1=".1" y1="0" x2=".64" y2="1"><stop stop-color="#faf7ff"/><stop offset=".08" stop-color="#98a39e"/><stop offset=".15" stop-color="#51526a"/><stop offset=".32" stop-color="#b5bac4"/><stop offset=".51" stop-color="#5c6d67"/><stop offset=".72" stop-color="#bfc2cf"/><stop offset=".90" stop-color="#777782"/><stop offset="1" stop-color="#cccbd5"/></linearGradient>
      <linearGradient id="cc-enamel" x2=".48" y2="1"><stop stop-color="#326821"/><stop offset=".42" stop-color="#285b20"/><stop offset=".90" stop-color="#133d1a"/><stop offset="1" stop-color="#08270f"/></linearGradient>
    </defs>
    <g id="cc-casting">
    <path d={shell} transform="translate(0 7)" fill="#363941" stroke="#171d17" stroke-width="6"/>
    <path d={shell} fill="url(#cc-metal)" stroke="#cfd5ca" stroke-width="2"/>
    <path d={lip} fill="url(#cc-enamel)" stroke="#414d48" stroke-width="7"/>
    <path d={lip} fill="none" stroke="#adc2a4" stroke-opacity=".8" stroke-width="1.8"/>
    <path d="M31 66Q50 0 148 24C280 62 301 66 425 55M511 452C670 438 816 484 920 479" fill="none" stroke="#fff" stroke-opacity=".7" stroke-width="2.6"/>
    <For each={range(48)}>{i => {
      const x = 232 + i * 4, y = 48 + 8 * Math.sin(i/26);
      return <path d={`M${x} ${y}l2-5m${450-i*1.5} 406 2 5`} stroke="#3b4541" stroke-width="1.8"/>;
    }}</For>
    </g></svg>
    <svg viewBox="0 90 1000 316" preserveAspectRatio="none"><use href="#cc-casting"/></svg>
    <svg viewBox="0 406 1000 100" preserveAspectRatio="none"><use href="#cc-casting"/></svg>
  </div>;
}

// The bulletin's own landscape: a radio array at the edge of a crater.
export function RelayLandscape() {
  return <svg class="cc-landscape" viewBox="0 0 420 178" preserveAspectRatio="xMidYMid slice" role="img" aria-label="A relay dish above a crater at sunset">
    <defs>
      <linearGradient id="cc-sky" x2=".2" y2="1"><stop stop-color="#353b42"/><stop offset=".55" stop-color="#bd7957"/><stop offset="1" stop-color="#edc688"/></linearGradient>
      <pattern id="cc-print" width="3" height="3" patternUnits="userSpaceOnUse"><path d="M0 1h3" stroke="#181d15" stroke-opacity=".15" stroke-width=".7"/></pattern>
    </defs>
    <path fill="url(#cc-sky)" d="M0 0h420v178H0z"/>
    <circle cx="300" cy="53" r="38" fill="#ffdfa0"/>
    <path d="m0 97 55-19 23 6 64-29 43 16 30-5 73 34 69-28 63 14v92H0" fill="#827665"/>
    <path d="m0 119 92-32 31 4 33-12 38 19 31-7 35 14 63-20 20 11 34-9 43 17v74H0" fill="#575f58"/>
    <path d="m0 135 67-25 55 9 47-18 70 17 49-7 39-14 46 10 47-9v80H0" fill="#314343"/>
    <g fill="#162825" stroke="#162825" stroke-linecap="round">
      <path d="M107 33q18 44 82 59q-61 13-82-59Z"/><path d="m112 41 56 21 19 26m-20-29-7 20" fill="none" stroke-width="2"/>
      <path d="m145 81-18 61h9l15-45 10 45h10l-17-57z"/><path d="m137 116 23 10-27 9" fill="none" stroke-width="2"/>
      <path d="M246 93q12 24 38 30q-31 5-38-30Zm16 27-8 28h6l6-20 3 20h5l-6-27z"/>
    </g>
    <path d="m0 149 57-9 34 5 41-8 48 3 50 11 66-9 39-4 55 2 30-4v42H0" fill="#152722"/>
    <path d="M0 0h420v178H0z" fill="url(#cc-print)"/>
  </svg>;
}
