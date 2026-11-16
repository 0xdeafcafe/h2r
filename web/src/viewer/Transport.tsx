// Play / pause, the scrubber and the speed for a replay session: the same
// controls wherever the replay is shown, all driving the one clock.

import { For, Show } from "solid-js";
import { clock } from "../lib/format";
import { TICKS_PER_SECOND } from "../state/frames";
import { SPEEDS, type ReplaySession } from "./session";

export default function Transport(props: { session: ReplaySession; killColour?: (killer: number | null) => string | undefined }) {
  const s = props.session;
  return (
    <Show when={s.timeline()}>
      {(tl) => (
        <div class="replay-bar" role="group" aria-label="Replay controls">
          <button class="play" onClick={() => s.toggle()} aria-label={s.playing() ? "Pause" : "Play"}>{s.playing() ? "❚❚" : "▶"}</button>
          <div class="scrub">
            <input
              type="range"
              min={tl().startTick}
              max={tl().endTick}
              step={1}
              value={s.tick()}
              onInput={(e) => s.seek(+e.currentTarget.value)}
              aria-label="Replay position"
            />
            <div class="scrub__kills" aria-hidden="true">
              <For each={tl().kills}>
                {(k) => (
                  <span
                    class="scrub__kill"
                    style={{
                      left: `${((k.tick - tl().startTick) / Math.max(1, tl().durationTicks)) * 100}%`,
                      background: props.killColour?.(k.killer),
                    }}
                  />
                )}
              </For>
            </div>
          </div>
          <span class="replay-time">{clock((s.tick() - tl().startTick) / TICKS_PER_SECOND)} / {clock(tl().durationSeconds)}</span>
          <select value={s.speed()} onChange={(e) => s.setSpeed(+e.currentTarget.value)} aria-label="Speed">
            <For each={SPEEDS}>{(v) => <option value={v}>{v}×</option>}</For>
          </select>
        </div>
      )}
    </Show>
  );
}
