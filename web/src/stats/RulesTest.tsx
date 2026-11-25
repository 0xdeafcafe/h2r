// The workbench's test half (docs/game/rules-engine.md, "Workbench"): the
// strip that sets up generated matches (players, team play, the seed and
// its die), the summary of a batch of them (who wins, how games end, the
// worst tick, warnings with the seed that shows each), one match's
// results (final standings, then a timeline of what happened, what the
// script did about it and who scored), and the hand editor for a match
// (seated players, the test map's things by label with what it lacks and
// "Add what the script needs", events read as sentences that name things
// by label, times typed as times), folded away until asked for, and
// "What a map needs". The pure half is ../lib/rules.ts.

import { For, Index, Show, createMemo, createSignal } from "solid-js";
import { teamName } from "../lib/game";
import {
  EVENT_TYPES, MAP_CLASSES, MAX_SEED, MAX_TICKS, SIM_PLAYERS, TICKS_PER_SECOND, classLabel, defaultScenario, describeEvent, endLabel,
  faultLabel, hudStrip, kindWords, mapGroups, mapInPlay, mapShort, meter, moveEvent, neighbourSeed, newEvent, newThing, nextEventTick, nextSlot,
  objectName, ofMatches, parseTickTime, playerName, problemPlace, rerolled, shortTime, signed, simWarnings, standings, suggestedClass,
  teamStandings, thingChoices, thingName, thingsOfMinimal, tickTime, timeline, traceLines, unitOf, winnerName, withEvent, withNeeds,
  type EventType, type HudLine, type LabelLine, type MapThing, type Problem, type ScenarioDraft, type ScenarioEvent, type ScenarioObject,
  type ScenarioPlayer, type SimResult, type SimRow, type SimSetup, type Trace, type Trouble,
} from "../lib/rules";

const int = (v: string, dflt = 0): number => {
  const n = Number.parseInt(v, 10);
  return Number.isFinite(n) ? n : dflt;
};
const teamLabel = (team: number): string => `${teamName(team)} team`;
const TIME_HELP = "A time: minutes:seconds (0:02.50), seconds (2.5) or ticks (75t). A second is 30 ticks.";
/** Timeline rows shown before "Show all". */
const TIMELINE_ROWS = 150;
/** Winners listed before the rest are summed. */
const WINNERS_SHOWN = 6;

/** A request that failed, with whose fault it is and a way to ask again. */
export function TroubleNote(props: { trouble: Trouble; onRetry?: () => void }) {
  return (
    <p class="s-rules-trouble" classList={{ "s-rules-trouble--script": props.trouble.fault === "script" }} role="status">
      <b>{faultLabel(props.trouble.fault)}.</b> <span>{props.trouble.text}</span>
      <Show when={props.trouble.retry && props.onRetry}><button class="s-btn" onClick={() => props.onRetry?.()}>Try again</button></Show>
    </p>
  );
}

/** The script's HUD in the run, as the match's first seated player sees
 *  it: the last tick that showed a row, with each row's place, words and
 *  mark. A run whose script never feeds its HUD shows none. */
