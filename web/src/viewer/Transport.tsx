// Play / pause, the scrubber and the speed for a replay session: the same
// controls wherever the replay is shown, all driving the one clock. The
// scrubber carries the timeline's marks (kills, deaths, revert cuts, zone
// changes, cutscenes); a kill or death mark plays its killcam. `replayKey` gives the
// pages hosting a replay the same keys.

import { For, Show, createMemo } from "solid-js";
import { clock } from "../lib/format";
import { TICKS_PER_SECOND } from "../state/frames";
import { inTickRange, type TickRange, type TimelineMark } from "../state/replay";
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
 *  the latest kill. The arrows seek five seconds. True when the key was the replay's. */
export function replayKey(s: ReplaySession, e: KeyboardEvent): boolean {
  if (e.ctrlKey || e.metaKey || e.altKey) return false;
  const tl = s.timeline();
  switch (e.key) {
    case " ": s.toggle(); break;
    case "ArrowLeft": s.seek(s.tick() - 5 * TICKS_PER_SECOND); break;
    case "ArrowRight": s.seek(s.tick() + 5 * TICKS_PER_SECOND); break;
    case ",": s.stepFrame(-1); break;
    case ".": s.stepFrame(1); break;
    case "<": s.stepMark(-1); break;
    case ">": s.stepMark(1); break;
    case "k": {
      const k = tl?.killNear(s.tick(), s.range?.());
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
  selection?: { from_tick: number | null; to_tick: number | null };
}) {
  const s = props.session;
  const bounds = createMemo((): TickRange => s.range?.() ?? {
    from_tick: s.timeline()?.startTick ?? 0,
    to_tick: s.timeline()?.endTick ?? 0,
  });
  const length = () => bounds().to_tick - bounds().from_tick;
  const percent = (tick: number) => (tick - bounds().from_tick) / Math.max(1, length()) * 100;
  const at = (tick: number) => `${percent(tick)}%`;
  const time = (tick: number) => clock((tick - (s.isLive() ? props.matchStartTick : bounds().from_tick)) / TICKS_PER_SECOND);
  const marks = createMemo(() => {
    const marks = s.timeline()?.marks ?? [];
    const range = s.range?.();
    if (!range) return marks;
    return marks.filter((m) => m.kind === "cinematic"
      ? m.tick <= range.to_tick && (m.to ?? m.tick) >= range.from_tick
      : inTickRange(m.tick, range));
  });
  const markStyle = (m: TimelineMark) => {
    const start = Math.max(bounds().from_tick, m.tick);
    const end = Math.min(bounds().to_tick, m.to ?? m.tick);
    return { left: at(start), width: m.kind === "cinematic" ? `${percent(end) - percent(start)}%` : undefined };
  };
  const inMark = () => props.selection?.from_tick ?? null;
  const outMark = () => props.selection?.to_tick ?? null;
  return (
    <Show when={s.timeline()}>
      <div class="replay-bar" role="group" aria-label={s.isLive() ? "Live playback controls" : "Replay controls"}>
        <button class="step" onClick={() => s.stepMark(-1)} aria-label="Previous mark" title="Previous kill, death or cut. Key: <">⏮</button>
        <button class="step" onClick={() => s.stepFrame(-1)} aria-label="Back one frame" title="Back one recorded frame. Key: ,">◀︎</button>
        <button class="play" onClick={() => s.toggle()} aria-label={s.playing() ? "Pause" : "Play"}>{s.playing() ? "❚❚" : "▶"}</button>
        <button class="step" onClick={() => s.stepFrame(1)} aria-label="Forward one frame" title="Forward one recorded frame. Key: .">▶︎</button>
        <button class="step" onClick={() => s.stepMark(1)} aria-label="Next mark" title="Next kill, death or cut. Key: >">⏭</button>
        <div class="scrub">
          <input
            type="range"
            min={bounds().from_tick}
            max={bounds().to_tick}
            step={1}
            value={s.tick()}
            onInput={(e) => s.seek(+e.currentTarget.value)}
            aria-label={s.isLive() ? "Live playback position" : "Replay position"}
          />
          <Show when={inMark() !== null && outMark() !== null && inMark()! < outMark()!}>
            <span class="scrub__selection" style={{ left: at(inMark()!), width: `${percent(outMark()!) - percent(inMark()!)}%` }} aria-hidden="true" />
          </Show>
          <Show when={inMark() !== null}><span class="scrub__boundary scrub__boundary--in" style={{ left: at(inMark()!) }} aria-hidden="true" /></Show>
          <Show when={outMark() !== null}><span class="scrub__boundary scrub__boundary--out" style={{ left: at(outMark()!) }} aria-hidden="true" /></Show>
          <div class="scrub__kills">
            <For each={marks()}>
              {(m) => (
                <Show
                  when={m.kill}
                  fallback={<span class={`scrub__mark scrub__mark--${m.kind}`} style={markStyle(m)} title={MARK_TITLES[m.kind]} aria-hidden="true" />}
                >
                  {(k) => (
                    <button
                      class={`scrub__kill scrub__kill--${m.kind}`}
                      tabindex="-1"
                      style={{ left: at(m.tick), background: m.kind === "kill" ? props.killColour?.(k().killer) : undefined }}
                      title={`${MARK_TITLES[m.kind]} (${time(m.tick)}). Key: k`}
                      aria-label={`Watch the ${m.kind} at ${time(m.tick)}`}
                      onClick={() => s.watchKill(k())}
                    />
                  )}
                </Show>
              )}
            </For>
          </div>
        </div>
        <span class="replay-time">{time(s.tick())}<Show when={!s.isLive()}> / {clock(length() / TICKS_PER_SECOND)}</Show></span>
        <select value={s.speed()} onChange={(e) => s.setSpeed(+e.currentTarget.value)} disabled={s.followingLive()} aria-label="Speed">
          <For each={SPEEDS}>{(v) => <option value={v}>{v}×</option>}</For>
        </select>
        <Show when={s.isLive()}>
          <button type="button" class="replay-live" aria-pressed={s.followingLive()} title="Return to live play" onClick={() => s.goLive()}>Live</button>
        </Show>
      </div>
    </Show>
  );
}
