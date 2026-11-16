// Skull icons, one per Halo 2 skull: a shared bone-coloured skull with a
// small mark on the brow that tells them apart. Drawn here, no game art.
// `SkullBadges` is the row of them a game or a list row shows.

import { For, Show } from "solid-js";
import { skullName, skullTier, skullTitle, tierOrder } from "../lib/skulls";

/** The brow marks, in a 24 by 24 box between x 8 and 16, y 4 and 8.4.
 *  `fill` ones are solid, the rest are strokes. */
const MARKS: Array<{ d: string; fill?: boolean }> = [
  { d: "M12 4.2l2.6 2-2.6 2-2.6-2z" }, // 0 Envy: a faint diamond (camouflage)
  { d: "M12 4l2.2 4.2H9.8z", fill: true }, // 1 Grunt Birthday Party: a party hat
  { d: "M12 4v4.4M10.4 5.6h3.2" }, // 2 Assassins: a dagger
  { d: "M13.2 4l-2.4 2.3h2.4l-2.4 2.2" }, // 3 Thunderstorm: a bolt
  { d: "M12 4.4a1.8 1.8 0 1 0 0 3.6a1.8 1.8 0 1 0 0-3.6z" }, // 4 Famine: an empty bowl
  { d: "M10.5 5.3a1.5 1.5 0 1 1 2.2 1.3c-.5.3-.7.6-.7 1.1M12 8.4v.1" }, // 5 IWHBYD: a question mark
  { d: "M8.8 6.2c1.1-1.7 5.3-1.7 6.4 0c-1.1 1.7-5.3 1.7-6.4 0zM9.4 8.4l5.2-4.4" }, // 6 Blind: a struck-out eye
  { d: "M8.8 6.2q1.55-2.2 3.1 0t3.2 0" }, // 7 Ghost: a wavy line
  { d: "M12 4.5a1.7 1.7 0 1 0 0 3.4a1.7 1.7 0 1 0 0-3.4z", fill: true }, // 8 Black Eye: a dark disc
  { d: "M12 5.4a1.7 1.7 0 1 0 0 3.4a1.7 1.7 0 1 0 0-3.4zM11 4.4h2" }, // 9 Catch: a grenade
  { d: "M8.7 7.7L15.3 4.7M12 5.3a1.3 1.3 0 1 0 0 2.6a1.3 1.3 0 1 0 0-2.6z" }, // 10 Sputnik: a satellite
  { d: "M9.6 4.4h4.8v2.6L12 8.5L9.6 7z" }, // 11 Iron: a shield
  { d: "M9.2 8.2V4.8l1.7 1.5L12 4.4l1.1 1.9l1.7-1.5v3.4z", fill: true }, // 12 Mythic: a crown
  { d: "M8.8 4.4l2.8 2M15.2 4.4l-2.8 2" }, // 13 Angry: slanted brows
  { d: "M10.4 5a1.3 1.3 0 1 0 0 2.6a1.3 1.3 0 1 0 0-2.6zM13.6 5a1.3 1.3 0 1 0 0 2.6a1.3 1.3 0 1 0 0-2.6z" }, // 14 Bandana: a loop, unlimited
];

const HEAD = "M12 2.4c-4.7 0-8 3.2-8 7.6c0 2.6 1.2 4.3 2.8 5.3V20h2.3v-2h1.4v2h3v-2h1.4v2h2.3v-4.7c1.6-1 2.8-2.7 2.8-5.3c0-4.4-3.3-7.6-8-7.6z";

export default function SkullIcon(props: { index: number; size?: number; title?: boolean }) {
  const size = () => props.size ?? 20;
  const mark = () => MARKS[props.index];
  return (
    <svg
      class="skull" classList={{ "skull--gold": skullTier(props.index) === "gold" }} viewBox="0 0 24 24" width={size()} height={size()} role="img"
      aria-label={skullName(props.index)}
    >
      <Show when={props.title !== false}><title>{skullTitle(props.index)}</title></Show>
      <path class="skull-bone" d={HEAD} />
      <circle class="skull-hole" cx="8.7" cy="12.2" r="2" />
      <circle class="skull-hole" cx="15.3" cy="12.2" r="2" />
      <path class="skull-hole" d="M12 14.6l-1 1.8h2z" />
      <Show when={mark()}>
        {(m) => <path class="skull-mark" classList={{ "skull-mark--fill": !!m().fill }} d={m().d} />}
      </Show>
    </svg>
  );
}

/** A row of skull icons for a game, nothing when none were on. `names` writes
 *  the names beside the icons (the game's own page). */
export function SkullBadges(props: { skulls: readonly number[] | null | undefined; size?: number; names?: boolean; label?: string }) {
  return (
    <Show when={props.skulls?.length}>
      <ul class="skulls" classList={{ "skulls--names": !!props.names }} aria-label={props.label ?? "Skulls on"}>
        <For each={tierOrder(props.skulls!)}>
          {(i) => (
            <li title={skullTitle(i)} classList={{ "skulls-gold": skullTier(i) === "gold" }}>
              <SkullIcon index={i} size={props.size} title={false} />
              <Show when={props.names}><span>{skullName(i)}</span></Show>
            </li>
          )}
        </For>
      </ul>
    </Show>
  );
}
