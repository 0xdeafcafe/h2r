// Original medal icons, one small SVG per derived medal.

import type { MedalId } from "../state/derive";
import { MEDAL_BY_ID } from "../state/derive";

const RIM = ["#8aa0b8", "#d9a72b", "#e05b4b", "#b07cff"];

function Glyph(props: { id: MedalId }) {
  switch (props.id) {
    case "opening-act": return <path d="M30 70 V30 H66 L58 42 L66 54 H38 V70 Z" fill="#fff" />; // flag
    case "last-word": return <path d="M28 36 H72 V60 H50 L38 72 V60 H28 Z" fill="#fff" />; // speech bubble
    case "payback": return <path d="M30 50 A20 20 0 1 1 50 70 L50 60 A10 10 0 1 0 40 50 L46 50 L34 64 L22 50 Z" fill="#fff" />; // return arrow
    case "twofer": return <g fill="#fff"><circle cx="40" cy="50" r="10" /><circle cx="62" cy="50" r="10" /></g>;
    case "hat-trick": return <g fill="#fff"><circle cx="50" cy="36" r="9" /><circle cx="37" cy="58" r="9" /><circle cx="63" cy="58" r="9" /></g>;
    case "full-house": return <g fill="#fff"><rect x="32" y="32" width="15" height="15" /><rect x="53" y="32" width="15" height="15" /><rect x="32" y="53" width="15" height="15" /><rect x="53" y="53" width="15" height="15" /></g>;
    case "encore": return <path d="M50 24 L56 42 L76 42 L60 54 L66 74 L50 62 L34 74 L40 54 L24 42 L44 42 Z" fill="#fff" />;
    case "hot-streak": return <path d="M50 22 C60 36 68 42 64 58 C62 68 54 74 50 74 C42 74 34 66 36 56 C38 48 44 46 44 38 C48 42 50 46 50 50 C54 44 54 32 50 22 Z" fill="#fff" />;
    case "wildfire": return <g fill="#fff"><path d="M38 26 C46 38 50 44 46 58 C44 66 38 70 34 70 C26 68 24 60 28 52 C30 46 34 44 36 36 Z" /><path d="M62 26 C70 38 74 44 70 58 C68 66 62 70 58 70 C50 68 48 60 52 52 C54 46 58 44 60 36 Z" /></g>;
    case "untouchable": return <path d="M50 22 L74 32 V50 C74 62 64 72 50 78 C36 72 26 62 26 50 V32 Z M50 34 L42 50 H50 L46 66 L60 46 H52 L56 34 Z" fill="#fff" fill-rule="evenodd" />;
  }
}

export default function MedalIcon(props: { id: MedalId; size?: number; dim?: boolean }) {
  const def = () => MEDAL_BY_ID[props.id];
  return (
    <svg class="medal" classList={{ "medal--dim": !!props.dim }} width={props.size ?? 28} height={props.size ?? 28} viewBox="0 0 100 100" role="img" aria-label={def().name}>
      <title>{`${def().name}: ${def().blurb} (derived from game events)`}</title>
      <path d="M50 4 L90 27 V73 L50 96 L10 73 V27 Z" fill="#14161a" stroke={RIM[def().tier]} stroke-width="6" />
      <path d="M50 14 L81 32 V68 L50 86 L19 68 V32 Z" fill={RIM[def().tier]} opacity="0.35" />
      <Glyph id={props.id} />
    </svg>
  );
}
