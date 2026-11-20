// Weapon silhouettes for the Tool of Destruction rows. Original, generic
// shapes picked by keyword from whatever name the game reported; or, when
// the server has game art for that weapon, its silhouette. Those are single
// masks (the alpha channel carries the shape, faintly), so the picture is
// stretched to full strength and used as a CSS mask over the current text
// colour: it takes the page's tint.

import { Show, createEffect, createSignal, onCleanup } from "solid-js";
import { weaponKey } from "../lib/gameart";
import { gameImage, maskUrl } from "../state/gameart";

const SHAPES: Array<[RegExp, string]> = [
  [/sniper|beam|long/, "M1 9H12L14 7H29V4H41V7H63V9H44L42 12H36L34 18H29L30 12H16L9 15H1Z"],
  [/rocket|launcher|cannon|fuel|brute.shot|turret/, "M3 5H50L61 2V16L50 13H38L36 19H30L31 13H3Z M8 7V11H24V7Z"],
  [/shotgun|scatter/, "M2 6H61V9H42V13H27V10H19L11 17H2Z"],
  [/sword|blade|melee|knife/, "M2 10L39 5V2H43V7H61V13H43V18H39V15Z"],
  [/grenade|frag|plasma_g|bomb|sticky/, "M28 2H37V5H35V6A7.5 7.5 0 1 1 30 6V5H28Z M39 3L45 3L43 8L41 8Z"],
  [/smg|sub|needler|spiker/, "M6 5H47V8H41V10H35L34 19H28V10H21L18 17H12L14 10H6Z"],
  [/pistol|magnum|hand/, "M10 4H43V9H32L30 12H22L20 19H12L14 9H10Z"],
  [/blaster|plasma|carbine|energy/, "M3 8Q20 1 45 4L60 8L45 11Q36 12 31 12L28 19H21L23 12Q10 12 3 8Z"],
  [/rifle|^br$|^ar$|assault|battle/, "M2 8H10V6H44V3H51V6H62V9H50L48 12H40L38 18H32L33 12H14L8 16H2Z"],
  [/vehicle|splatter|ghost|hog|tank|banshee|wraith|scorpion|spectre/, "M6 13L12 6H40L52 10L60 12V15H52A5 5 0 0 0 42 15H24A5 5 0 0 0 14 15H6Z"],
];
const GENERIC = "M22 3H42V6H52V14H42V17H22V14H12V6H22Z M27 7V13H37V7Z";

export default function WeaponIcon(props: { weapon: string; width?: number }) {
  const d = () => {
    const w = props.weapon.toLowerCase();
    return SHAPES.find(([re]) => re.test(w))?.[1] ?? GENERIC;
  };
  const w = () => props.width ?? 54;
  const h = () => Math.round(w() * 20 / 64);
  const [game, setGame] = createSignal<string | null>(null);
  createEffect(() => {
    const ref = gameImage(weaponKey(props.weapon));
    if (!ref) { setGame(null); return; }
    let current = true;
    onCleanup(() => { current = false; });
    void maskUrl(ref).then((url) => { if (current) setGame(url); });
  });
  const mask = (url: string) => `url("${url}") center / contain no-repeat`;
  return (
    <Show when={game()} fallback={<Svg />}>
      {(url) => <span class="weapon-icon weapon-icon--game" aria-hidden="true" style={{ width: `${w()}px`, height: `${h()}px`, mask: mask(url()), "-webkit-mask": mask(url()) }} />}
    </Show>
  );
  function Svg() {
    return (
    <svg class="weapon-icon" width={w()} height={Math.round(w() * 20 / 64)} viewBox="0 0 64 20" aria-hidden="true">
      <path d={d()} fill="currentColor" fill-rule="evenodd" />
    </svg>
    );
  }
}
