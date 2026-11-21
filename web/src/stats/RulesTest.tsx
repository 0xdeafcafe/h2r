// The workbench's test half (docs/game/rules-engine.md, "Workbench"): the
// scenario editor (run length, seated players, events added in a click
// and read as sentences, times typed as times) and the results of a run
// (final standings, then a timeline of what happened, what the script did
// about it and who scored). The pure half is ../lib/rules.ts.

import { For, Index, Show, createMemo, createSignal } from "solid-js";
import { teamName } from "../lib/game";
import {
  EVENT_TYPES, MAX_TICKS, TICKS_PER_SECOND, defaultScenario, describeEvent, faultLabel, moveEvent, newEvent, nextEventTick,
  nextSlot, parseTickTime, playerName, problemPlace, signed, standings, teamStandings, tickTime, timeline, traceLines,
  withEvent,
  type EventType, type Problem, type ScenarioDraft, type ScenarioEvent, type ScenarioPlayer, type Trace, type Trouble,
} from "../lib/rules";

const int = (v: string, dflt = 0): number => {
  const n = Number.parseInt(v, 10);
  return Number.isFinite(n) ? n : dflt;
};
const teamLabel = (team: number): string => `${teamName(team)} team`;
const TIME_HELP = "A time: minutes:seconds (0:02.50), seconds (2.5) or ticks (75t). A second is 30 ticks.";

/** A request that failed, with whose fault it is and a way to ask again. */
export function TroubleNote(props: { trouble: Trouble; onRetry?: () => void }) {
  return (
    <p class="s-rules-trouble" classList={{ "s-rules-trouble--script": props.trouble.fault === "script" }} role="status">
      <b>{faultLabel(props.trouble.fault)}.</b> <span>{props.trouble.text}</span>
      <Show when={props.trouble.retry && props.onRetry}><button class="s-btn" onClick={() => props.onRetry?.()}>Try again</button></Show>
    </p>
  );
}

/** A tick typed and shown as a time. What can't be read is put back. */
function TimeInput(props: { tick: number; label: string; max?: number; onTick: (tick: number) => void }) {
  return (
    <input
      class="s-rules-timein"
      type="text"
      inputmode="decimal"
      spellcheck={false}
      value={tickTime(props.tick)}
      aria-label={props.label}
      title={TIME_HELP}
      onChange={(e) => {
        const read = parseTickTime(e.currentTarget.value);
        const next = read === null ? props.tick : Math.min(props.max ?? MAX_TICKS, read);
        e.currentTarget.value = tickTime(next);
        if (next !== props.tick) props.onTick(next);
      }}
    />
  );
}

/** A select of the seated players, with "The world" for a killer. A
 *  player named but no longer seated stays listed, marked. */
function PlayerPick(props: { value: number | null | undefined; slots: number[]; world?: boolean; label: string; onPick: (v: number | null) => void }) {
  const unseated = () => (typeof props.value === "number" && !props.slots.includes(props.value) ? props.value : null);
  return (
    <select aria-label={props.label} value={typeof props.value === "number" ? String(props.value) : "world"} onChange={(e) => props.onPick(e.currentTarget.value === "world" ? null : int(e.currentTarget.value))}>
      <Show when={props.world}><option value="world">The world</option></Show>
      <For each={props.slots}>{(s) => <option value={String(s)}>{playerName(s)}</option>}</For>
      <Show when={unseated() !== null}><option value={String(unseated())}>{playerName(unseated() ?? 0)} (not seated)</option></Show>
    </select>
  );
}

function CountInput(props: { value: number; label: string; onCount: (n: number) => void }) {
  return <input class="s-rules-num" type="number" min={0} step={1} value={props.value} aria-label={props.label} onChange={(e) => props.onCount(Math.max(0, int(e.currentTarget.value)))} />;
}

