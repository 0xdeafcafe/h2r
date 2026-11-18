// Play / pause, the scrubber and the speed for a replay session: the same
// controls wherever the replay is shown, all driving the one clock. The
// scrubber carries the timeline's marks (kills, deaths, revert cuts, zone
// changes); a kill or death mark plays its killcam. `replayKey` gives the
// pages hosting a replay the same keys.

import { For, Show } from "solid-js";
import { clock } from "../lib/format";
import { TICKS_PER_SECOND } from "../state/frames";
import type { ReplayTimeline, TimelineMark } from "../state/replay";
import { SPEEDS, type ReplaySession } from "./session";

const MARK_TITLES: Record<TimelineMark["kind"], string> = {
  kill: "Kill: click to watch it",
  death: "Death: click to watch it",
  cut: "The game went back to a checkpoint or restarted here",
  zone: "The level changed zone here",
};

/** The keys a replay answers, wherever it is shown: Space plays, `,` and
 *  `.` step a recorded frame, `<` and `>` jump between marks, `k` watches
 *  the latest kill. True when the key was the replay's. */
export function replayKey(s: ReplaySession, e: KeyboardEvent): boolean {
  if (e.ctrlKey || e.metaKey || e.altKey) return false;
  const tl = s.timeline();
  switch (e.key) {
    case " ": s.toggle(); break;
    case ",": s.stepFrame(-1); break;
    case ".": s.stepFrame(1); break;
    case "<": s.stepMark(-1); break;
    case ">": s.stepMark(1); break;
    case "k": {
      const k = tl?.killNear(s.tick());
      if (!k) return false;
      s.watchKill(k);
      break;
    }
    default: return false;
  }
  e.preventDefault();
  return true;
}

export default function Transport(props: { session: ReplaySession; killColour?: (killer: number | null) => string | undefined }) {
  const s = props.session;
  const at = (tl: ReplayTimeline, tick: number) => `${((tick - tl.startTick) / Math.max(1, tl.durationTicks)) * 100}%`;
  return (
    <Show when={s.timeline()}>
      {(tl) => (
        <div class="replay-bar" role="group" aria-label="Replay controls">
          <button class="step" onClick={() => s.stepMark(-1)} aria-label="Previous mark" title="Previous kill, death or cut. Key: <">⏮</button>
          <button class="step" onClick={() => s.stepFrame(-1)} aria-label="Back one frame" title="Back one recorded frame. Key: ,">◀︎</button>
          <button class="play" onClick={() => s.toggle()} aria-label={s.playing() ? "Pause" : "Play"}>{s.playing() ? "❚❚" : "▶"}</button>
          <button class="step" onClick={() => s.stepFrame(1)} aria-label="Forward one frame" title="Forward one recorded frame. Key: .">▶︎</button>
          <button class="step" onClick={() => s.stepMark(1)} aria-label="Next mark" title="Next kill, death or cut. Key: >">⏭</button>
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
            <div class="scrub__kills">
              <For each={tl().marks}>
                {(m) => (
                  <Show
                    when={m.kill}
                    fallback={<span class={`scrub__mark scrub__mark--${m.kind}`} style={{ left: at(tl(), m.tick) }} title={MARK_TITLES[m.kind]} aria-hidden="true" />}
                  >
                    {(k) => (
                      <button
                        class={`scrub__kill scrub__kill--${m.kind}`}
                        tabindex="-1"
                        style={{ left: at(tl(), m.tick), background: m.kind === "kill" ? props.killColour?.(k().killer) : undefined }}
                        title={`${MARK_TITLES[m.kind]} (${clock((m.tick - tl().startTick) / TICKS_PER_SECOND)}). Key: k`}
                        aria-label={`Watch the ${m.kind} at ${clock((m.tick - tl().startTick) / TICKS_PER_SECOND)}`}
                        onClick={() => s.watchKill(k())}
                      />
                    )}
                  </Show>
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
