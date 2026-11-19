// Play / pause, the scrubber and the speed for a replay session: the same
// controls wherever the replay is shown, all driving the one clock. The
// scrubber carries the timeline's marks (kills, deaths, revert cuts, zone
// changes, cutscenes); a kill or death mark plays its killcam. `replayKey` gives the
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
  cinematic: "A cutscene: playing skips it",
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

export default function Transport(props: {
  session: ReplaySession;
  matchStartTick: number;
  killColour?: (killer: number | null) => string | undefined;
}) {
  const s = props.session;
  const at = (tl: ReplayTimeline, tick: number) => `${((tick - tl.startTick) / Math.max(1, tl.durationTicks)) * 100}%`;
  const span = (tl: ReplayTimeline, m: TimelineMark) => `${(((m.to ?? m.tick) - m.tick) / Math.max(1, tl.durationTicks)) * 100}%`;
  const time = (tl: ReplayTimeline, tick: number) => clock((tick - (s.isLive() ? props.matchStartTick : tl.startTick)) / TICKS_PER_SECOND);
  return (
    <Show when={s.timeline()}>
      {(tl) => (
        <div class="replay-bar" role="group" aria-label={s.isLive() ? "Live playback controls" : "Replay controls"}>
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
              aria-label={s.isLive() ? "Live playback position" : "Replay position"}
            />
            <div class="scrub__kills">
              <For each={tl().marks}>
                {(m) => (
                  <Show
                    when={m.kill}
                    fallback={<span class={`scrub__mark scrub__mark--${m.kind}`} style={{ left: at(tl(), m.tick), width: m.kind === "cinematic" ? span(tl(), m) : undefined }} title={MARK_TITLES[m.kind]} aria-hidden="true" />}
                  >
                    {(k) => (
                      <button
                        class={`scrub__kill scrub__kill--${m.kind}`}
                        tabindex="-1"
                        style={{ left: at(tl(), m.tick), background: m.kind === "kill" ? props.killColour?.(k().killer) : undefined }}
                        title={`${MARK_TITLES[m.kind]} (${time(tl(), m.tick)}). Key: k`}
                        aria-label={`Watch the ${m.kind} at ${time(tl(), m.tick)}`}
                        onClick={() => s.watchKill(k())}
                      />
                    )}
                  </Show>
                )}
              </For>
            </div>
          </div>
          <span class="replay-time">{time(tl(), s.tick())}<Show when={!s.isLive()}> / {clock(tl().durationSeconds)}</Show></span>
          <select value={s.speed()} onChange={(e) => s.setSpeed(+e.currentTarget.value)} disabled={s.followingLive()} aria-label="Speed">
            <For each={SPEEDS}>{(v) => <option value={v}>{v}×</option>}</For>
          </select>
          <Show when={s.isLive()}>
            <button type="button" class="replay-live" aria-pressed={s.followingLive()} title="Return to live play" onClick={() => s.goLive()}>Live</button>
          </Show>
        </div>
      )}
    </Show>
  );
}