function EventRow(props: {
  e: ScenarioEvent;
  slots: number[];
  players: ScenarioPlayer[];
  onChange: (patch: Partial<ScenarioEvent>) => void;
  onReplace: (e: ScenarioEvent) => void;
  onTick: (tick: number) => void;
  onRemove: () => void;
}) {
  const player = () => <PlayerPick label="Player" value={props.e.player} slots={props.slots} onPick={(v) => props.onChange({ player: v ?? 0 })} />;
  const object = () => <><span>object</span><CountInput label="Object" value={props.e.object ?? 0} onCount={(n) => props.onChange({ object: n })} /></>;
  const details = () => {
    switch (props.e.type) {
      case "kill":
        return (
          <>
            <PlayerPick label="Killer" world value={props.e.killer} slots={props.slots} onPick={(v) => props.onChange({ killer: v })} />
            <span>killed</span>
            <PlayerPick label="Victim" value={props.e.victim} slots={props.slots} onPick={(v) => props.onChange({ victim: v ?? 0 })} />
          </>
        );
      case "press": case "pickup": case "drop":
        return <>{player()}{object()}</>;
      case "enter": case "leave":
        return <>{object()}<span>zone</span><CountInput label="Zone" value={props.e.zone ?? 0} onCount={(n) => props.onChange({ zone: n })} /></>;
      case "round_start": case "round_end":
        return <span class="s-rules-muted">nothing to set</span>;
      default:
        return player();
    }
  };
  return (
    <li class="s-rules-ev">
      <TimeInput label="When" tick={props.e.tick} onTick={(t) => props.onTick(t)} />
      <select aria-label="Event" value={props.e.type} onChange={(e) => props.onReplace(newEvent(e.currentTarget.value as EventType, props.e.tick, props.players))}>
        <For each={EVENT_TYPES}>{(t) => <option value={t.type}>{t.label}</option>}</For>
      </select>
      <span class="s-rules-ev-what">{details()}</span>
      <button class="s-btn" onClick={() => props.onRemove()} aria-label={`Remove: ${describeEvent(props.e)}`}>Remove</button>
    </li>
  );
}