export function HudStrip(props: { t: Trace; scenario: ScenarioDraft }) {
  const strip = () => hudStrip(props.t);
  const things = () => props.scenario.objects ?? [];
  const row = (h: HudLine) => {
    const words = h.kind === "waypoint"
      ? `marks ${h.at === null ? "nothing" : objectName(h.at, things())}`
      : h.text ?? String(h.value);
    return (
      <li classList={{ "s-rules-hud--hidden": !h.shown }}>
        <b>{h.name}</b> <span class="s-rules-muted">{h.slot ?? h.kind}</span> <span>{words}</span>
      </li>
    );
  };
  return (
    <Show when={strip()}>
      {(st) => (
        <>
          <h4>The script's HUD</h4>
          <div class="s-rules-hud">
            <p class="s-rules-muted">
              At {tickTime(st().tick)} ({st().tick.toLocaleString("en-US")} ticks), as {playerName(props.scenario.players[0]?.slot ?? 0)} sees it.
            </p>
            <ul><For each={st().rows}>{(h) => row(h)}</For></ul>
          </div>
        </>
      )}
    </Show>
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

/** A select of the things an event's field may name, by label and number
 *  ("capture 0"), with the seated players' own objects for an object. A
 *  handle the map no longer has stays listed, marked. With nothing to
 *  list, the handle is typed. */
function ThingPick(props: { value: number | undefined; want: "zone" | "object"; units?: boolean; things: ScenarioObject[]; labels: LabelLine[]; players: ScenarioPlayer[]; label: string; onPick: (id: number) => void }) {
  const choices = createMemo(() => [
    ...(props.units ? props.players.map((p) => ({ id: unitOf(p), name: playerName(p.slot) })) : []),
    ...thingChoices(props.things, props.want, props.labels),
  ]);
  const value = () => props.value ?? 0;
  const stray = () => (choices().some((c) => c.id === value()) ? null : value());
  return (
    <Show when={choices().length} fallback={<CountInput label={props.label} value={value()} onCount={props.onPick} />}>
      <select aria-label={props.label} value={String(value())} onChange={(e) => props.onPick(int(e.currentTarget.value))}>
        <For each={choices()}>{(c) => <option value={String(c.id)}>{c.name}</option>}</For>
        <Show when={stray() !== null}><option value={String(stray())}>{objectName(stray() ?? 0, props.things)} (not on the map)</option></Show>
      </select>
    </Show>
  );
}

function EventRow(props: {
  e: ScenarioEvent;
  slots: number[];
  players: ScenarioPlayer[];
  things: ScenarioObject[];
  labels: LabelLine[];
  onChange: (patch: Partial<ScenarioEvent>) => void;
  onReplace: (e: ScenarioEvent) => void;
  onTick: (tick: number) => void;
  onRemove: () => void;
}) {
  const player = () => <PlayerPick label="Player" value={props.e.player} slots={props.slots} onPick={(v) => props.onChange({ player: v ?? 0 })} />;
  const object = () => <><span>object</span><ThingPick label="Object" want="object" value={props.e.object} things={props.things} labels={props.labels} players={props.players} onPick={(n) => props.onChange({ object: n })} /></>;
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
      case "press": case "pickup": case "drop": case "bounce": case "edit":
        return <>{player()}{object()}</>;
      case "squad_dead":
        return <><span>squad</span><CountInput label="Squad" value={props.e.squad ?? 0} onCount={(n) => props.onChange({ squad: n })} /></>;
      case "actor_kill": case "actor_assist":
        return (
          <>
            <PlayerPick label="Killer" world value={props.e.killer} slots={props.slots} onPick={(v) => props.onChange({ killer: v })} />
            <Show when={props.e.type === "actor_assist"}>
              <span>helped by</span>
              <PlayerPick label="Assist" world value={props.e.assist} slots={props.slots} onPick={(v) => props.onChange({ assist: v })} />
            </Show>
            <span>killed a</span>
            <input class="s-rules-class" type="text" spellcheck={false} value={props.e.class ?? ""} aria-label="Class" onChange={(e) => props.onChange({ class: e.currentTarget.value.trim() })} />
          </>
        );
      case "enter": case "leave":
        return (
          <>
            <ThingPick label="Object" want="object" units value={props.e.object} things={props.things} labels={props.labels} players={props.players} onPick={(n) => props.onChange({ object: n })} />
            <span>{props.e.type === "enter" ? "entered" : "left"}</span>
            <ThingPick label="Zone" want="zone" value={props.e.zone} things={props.things} labels={props.labels} players={props.players} onPick={(n) => props.onChange({ zone: n })} />
          </>
        );
      case "round_start": case "round_end":
        return <span class="s-rules-muted">nothing to set</span>;
      default:
        return player();
    }
  };
  return (
    <li class="s-rules-ev">
      <TimeInput label="When" tick={props.e.tick} onTick={(t) => props.onTick(t)} />
      <select aria-label="Event" value={props.e.type} onChange={(e) => props.onReplace(newEvent(e.currentTarget.value as EventType, props.e.tick, props.players, props.things, props.labels))}>
        <For each={EVENT_TYPES}>{(t) => <option value={t.type}>{t.label}</option>}</For>
      </select>
      <span class="s-rules-ev-what">{details()}</span>
      <button class="s-btn" onClick={() => props.onRemove()} aria-label={`Remove: ${describeEvent(props.e, props.things)}`}>Remove</button>
    </li>
  );
}

