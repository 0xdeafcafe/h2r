// Player emblems: original geometric badges picked deterministically from
// the XUID (or gamertag), a background shape plus a foreground glyph.

import { hueFor } from "../lib/format";

const BACKS = [
  "M50 6 L92 28 L92 72 L50 94 L8 72 L8 28 Z", // hexagon
  "M50 4 L96 50 L50 96 L4 50 Z", // diamond
  "M10 10 H90 V70 L50 94 L10 70 Z", // shield
  "M50 6 A44 44 0 1 1 49.9 6 Z", // circle
  "M14 8 H86 L94 16 V84 L86 92 H14 L6 84 V16 Z", // cut square
];
const GLYPHS = [
  "M50 20 L58 42 L82 42 L62 56 L70 80 L50 66 L30 80 L38 56 L18 42 L42 42 Z", // star
  "M20 64 L50 34 L80 64 L80 50 L50 20 L20 50 Z", // chevron
  "M56 14 L30 54 L48 54 L42 86 L72 42 L54 42 Z", // bolt
  "M50 22 A28 28 0 1 1 49.9 22 Z M50 36 A14 14 0 1 0 50.1 36 Z", // ring
  "M50 18 L84 78 L16 78 Z", // triangle
  "M66 18 A32 32 0 1 0 66 82 A24 24 0 1 1 66 18 Z", // crescent
  "M44 16 H56 V44 H84 V56 H56 V84 H44 V56 H16 V44 H44 Z", // cross
  "M50 16 L60 40 L84 50 L60 60 L50 84 L40 60 L16 50 L40 40 Z", // spark
];

export default function Emblem(props: { seed: string; size?: number; class?: string; title?: string }) {
  const h = () => hueFor(props.seed);
  const back = () => BACKS[h() % BACKS.length]!;
  const glyph = () => GLYPHS[Math.floor(h() / 7) % GLYPHS.length]!;
  const c1 = () => `hsl(${(h() * 7) % 360} 55% 38%)`;
  const c2 = () => `hsl(${(h() * 13 + 140) % 360} 85% 70%)`;
  return (
    <svg class={`emblem ${props.class ?? ""}`} width={props.size ?? 48} height={props.size ?? 48} viewBox="0 0 100 100" role="img" aria-label={props.title ?? "player emblem"}>
      <rect width="100" height="100" fill="#0b0d10" />
      <path d={back()} fill={c1()} stroke="rgba(255,255,255,.35)" stroke-width="2" />
      <path d={glyph()} fill={c2()} fill-rule="evenodd" stroke="rgba(0,0,0,.5)" stroke-width="1.5" />
    </svg>
  );
}