export function ScenarioEditor(props: { scenario: ScenarioDraft; set: (f: (d: ScenarioDraft) => ScenarioDraft) => void; problems: string[] }) {
  const slots = () => props.scenario.players.map((p) => p.slot);
  const setPlayer = (i: number, patch: Partial<ScenarioPlayer>) =>
    props.set((d) => ({ ...d, players: d.players.map((p, j) => (j === i ? { ...p, ...patch } : p)) }));
  const setEvent = (i: number, f: (e: ScenarioEvent) => ScenarioEvent) =>
    props.set((d) => ({ ...d, events: d.events.map((e, j) => (j === i ? f(e) : e)) }));

  // The kill being added: the first seated player kills the next, until
  // other picks are made. `null` is the world.
  const [pickedKiller, setPickedKiller] = createSignal<number | null | undefined>(undefined);
  const [pickedVictim, setPickedVictim] = createSignal<number | undefined>(undefined);
  const killer = (): number | null => {
    const k = pickedKiller();
    if (k === null) return null;
    return k !== undefined && slots().includes(k) ? k : slots()[0] ?? null;
  };
  const victim = (): number | undefined => {
    const v = pickedVictim();
    return v !== undefined && slots().includes(v) ? v : slots().find((s) => s !== killer()) ?? slots()[0];
  };
  const addKill = () => {
    const v = victim();
    if (v === undefined) return;
    const k = killer();
    props.set((d) => withEvent(d, { tick: nextEventTick(d), type: "kill", victim: v, killer: k }));
  };
  const addOther = (type: EventType) => props.set((d) => withEvent(d, newEvent(type, nextEventTick(d), d.players)));

  return (
    <div class="s-rules-scenario">
      <div class="s-rules-set">
        <label>
          <span>Run for</span>
          <TimeInput label="Run for" tick={props.scenario.ticks} onTick={(t) => props.set((d) => ({ ...d, ticks: t }))} />
          <small>{props.scenario.ticks.toLocaleString("en-US")} ticks</small>
        </label>
        <label>
          <span>Round clock</span>
          <input class="s-rules-num s-rules-num--wide" type="number" min={0} step={1} value={Math.round(props.scenario.roundTime / TICKS_PER_SECOND)} onChange={(e) => props.set((d) => ({ ...d, roundTime: Math.max(0, int(e.currentTarget.value)) * TICKS_PER_SECOND }))} />
          <small>seconds left at the start</small>
        </label>
        <label title="The same seed gives the same run.">
          <span>Seed</span>
          <input class="s-rules-num s-rules-num--wide" type="number" min={0} step={1} value={props.scenario.seed} onChange={(e) => props.set((d) => ({ ...d, seed: Math.max(0, int(e.currentTarget.value)) }))} />
        </label>
        <label class="s-rules-check">
          <input type="checkbox" checked={props.scenario.teamPlay} onChange={(e) => props.set((d) => ({ ...d, teamPlay: e.currentTarget.checked }))} />
          <span>Team play</span>
        </label>
        <button class="s-btn" onClick={() => props.set(() => defaultScenario())} title="Back to two players and three events">Reset scenario</button>
      </div>

      <Show when={props.problems.length}>
        <ul class="s-rules-issues" aria-label="Scenario problems">
          <For each={props.problems}>{(p) => <li>{p}</li>}</For>
        </ul>
      </Show>

      <section class="s-rules-part">
        <h4>Players <small>{props.scenario.players.length} seated</small></h4>
        <Show when={props.scenario.players.length} fallback={<p class="s-rules-muted s-rules-none">Nobody is seated. Add a player to make events with.</p>}>
          <table class="s-grid s-rules-table">
            <thead><tr><th>Player</th><th>Team</th><th>Lives</th><th /></tr></thead>
            <tbody>
              <Index each={props.scenario.players}>
                {(p, i) => (
                  <tr>
                    <td>
                      <select value={String(p().slot)} aria-label="Player slot" onChange={(e) => setPlayer(i, { slot: int(e.currentTarget.value) })}>
                        <For each={Array.from({ length: 16 }, (_, s) => s)}>{(s) => <option value={String(s)} disabled={s !== p().slot && slots().includes(s)}>{playerName(s)}</option>}</For>
                      </select>
                    </td>
                    <td>
                      <select value={String(p().team)} aria-label="Team" onChange={(e) => setPlayer(i, { team: int(e.currentTarget.value) })}>
                        <For each={Array.from({ length: 8 }, (_, t) => t)}>{(t) => <option value={String(t)}>{teamName(t)} (team {t})</option>}</For>
                      </select>
                    </td>
                    <td><CountInput label="Lives" value={p().lives} onCount={(n) => setPlayer(i, { lives: n })} /></td>
                    <td class="s-rules-end"><button class="s-btn" aria-label={`Remove ${playerName(p().slot)}`} onClick={() => props.set((d) => ({ ...d, players: d.players.filter((_, j) => j !== i) }))}>Remove</button></td>
                  </tr>
                )}
              </Index>
            </tbody>
          </table>
        </Show>
        <div class="s-rules-add">
          <button class="s-btn" disabled={nextSlot(props.scenario.players) === null}
            onClick={() => props.set((d) => { const slot = nextSlot(d.players); return slot === null ? d : { ...d, players: [...d.players, { slot, team: d.players.length % 2, lives: 0 }] }; })}>
            Add player
          </button>
          <Show when={nextSlot(props.scenario.players) === null}><span class="s-rules-muted">All 16 slots are seated.</span></Show>
        </div>
      </section>

      <section class="s-rules-part">
        <h4>Events <small>{props.scenario.events.length} in time order</small></h4>
        <div class="s-rules-quick" role="group" aria-label="Add an event">
          <PlayerPick label="Killer" world value={killer()} slots={slots()} onPick={(v) => setPickedKiller(v)} />
          <span>kills</span>
          <PlayerPick label="Victim" value={victim()} slots={slots()} onPick={(v) => setPickedVictim(v ?? undefined)} />
          <button class="s-btn" onClick={addKill} disabled={victim() === undefined}>Add kill</button>
          <span class="s-rules-muted">or</span>
          <select aria-label="Add another event" value="" onChange={(e) => { const type = e.currentTarget.value; e.currentTarget.value = ""; if (type) addOther(type as EventType); }}>
            <option value="">Add another event...</option>
            <For each={EVENT_TYPES.filter((t) => t.type !== "kill")}>{(t) => <option value={t.type}>{t.label}</option>}</For>
          </select>
        </div>
        <Show when={victim() === undefined}><p class="s-rules-muted s-rules-none">Add a player before adding a kill.</p></Show>
        <Show when={props.scenario.events.length} fallback={<p class="s-rules-muted s-rules-none">No events. The script's <code>on init</code>, <code>on tick</code> and timers still run.</p>}>
          <div class="s-rules-ev s-rules-ev--head" aria-hidden="true"><span>Time</span><span>Event</span><span>Who and what</span><span /></div>
          <ol class="s-rules-events">
            <Index each={props.scenario.events}>
              {(e, i) => (
                <EventRow
                  e={e()}
                  slots={slots()}
                  players={props.scenario.players}
                  onChange={(patch) => setEvent(i, (old) => ({ ...old, ...patch }))}
                  onReplace={(next) => setEvent(i, () => next)}
                  onTick={(t) => props.set((d) => moveEvent(d, i, t))}
                  onRemove={() => props.set((d) => ({ ...d, events: d.events.filter((_, j) => j !== i) }))}
                />
              )}
            </Index>
          </ol>
          <p class="s-rules-muted s-rules-none">A new event lands a second after the last one. Type a time to move it.</p>
        </Show>
      </section>
    </div>
  );
}

/** A run that was made, with the scenario it ran. */
export interface Ran { trace: Trace; scenario: ScenarioDraft; key: string }

export function ResultsView(props: {
  ran: Ran | null;
  running: boolean;
  /** Why the scenario can't run now; null when it can. */
  blocked: string | null;
  /** The results are of an older script or scenario. */
  stale: boolean;
  trouble: Trouble | null;
  /** The run's own build errors, when the check passed but the run didn't build. */
  errors: Problem[];
  onRun: () => void;
  onJump: (p: Problem) => void;
}) {
  const [copied, setCopied] = createSignal(false);
  const copy = async () => {
    const r = props.ran;
    if (!r) return;
    try { await navigator.clipboard.writeText(traceLines(r.trace).join("\n")); setCopied(true); setTimeout(() => setCopied(false), 1500); } catch { /* no clipboard */ }
  };
  const rows = createMemo(() => (props.ran ? timeline(props.ran.trace, props.ran.scenario, teamLabel) : []));
  const table = createMemo(() => (props.ran ? standings(props.ran.trace, props.ran.scenario) : []));
  const teams = () => (props.ran?.scenario.teamPlay ? teamStandings(table()) : []);
  return (
    <div class="s-rules-results">
      <div class="s-rules-runbar">
        <button class="s-btn" onClick={() => props.onRun()} disabled={!!props.blocked || props.running} title={props.blocked ?? "Run the scenario again"}>{props.running ? "Running..." : "Run again"}</button>
        <span class="s-rules-muted">Runs by itself when the script or the scenario changes.</span>
        <Show when={props.ran}><button class="s-btn s-rules-copy" onClick={() => void copy()}>{copied() ? "Copied" : "Copy as text"}</button></Show>
      </div>
      <Show when={props.trouble}>{(t) => <TroubleNote trouble={t()} onRetry={props.onRun} />}</Show>
      <Show when={props.errors.length}>
        <ul class="s-rules-errors" aria-label="Run errors">
          <For each={props.errors}>{(p) => <li><button class="s-rules-errlink" onClick={() => props.onJump(p)} disabled={p.line === 0}>{problemPlace(p)}</button><span>{p.message}</span></li>}</For>
        </ul>
      </Show>
      <Show when={props.blocked}>
        {(why) => <p class="s-rules-hold">{props.ran ? "These results are from the last run." : "No results yet."} {why()}</p>}
      </Show>
      <Show when={props.ran} fallback={<Show when={!props.blocked && !props.trouble}><p class="s-rules-muted s-rules-none">{props.running ? "Running the scenario..." : "The results show here once the scenario has run."}</p></Show>}>
        {(r) => (
          <div class="s-rules-run" classList={{ "s-rules-stale": props.stale }}>
            <p class="s-rules-sum">
              <Show when={r().trace.round_ended !== null} fallback={<>Ran for <b>{tickTime(r().trace.last_tick)}</b> ({r().trace.last_tick.toLocaleString("en-US")} ticks) without the round ending.</>}>
                The script ended the round at <b>{tickTime(r().trace.round_ended ?? 0)}</b> (tick {r().trace.round_ended}).
              </Show>
              {" "}<span class="s-rules-muted">{r().trace.ops.toLocaleString("en-US")} ops used.</span>
            </p>
            <Show when={r().trace.error}>{(e) => <p class="s-rules-trouble s-rules-trouble--script"><b>Script.</b> <span>It stopped with an error: {e()}</span></p>}</Show>
            <Show when={r().trace.cut}>{(c) => <p class="s-rules-hold">{c()}</p>}</Show>

            <h4>Final standings</h4>
            <Show when={table().length} fallback={<p class="s-rules-muted s-rules-none">Nobody has a score.</p>}>
              <table class="s-grid s-rules-table s-rules-standings">
                <thead><tr><th class="num">Place</th><th>Player</th><Show when={r().scenario.teamPlay}><th>Team</th></Show><th class="num">Score</th></tr></thead>
                <tbody>
                  <For each={table()}>
                    {(s) => <tr><td class="num">{s.place}</td><td>{playerName(s.slot)}</td><Show when={r().scenario.teamPlay}><td>{teamName(s.team)}</td></Show><td class="num"><b>{s.score}</b></td></tr>}
                  </For>
                </tbody>
              </table>
            </Show>
            <Show when={teams().length}>
              <p class="s-rules-teams">Teams: <For each={teams()}>{(t, i) => <>{i() > 0 ? ", " : ""}<b>{teamName(t.team)} {t.score}</b></>}</For></p>
            </Show>

            <h4>What happened</h4>
            <Show when={rows().length} fallback={<p class="s-rules-muted s-rules-none">Nothing happened: the scenario has no events in the run, and the script made no call and changed nothing.</p>}>
              <ol class="s-rules-timeline">
                <For each={rows()}>
                  {(k) => (
                    <li classList={{ "s-rules-tl--bad": !!k.error }}>
                      <span class="s-rules-tl-when" title={`tick ${k.tick}`}>{tickTime(k.tick)}<small>tick {k.tick}</small></span>
                      <div class="s-rules-tl-what">
                        <For each={k.happened}>{(h) => <p class="s-rules-tl-event">{h}</p>}</For>
                        <Show when={k.ran.length || k.calls.length} fallback={<Show when={k.happened.length && !k.changes.length && !k.scored.length}><p class="s-rules-muted">No handler ran for this.</p></Show>}>
                          <p class="s-rules-tl-did">
                            <For each={k.ran}>{(r2) => <code class="s-rules-chip">on {r2}</code>}</For>
                            <For each={k.calls}>{(c) => <code>{c}</code>}</For>
                          </p>
                        </Show>
                        <For each={k.scored}>
                          {(s) => <p class="s-rules-tl-score" classList={{ down: s.delta < 0 }}><b>{playerName(s.slot)} {signed(s.delta)}</b> <span>now {s.score}</span></p>}
                        </For>
                        <For each={k.changes}>{(c) => <p class="s-rules-tl-change">{c}</p>}</For>
                        <For each={k.warnings}>{(w) => <p class="s-rules-warn">Warning: {w}</p>}</For>
                        <Show when={k.error}>{(e) => <p class="s-rules-bad">The script stopped: {e()}</p>}</Show>
                      </div>
                    </li>
                  )}
                </For>
              </ol>
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
}