/** "What a map needs": each label the script declares, its kind and its
 *  requirement in Forge's words. */
export function MapNeeds(props: { labels: LabelLine[] }) {
  return (
    <ul class="s-rules-needs" aria-label="What a map needs">
      <For each={props.labels}>
        {(l) => (
          <li>
            <b><code>{l.text}</code></b>
            <small>{kindWords(l)}{l.per_team ? ", per team" : ""}</small>
            <span>{l.needs}</span>
          </li>
        )}
      </For>
    </ul>
  );
}

/** The test map: its things under their labels, what it lacks for the
 *  script's labels with "Add what the script needs", and a thing added by
 *  label. With no map of its own the match plays the script's smallest. */
function MapEditor(props: {
  scenario: ScenarioDraft;
  set: (f: (d: ScenarioDraft) => ScenarioDraft) => void;
  labels: LabelLine[];
  minimal: MapThing[];
  /** What the map lacks, as the run (else the page) found it. */
  problems: string[];
}) {
  const own = () => props.scenario.objects;
  const groups = createMemo(() => mapGroups(own() ?? [], props.labels));
  const setThings = (f: (things: ScenarioObject[]) => ScenarioObject[]) => props.set((d) => ({ ...d, objects: f(d.objects ?? thingsOfMinimal(props.minimal)) }));
  const setThing = (id: number, patch: Partial<ScenarioObject>) => setThings((ts) => ts.map((t) => (t.id === id ? { ...t, ...patch } : t)));
  const [adding, setAdding] = createSignal<string | null>(null);
  /** The labels a thing may be added under: the script's, then the map's
   *  own; any other text is a label too. */
  const names = createMemo(() => [...new Set([...props.labels.map((l) => l.text), ...(own() ?? []).map((o) => o.label)])]);
  const addLabel = () => (adding() ?? names()[0] ?? "").trim();
  const add = () => {
    const label = addLabel();
    if (!label) return;
    setThings((ts) => [...ts, newThing(ts, label, props.labels.find((l) => l.text === label) ?? null)]);
  };
  const count = () => own()?.length ?? 0;
  return (
    <section class="s-rules-part">
      <h4>Map <small>{own() ? `${count()} ${count() === 1 ? "thing" : "things"} labelled` : "the script's smallest map"}</small></h4>
      <Show when={own()} fallback={
        <p class="s-rules-muted s-rules-none">
          The match plays on the smallest map the script's labels accept{props.minimal.length ? ` (${props.minimal.length} ${props.minimal.length === 1 ? "thing" : "things"})` : ""}.{" "}
          <button class="s-btn" onClick={() => props.set((d) => ({ ...d, objects: thingsOfMinimal(props.minimal) }))}>Make it my map</button>
        </p>
      }>
        <Show when={props.problems.length}>
          <div class="s-rules-mapfix">
            <ul class="s-rules-issues" aria-label="Map problems">
              <For each={props.problems}>{(p) => <li>{capitalised(p)}.</li>}</For>
            </ul>
            <button class="s-btn" disabled={!props.minimal.length && !props.labels.some((l) => l.per_team)} title="Merges the script's smallest map into this one" onClick={() => setThings((ts) => withNeeds(ts, props.minimal, props.labels))}>Add what the script needs</button>
          </div>
        </Show>
        <Show when={groups().length} fallback={<p class="s-rules-muted s-rules-none">Nothing on this map is labelled.</p>}>
          <table class="s-grid s-rules-table s-rules-map">
            <thead><tr><th>Thing</th><th>Number</th><th>Team</th><th>Is a</th><th /></tr></thead>
            <For each={groups()}>
              {(g) => (
                <tbody>
                  <tr class="s-rules-map-label">
                    <th colspan="5"><code>{g.label}</code> <small>{g.line ? g.line.needs : "the script doesn't declare this label"}</small></th>
                  </tr>
                  <For each={g.things}>
                    {(t) => (
                      <tr>
                        <td>{thingName(t, own() ?? [])}</td>
                        <td>
                          <input class="s-rules-num" type="number" step={1} value={t.number ?? ""} placeholder="none" aria-label="Number"
                            onChange={(e) => { const v = e.currentTarget.value.trim(); setThing(t.id, { number: v === "" ? null : int(v) }); }} />
                        </td>
                        <td>
                          <select value={t.team === null ? "" : String(t.team)} aria-label="Team" onChange={(e) => setThing(t.id, { team: e.currentTarget.value === "" ? null : int(e.currentTarget.value) })}>
                            <option value="">none</option>
                            <For each={Array.from({ length: 8 }, (_, n) => n)}>{(n) => <option value={String(n)}>{teamName(n)} (team {n})</option>}</For>
                          </select>
                        </td>
                        <td>
                          <select value={t.class ?? ""} aria-label="Is a" onChange={(e) => { const c = MAP_CLASSES.find((x) => x.class === e.currentTarget.value)?.class; setThing(t.id, { class: c }); }}>
                            <option value="">{classLabel(suggestedClass(g.line?.kind ?? null))} (from the label)</option>
                            <For each={MAP_CLASSES}>{(c) => <option value={c.class}>{c.label}</option>}</For>
                          </select>
                        </td>
                        <td class="s-rules-end"><button class="s-btn" aria-label={`Remove ${thingName(t, own() ?? [])}`} onClick={() => setThings((ts) => ts.filter((x) => x.id !== t.id))}>Remove</button></td>
                      </tr>
                    )}
                  </For>
                </tbody>
              )}
            </For>
          </table>
        </Show>
        <div class="s-rules-add">
          <input class="s-rules-class" type="text" spellcheck={false} list="s-rules-map-labels" placeholder="label" aria-label="Label of the thing to add" value={addLabel()} onInput={(e) => setAdding(e.currentTarget.value)} />
          <datalist id="s-rules-map-labels"><For each={names()}>{(n) => <option value={n} />}</For></datalist>
          <button class="s-btn" disabled={!addLabel()} onClick={add}>Add thing</button>
          <button class="s-btn" title="Play on the script's smallest map again" onClick={() => props.set((d) => ({ ...d, objects: undefined }))}>Use the smallest map</button>
        </div>
      </Show>
    </section>
  );
}

const capitalised = (s: string): string => (s ? s[0]!.toUpperCase() + s.slice(1) : s);

export function ScenarioEditor(props: {
  scenario: ScenarioDraft;
  set: (f: (d: ScenarioDraft) => ScenarioDraft) => void;
  problems: string[];
  /** The script's labels and smallest map, from its check. */
  labels: LabelLine[];
  minimal: MapThing[];
  /** What the map lacks, from the last run of this match; empty when the
   *  run found nothing or has not answered. */
  mapProblems: string[];
}) {
  const slots = () => props.scenario.players.map((p) => p.slot);
  const things = createMemo(() => mapInPlay(props.scenario, props.minimal));
  const mapIssues = createMemo(() => (props.mapProblems.length ? props.mapProblems : props.scenario.objects ? mapShort(props.labels, props.scenario.objects) : []));
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
  const addOther = (type: EventType) => props.set((d) => withEvent(d, newEvent(type, nextEventTick(d), d.players, mapInPlay(d, props.minimal), props.labels)));

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
        <button class="s-btn" onClick={() => props.set(() => defaultScenario())} title="Back to two players and three events">Reset</button>
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

      <MapEditor scenario={props.scenario} set={props.set} labels={props.labels} minimal={props.minimal} problems={mapIssues()} />

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
                  things={things()}
                  labels={props.labels}
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

// ---------- generated matches ----------

function Die() {
  return (
    <svg class="s-rules-die-face" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true">
      <rect x="0.5" y="0.5" width="11" height="11" rx="2" fill="none" stroke="currentColor" />
      <circle cx="3.5" cy="3.5" r="1" /><circle cx="8.5" cy="3.5" r="1" /><circle cx="6" cy="6" r="1" /><circle cx="3.5" cy="8.5" r="1" /><circle cx="8.5" cy="8.5" r="1" />
    </svg>
  );
}

/** What the generated matches are played with: players, team play, the
 *  seed and its die. Nothing else to fill in. */
export function SimStrip(props: { setup: SimSetup; set: (f: (s: SimSetup) => SimSetup) => void }) {
  const players = (n: number) => props.set((s) => ({ ...s, players: Math.min(SIM_PLAYERS.max, Math.max(SIM_PLAYERS.min, n)) }));
  return (
    <div class="s-rules-set s-rules-strip" role="group" aria-label="Generated matches">
      <span class="s-rules-stepper">
        <span>Players</span>
        <button class="s-btn" aria-label="One player fewer" disabled={props.setup.players <= SIM_PLAYERS.min} onClick={() => players(props.setup.players - 1)}>-</button>
        <input type="range" min={SIM_PLAYERS.min} max={SIM_PLAYERS.max} step={1} value={props.setup.players} aria-label="Players" onInput={(e) => players(int(e.currentTarget.value, props.setup.players))} />
        <button class="s-btn" aria-label="One player more" disabled={props.setup.players >= SIM_PLAYERS.max} onClick={() => players(props.setup.players + 1)}>+</button>
        <b>{props.setup.players}</b>
      </span>
      <label class="s-rules-check">
        <input type="checkbox" checked={props.setup.teamPlay} onChange={(e) => props.set((s) => ({ ...s, teamPlay: e.currentTarget.checked }))} />
        <span>Team play</span>
      </label>
      <label title="The same seed gives the same matches.">
        <span>Seed</span>
        <input class="s-rules-num s-rules-num--wide" type="number" min={0} max={MAX_SEED} step={1} value={props.setup.seed} onChange={(e) => props.set((s) => ({ ...s, seed: Math.min(MAX_SEED, Math.max(0, int(e.currentTarget.value, s.seed))) }))} />
      </label>
      <button class="s-btn s-rules-die" title="A new seed: other matches" onClick={() => props.set((s) => ({ ...s, seed: rerolled(s.seed) }))}><Die /> Re-roll</button>
    </div>
  );
}

/** A seed as a link to its match. */
function SeedLink(props: { seed: number; shown: number | null; onSeed: (seed: number) => void }) {
  return (
    <button class="s-rules-seed" disabled={props.seed === props.shown} title={props.seed === props.shown ? "This match is shown" : "Show this match"} onClick={() => props.onSeed(props.seed)}>
      seed {props.seed}
    </button>
  );
}

/** A batch of generated matches summed up: who wins how often, how games
 *  end, the worst tick against the budget, and what looks wrong, each
 *  warning with the seed that shows it. */
export function BatchView(props: {
  sim: SimResult | null;
  setup: SimSetup;
  busy: boolean;
  /** Why no match can be generated now; null when one can. */
  blocked: string | null;
  /** The summary is of an older script or setup. */
  stale: boolean;
  trouble: Trouble | null;
  errors: Problem[];
  /** The seed of the match shown beside the summary. */
  shown: number | null;
  onSeed: (seed: number) => void;
  onRetry: () => void;
  onJump: (p: Problem) => void;
}) {
  const sum = () => props.sim?.summary ?? null;
  const teams = () => props.sim?.teams ?? 0;
  const name = (w: SimRow["winner"]) => winnerName(w, teamLabel);
  const top = createMemo(() => (sum()?.wins ?? []).slice(0, WINNERS_SHOWN));
  const rest = createMemo(() => (sum()?.wins ?? []).slice(WINNERS_SHOWN).reduce((n, w) => n + w.matches, 0));
  const warnings = createMemo(() => { const s = sum(); return s ? simWarnings(s) : []; });
  const tick = () => { const s = sum(); return s ? meter(s.worst_tick_ops, s.budget) : { share: 0, tone: "ok" as const }; };
  return (
    <div class="s-rules-batch">
      <Show when={props.trouble}>{(t) => <TroubleNote trouble={t()} onRetry={props.onRetry} />}</Show>
      <Show when={props.errors.length}>
        <ul class="s-rules-errors" aria-label="Errors">
          <For each={props.errors}>{(p) => <li><button class="s-rules-errlink" onClick={() => props.onJump(p)} disabled={p.line === 0}>{problemPlace(p)}</button><span>{p.message}</span></li>}</For>
        </ul>
      </Show>
      <Show when={props.blocked}>
        {(why) => <p class="s-rules-hold">{props.sim ? "These matches are from the last script that checked." : "No matches yet."} {why()}</p>}
      </Show>
      <Show when={sum()} fallback={<Show when={!props.blocked && !props.trouble}><p class="s-rules-muted s-rules-none">{props.busy ? "Playing the matches..." : "The matches show here once the script checks."}</p></Show>}>
        {(s) => (
          <div class="s-rules-sumbox" classList={{ "s-rules-stale": props.stale }}>
            <p class="s-rules-sum">
              <b>{s().ran} {s().ran === 1 ? "match" : "matches"}</b>, {props.setup.players} players, {teams() >= 2 ? `${teams()} teams` : "free for all"}.
              <Show when={teams() >= 2 && !props.setup.teamPlay}> <span class="s-rules-muted">This script needs teams.</span></Show>
            </p>
            <dl class="s-rules-facts">
              <dt>Who wins</dt>
              <dd>
                <ol class="s-rules-wins">
                  <For each={top()}>
                    {(w) => (
                      <li>
                        <span>{name(w.winner)}</span>
                        <span class="s-rules-meter-bar" aria-hidden="true"><i style={{ width: `${((w.matches / Math.max(1, s().ran)) * 100).toFixed(1)}%` }} /></span>
                        <span class="s-rules-wins-n">{ofMatches(w.matches, s().ran)}</span>
                      </li>
                    )}
                  </For>
                  <Show when={rest() > 0}><li class="s-rules-muted"><span>Others</span><span /><span class="s-rules-wins-n">{ofMatches(rest(), s().ran)}</span></li></Show>
                </ol>
              </dd>
              <dt>How games end</dt>
              <dd><For each={s().ends}>{(e, i) => <>{i() > 0 ? ", " : ""}{endLabel(e.end)} <b>{e.matches}</b></>}</For></dd>
              <dt>Winning score</dt>
              <dd>average <b>{s().avg_score.toFixed(1)}</b>, highest <b>{s().max_score}</b></dd>
              <dt>Length</dt>
              <dd>average <b>{shortTime(s().avg_ticks)}</b>, longest <b>{shortTime(s().max_ticks)}</b></dd>
              <dt>Worst tick</dt>
              <dd class={`s-rules-worst s-rules-meter--${tick().tone}`}>
                <span class="s-rules-meter-bar" role="meter" aria-valuemin={0} aria-valuemax={s().budget} aria-valuenow={s().worst_tick_ops} aria-label="Worst tick"><i style={{ width: `${(tick().share * 100).toFixed(1)}%` }} /></span>
                <span><b>{s().worst_tick_ops.toLocaleString("en-US")}</b> of {s().budget.toLocaleString("en-US")} ops, <SeedLink seed={s().worst_tick_seed} shown={props.shown} onSeed={props.onSeed} /></span>
              </dd>
            </dl>
            <h4>Warnings</h4>
            <Show when={warnings().length} fallback={<p class="s-rules-clean s-rules-none"><b>Nothing looks wrong.</b> Every match ended and somebody scored.</p>}>
              <ul class="s-rules-flags">
                <For each={warnings()}>
                  {(w) => (
                    <li classList={{ bad: w.bad }}>
                      <span>{w.text}.</span>
                      <Show when={w.seed !== null}><SeedLink seed={w.seed ?? 0} shown={props.shown} onSeed={props.onSeed} /></Show>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
            <details class="s-rules-all">
              <summary>All {props.sim?.rows.length ?? 0} matches</summary>
              <table class="s-grid s-rules-table">
                <thead><tr><th>Match</th><th>Winner</th><th>Ended</th><th class="num">Length</th><th class="num">Score</th></tr></thead>
                <tbody>
                  <For each={props.sim?.rows ?? []}>
                    {(r) => (
                      <tr classList={{ "s-rules-row-on": r.seed === props.shown }}>
                        <td><SeedLink seed={r.seed} shown={props.shown} onSeed={props.onSeed} /></td>
                        <td>{name(r.winner)}</td>
                        <td>{endLabel(r.end)}</td>
                        <td class="num">{shortTime(r.ticks)}</td>
                        <td class="num">{r.top_score}</td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </details>
          </div>
        )}
      </Show>
    </div>
  );
}

/** Which match the results are of, with the way to its neighbours and
 *  into the hand editor. */
export function MatchBar(props: {
  /** The shown generated match's seed; null before there is one. */
  seed: number | null;
  rows: SimRow[];
  /** The results are of the match edited by hand. */
  custom: boolean;
  onSeed: (seed: number) => void;
  onEdit: () => void;
  onBack: () => void;
}) {
  const near = (step: -1 | 1) => (props.seed === null ? null : neighbourSeed(props.rows, props.seed, step));
  const go = (step: -1 | 1) => { const s = near(step); if (s !== null) props.onSeed(s); };
  return (
    <div class="s-rules-matchbar">
      <Show when={!props.custom} fallback={<><b>Your match</b><span class="s-rules-muted">edited by hand</span><button class="s-btn s-rules-copy" onClick={() => props.onBack()}>Back to generated matches</button></>}>
        <b>{props.seed === null ? "No match yet" : `Seed ${props.seed}`}</b>
        <button class="s-btn" disabled={near(-1) === null} onClick={() => go(-1)}>Previous</button>
        <button class="s-btn" disabled={near(1) === null} onClick={() => go(1)}>Next</button>
        <button class="s-btn s-rules-copy" disabled={props.seed === null} title="Open this match in the hand editor" onClick={() => props.onEdit()}>Edit this match</button>
      </Show>
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
  /** What the match's map lacks for the script's labels, when the run
   *  refused it and nothing ran. */
  map?: string[];
  /** The run button's label and the line beside it. */
  again?: string;
  hint?: string;
  onRun: () => void;
  onJump: (p: Problem) => void;
}) {
  const [copied, setCopied] = createSignal(false);
  const [all, setAll] = createSignal(false);
  const copy = async () => {
    const r = props.ran;
    if (!r) return;
    try { await navigator.clipboard.writeText(traceLines(r.trace).join("\n")); setCopied(true); setTimeout(() => setCopied(false), 1500); } catch { /* no clipboard */ }
  };
  const rows = createMemo(() => (props.ran ? timeline(props.ran.trace, props.ran.scenario, teamLabel) : []));
  const shownRows = () => (all() ? rows() : rows().slice(0, TIMELINE_ROWS));
  const table = createMemo(() => (props.ran ? standings(props.ran.trace, props.ran.scenario) : []));
  const teams = () => (props.ran?.scenario.teamPlay ? teamStandings(table()) : []);
  return (
    <div class="s-rules-results">
      <div class="s-rules-runbar">
        <button class="s-btn" onClick={() => props.onRun()} disabled={!!props.blocked || props.running} title={props.blocked ?? props.again ?? "Run the match again"}>{props.running ? "Running..." : props.again ?? "Run again"}</button>
        <span class="s-rules-muted">{props.hint ?? "Runs by itself when the script or the match changes."}</span>
        <Show when={props.ran}><button class="s-btn s-rules-copy" onClick={() => void copy()}>{copied() ? "Copied" : "Copy as text"}</button></Show>
      </div>
      <Show when={props.trouble}>{(t) => <TroubleNote trouble={t()} onRetry={props.onRun} />}</Show>
      <Show when={props.errors.length}>
        <ul class="s-rules-errors" aria-label="Run errors">
          <For each={props.errors}>{(p) => <li><button class="s-rules-errlink" onClick={() => props.onJump(p)} disabled={p.line === 0}>{problemPlace(p)}</button><span>{p.message}</span></li>}</For>
        </ul>
      </Show>
      <Show when={props.map?.length}>
        <div class="s-rules-hold">
          <p>{props.ran ? "These results are from the last run. " : ""}Nothing ran: the map doesn't pass the script's labels. Fix it under Map, or choose Add what the script needs.</p>
          <ul class="s-rules-issues" aria-label="Map problems">
            <For each={props.map ?? []}>{(p) => <li>{capitalised(p)}.</li>}</For>
          </ul>
        </div>
      </Show>
      <Show when={props.blocked}>
        {(why) => <p class="s-rules-hold">{props.ran ? "These results are from the last run." : "No results yet."} {why()}</p>}
      </Show>
      <Show when={props.ran} fallback={<Show when={!props.blocked && !props.trouble}><p class="s-rules-muted s-rules-none">{props.running ? "Playing the match..." : "The results show here once a match has been played."}</p></Show>}>
        {(r) => (
          <div class="s-rules-run" classList={{ "s-rules-stale": props.stale }}>
            <p class="s-rules-sum">
              <Show when={r().trace.round_ended !== null} fallback={
                <Show when={r().trace.limit} fallback={<>Ran for <b>{tickTime(r().trace.last_tick)}</b> ({r().trace.last_tick.toLocaleString("en-US")} ticks) without the round ending.</>}>
                  The game ended at <b>{tickTime(r().trace.last_tick)}</b> {r().trace.limit === "score" ? "on the score limit" : "when the time ran out"}.
                </Show>
              }>
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

            <HudStrip t={r().trace} scenario={r().scenario} />

            <h4>What happened</h4>
            <Show when={rows().length} fallback={<p class="s-rules-muted s-rules-none">Nothing happened: the match has no events, and the script made no call and changed nothing.</p>}>
              <ol class="s-rules-timeline">
                <For each={shownRows()}>
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
              <Show when={rows().length > TIMELINE_ROWS}>
                <p class="s-rules-more">
                  <span class="s-rules-muted">{all() ? `All ${rows().length} moments are shown.` : `The first ${TIMELINE_ROWS} of ${rows().length} moments are shown.`}</span>
                  <button class="s-btn" onClick={() => setAll((v) => !v)}>{all() ? "Show fewer" : "Show all"}</button>
                </p>
              </Show>
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
}
