// Game type scripts: the rules workbench (docs/game/rules-engine.md,
// "Workbench"). Write a script, check it as you type (the server compiles
// and verifies it), read its budget and capabilities, step a scenario over
// the fake game, and save it when signed in. The pure half is
// ../lib/rules.ts; the draft lives in localStorage.

import { For, Index, Show, createEffect, createResource, createSignal, on, onCleanup } from "solid-js";
import { A, useSearchParams } from "@solidjs/router";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { statsPath } from "../routes";
import { HOME_CRUMB } from "./modes";
import { Crumbs, Panel, Tabs, Title } from "./Layout";
import { shortHash } from "../admin/content";
import {
  EVENT_TYPES, MAX_TICKS, TICKS_PER_SECOND, checkRules, defaultScenario, eventFields, fetchBuiltins, fetchScript,
  fetchScripts, formatCall, formatVars, gutter, loadDraft, meter, meterText, newEvent, nextSlot, offsetOf,
  problemPlace, runFailed, runRules, saveDraft, saveRules, scenarioProblems, tabKey, tickTime, traceLines,
  type CheckResult, type EventField, type EventType, type Problem, type RunFailed, type ScenarioDraft,
  type ScenarioEvent, type Trace,
} from "../lib/rules";
import "../styles/rules.css";

const CRUMB: [string, string] = ["Game Types", statsPath("/rules")];
type Tab = "edit" | "test" | "mine";
const TABS: Array<{ id: Tab; label: string }> = [
  { id: "edit", label: "Script" },
  { id: "test", label: "Test" },
  { id: "mine", label: "My Scripts" },
];

/** What a new draft starts as. */
export const STARTER = `rules "My Game Type" 1
carrier slayer

option kill_points: int 1..5 = 1 "Kill points" "Points for each kill"

on kill(victim, killer) {
    if killer != none and killer != victim {
        score(killer, kill_points)
    }
}
`;

/** How long typing pauses before a check. */
const CHECK_DELAY_MS = 450;
/** The editor's line height in px (rules.css). */
const LINE_PX = 16;

const storage = (): Storage | null => {
  try { return globalThis.localStorage ?? null; } catch { return null; }
};
const int = (v: string, dflt = 0): number => {
  const n = Number.parseInt(v, 10);
  return Number.isFinite(n) ? n : dflt;
};

function Meter(props: { label: string; used: number; limit: number; unit: string }) {
  const m = () => meter(props.used, props.limit);
  return (
    <div class={`s-rules-meter s-rules-meter--${m().tone}`}>
      <span class="s-rules-meter-label">{props.label}</span>
      <span class="s-rules-meter-bar" role="meter" aria-valuemin={0} aria-valuemax={props.limit} aria-valuenow={props.used} aria-label={props.label}>
        <i style={{ width: `${(m().share * 100).toFixed(1)}%` }} />
      </span>
      <span class="s-rules-meter-text">{meterText(props.used, props.limit, props.unit)}</span>
    </div>
  );
}

function BudgetPanel(props: { check: CheckResult | null; busy: boolean }) {
  return (
    <Panel title="Budget" sub={props.busy ? "checking..." : "worst tick, IR size and slots"}>
      <Show when={props.check?.report} fallback={<p class="s-pad s-rules-muted">Check the script to see its budget.</p>}>
        {(r) => (
          <div class="s-rules-meters">
            <Meter label="Worst tick" used={r().worst_tick_ops} limit={r().budget} unit="ops" />
            <Meter label="IR" used={r().ir_bytes} limit={r().ir_limit} unit="bytes" />
            <For each={r().slots}>{(s) => <Meter label={`${s.scope} vars`} used={s.used} limit={s.limit} unit="slots" />}</For>
            <Show when={r().costliest}>{(c) => <p class="s-rules-note">Costliest: <code>on {c()}</code></p>}</Show>
            <Show when={r().handlers.length}>
              <table class="s-rules-handlers">
                <tbody>
                  <For each={r().handlers}>{(h) => <tr><td><code>on {h.event}</code></td><td>{h.worst_ops.toLocaleString("en-US")} ops</td></tr>}</For>
                </tbody>
              </table>
            </Show>
          </div>
        )}
      </Show>
    </Panel>
  );
}

function CapabilityPanel(props: { check: CheckResult | null }) {
  return (
    <Panel title="Capabilities" sub="what the script may do, as the lobby says it">
      <Show when={props.check?.report} fallback={<p class="s-pad s-rules-muted">Nothing checked yet.</p>}>
        {(r) => (
          <>
            <Show when={r().capabilities.length} fallback={<p class="s-pad s-rules-muted">Core only: it scores, ends rounds and reads the game.</p>}>
              <ul class="s-rules-caps">
                <For each={r().capabilities}>
                  {(c) => (
                    <li classList={{ bad: !c.known }}>
                      <b>{c.known ? c.words : "unknown capability"}</b>
                      <small><code>{c.name}</code>{c.grantable ? " · can be granted to players" : ""}</small>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
            <Show when={r().unhostable.length}>
              <p class="s-pad s-rules-warn">No host can run this yet: it calls <For each={r().unhostable}>{(u, i) => <>{i() > 0 ? ", " : ""}<code>{u}</code></>}</For>.</p>
            </Show>
          </>
        )}
      </Show>
    </Panel>
  );
}

/** A select of the seated players, with "the world" for a killer. */
function PlayerPick(props: { value: number | null | undefined; slots: number[]; world?: boolean; label: string; onPick: (v: number | null) => void }) {
  return (
    <label class="s-rules-field">
      <span>{props.label}</span>
      <select value={props.value === null || props.value === undefined ? "world" : String(props.value)} onChange={(e) => props.onPick(e.currentTarget.value === "world" ? null : int(e.currentTarget.value))}>
        <Show when={props.world}><option value="world">the world</option></Show>
        <For each={props.slots}>{(s) => <option value={String(s)}>player {s}</option>}</For>
      </select>
    </label>
  );
}

function EventRow(props: { e: ScenarioEvent; slots: number[]; onChange: (patch: Partial<ScenarioEvent>) => void; onRemove: () => void }) {
  const field = (f: EventField) => {
    switch (f) {
      case "player": return <PlayerPick label="player" value={props.e.player} slots={props.slots} onPick={(v) => props.onChange({ player: v ?? 0 })} />;
      case "victim": return <PlayerPick label="victim" value={props.e.victim} slots={props.slots} onPick={(v) => props.onChange({ victim: v ?? 0 })} />;
      case "killer": return <PlayerPick label="killer" world value={props.e.killer} slots={props.slots} onPick={(v) => props.onChange({ killer: v })} />;
      case "object": case "zone":
        return (
          <label class="s-rules-field">
            <span>{f}</span>
            <input type="number" min={0} value={props.e[f] ?? 0} onChange={(e) => { const n = Math.max(0, int(e.currentTarget.value)); props.onChange(f === "zone" ? { zone: n } : { object: n }); }} />
          </label>
        );
    }
  };
  return (
    <tr>
      <td><input class="s-rules-tick" type="number" min={0} max={MAX_TICKS} value={props.e.tick} aria-label="tick" onChange={(e) => props.onChange({ tick: Math.max(0, int(e.currentTarget.value)) })} /></td>
      <td class="s-rules-time">{tickTime(props.e.tick)}</td>
      <td>
        <select value={props.e.type} aria-label="event" onChange={(e) => props.onChange(newEvent(e.currentTarget.value as EventType, props.e.tick, props.slots.map((slot) => ({ slot, team: 0, lives: 0 }))))}>
          <For each={EVENT_TYPES}>{(t) => <option value={t.type}>{t.label}</option>}</For>
        </select>
      </td>
      <td class="s-rules-fields"><For each={eventFields(props.e.type)}>{field}</For></td>
      <td><button class="s-btn" onClick={() => props.onRemove()} aria-label="remove event">remove</button></td>
    </tr>
  );
}

function ScenarioEditor(props: { scenario: ScenarioDraft; set: (f: (d: ScenarioDraft) => ScenarioDraft) => void }) {
  const slots = () => props.scenario.players.map((p) => p.slot);
  const setPlayer = (i: number, patch: Partial<ScenarioDraft["players"][number]>) =>
    props.set((d) => ({ ...d, players: d.players.map((p, j) => (j === i ? { ...p, ...patch } : p)) }));
  const setEvent = (i: number, patch: Partial<ScenarioEvent>) =>
    props.set((d) => ({ ...d, events: d.events.map((e, j) => (j === i ? { ...e, ...patch } : e)) }));
  const addEvent = () => props.set((d) => {
    const last = d.events.reduce((m, e) => Math.max(m, e.tick), 0);
    const tick = Math.min(d.ticks, last + TICKS_PER_SECOND);
    return { ...d, events: [...d.events, newEvent("kill", tick, d.players)] };
  });
  return (
    <div class="s-rules-scenario">
      <div class="s-toolbar s-rules-settings">
        <label>Last tick <input type="number" min={0} max={MAX_TICKS} value={props.scenario.ticks} onChange={(e) => props.set((d) => ({ ...d, ticks: Math.min(MAX_TICKS, Math.max(0, int(e.currentTarget.value))) }))} /></label>
        <span class="s-rules-muted">({tickTime(props.scenario.ticks)})</span>
        <label>Round time (s) <input type="number" min={0} value={Math.round(props.scenario.roundTime / TICKS_PER_SECOND)} onChange={(e) => props.set((d) => ({ ...d, roundTime: Math.max(0, int(e.currentTarget.value)) * TICKS_PER_SECOND }))} /></label>
        <label>Seed <input type="number" min={0} value={props.scenario.seed} onChange={(e) => props.set((d) => ({ ...d, seed: Math.max(0, int(e.currentTarget.value)) }))} /></label>
        <label><input type="checkbox" checked={props.scenario.teamPlay} onChange={(e) => props.set((d) => ({ ...d, teamPlay: e.currentTarget.checked }))} /> Team play</label>
        <button class="s-btn" onClick={() => props.set(() => defaultScenario())}>Reset</button>
      </div>
      <div class="s-rules-split">
        <section>
          <h4>Players</h4>
          <table class="s-grid s-rules-table">
            <thead><tr><th>Slot</th><th>Team</th><th>Lives</th><th /></tr></thead>
            <tbody>
              <Index each={props.scenario.players}>
                {(p, i) => (
                  <tr>
                    <td>
                      <select value={String(p().slot)} aria-label="slot" onChange={(e) => setPlayer(i, { slot: int(e.currentTarget.value) })}>
                        <For each={Array.from({ length: 16 }, (_, s) => s)}>{(s) => <option value={String(s)} disabled={s !== p().slot && slots().includes(s)}>{s}</option>}</For>
                      </select>
                    </td>
                    <td>
                      <select value={String(p().team)} aria-label="team" onChange={(e) => setPlayer(i, { team: int(e.currentTarget.value) })}>
                        <For each={Array.from({ length: 8 }, (_, t) => t)}>{(t) => <option value={String(t)}>team {t}</option>}</For>
                      </select>
                    </td>
                    <td><input type="number" class="s-rules-tick" value={p().lives} aria-label="lives" onChange={(e) => setPlayer(i, { lives: int(e.currentTarget.value) })} /></td>
                    <td><button class="s-btn" onClick={() => props.set((d) => ({ ...d, players: d.players.filter((_, j) => j !== i) }))}>remove</button></td>
                  </tr>
                )}
              </Index>
            </tbody>
          </table>
          <button class="s-btn s-rules-add" disabled={nextSlot(props.scenario.players) === null}
            onClick={() => props.set((d) => { const slot = nextSlot(d.players); return slot === null ? d : { ...d, players: [...d.players, { slot, team: 0, lives: 0 }] }; })}>
            Add player
          </button>
        </section>
        <section>
          <h4>Events</h4>
          <table class="s-grid s-rules-table">
            <thead><tr><th>Tick</th><th>Time</th><th>Event</th><th>Who</th><th /></tr></thead>
            <tbody>
              <Index each={props.scenario.events}>
                {(e, i) => <EventRow e={e()} slots={slots()} onChange={(patch) => setEvent(i, patch)} onRemove={() => props.set((d) => ({ ...d, events: d.events.filter((_, j) => j !== i) }))} />}
              </Index>
            </tbody>
          </table>
          <button class="s-btn s-rules-add" onClick={addEvent}>Add event</button>
        </section>
      </div>
    </div>
  );
}

function TraceView(props: { trace: Trace }) {
  const [copied, setCopied] = createSignal(false);
  const copy = async () => {
    try { await navigator.clipboard.writeText(traceLines(props.trace).join("\n")); setCopied(true); setTimeout(() => setCopied(false), 1500); } catch { /* no clipboard */ }
  };
  return (
    <div class="s-rules-trace">
      <div class="s-toolbar">
        <span>
          <Show when={props.trace.round_ended !== null} fallback={<>Ran to tick {props.trace.last_tick} ({tickTime(props.trace.last_tick)})</>}>
            Round ended at tick {props.trace.round_ended}
          </Show>
          {" · "}{props.trace.ops.toLocaleString("en-US")} ops · scores: {props.trace.scores.map((s) => `player ${s.slot} ${s.score}`).join(", ") || "none"}
        </span>
        <button class="s-btn" onClick={() => void copy()}>{copied() ? "Copied" : "Copy as text"}</button>
      </div>
      <Show when={props.trace.error}>{(e) => <p class="s-pad s-rules-bad">{e()}</p>}</Show>
      <Show when={props.trace.cut}>{(c) => <p class="s-pad s-rules-warn">{c()}</p>}</Show>
      <Show when={props.trace.ticks.length} fallback={<p class="s-pad s-rules-muted">Nothing happened: no handler ran, no call was made and no variable changed.</p>}>
        <table class="s-grid s-rules-table">
          <thead><tr><th>Tick</th><th>Time</th><th>Ran</th><th>Calls</th><th>Variables</th></tr></thead>
          <tbody>
            <For each={props.trace.ticks}>
              {(k) => (
                <tr classList={{ "s-rules-errrow": !!k.error }}>
                  <td>{k.tick}</td>
                  <td class="s-rules-time">{tickTime(k.tick)}</td>
                  <td><For each={k.ran}>{(r) => <code class="s-rules-chip">on {r}</code>}</For></td>
                  <td>
                    <For each={k.calls}>{(c) => <div><code>{formatCall(c)}</code></div>}</For>
                    <For each={k.warnings}>{(w) => <div class="s-rules-warn">{w}</div>}</For>
                    <Show when={k.error}>{(e) => <div class="s-rules-bad">{e()}</div>}</Show>
                  </td>
                  <td>
                    <Show when={k.globals.length}><div>{formatVars(k.globals)}</div></Show>
                    <For each={k.players}>
                      {(p) => <div class="s-rules-muted">player {p.slot}: team {p.team}, {p.alive ? "alive" : "dead"}, score {p.score}{p.vars.length ? `, ${formatVars(p.vars)}` : ""}</div>}
                    </For>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
    </div>
  );
}

export default function RulesPage() {
  const [sp, setSp] = useSearchParams();
  const tab = (): Tab => (sp.tab === "test" || sp.tab === "mine" ? sp.tab : "edit");
  const first = loadDraft(storage(), STARTER);
  const [source, setSource] = createSignal(first.source);
  const [scenario, setScenario] = createSignal<ScenarioDraft>(first.scenario);
  const [check, setCheck] = createSignal<CheckResult | null>(null);
  const [checking, setChecking] = createSignal(false);
  const [checkErr, setCheckErr] = createSignal<string | null>(null);
  const [trace, setTrace] = createSignal<Trace | RunFailed | null>(null);
  const [running, setRunning] = createSignal(false);
  const [runErr, setRunErr] = createSignal<string | null>(null);
  const [saveMsg, setSaveMsg] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  let editor: HTMLTextAreaElement | undefined;
  let gutterEl: HTMLPreElement | undefined;
  let picker: HTMLDialogElement | undefined;

  createEffect(on([source, scenario], ([s, d]) => saveDraft(storage(), { source: s, scenario: d })));

  // Check as you type: one request a pause, the older one abandoned.
  let timer: ReturnType<typeof setTimeout> | undefined;
  let inflight: AbortController | undefined;
  async function runCheck() {
    inflight?.abort();
    const ctl = new AbortController();
    inflight = ctl;
    setChecking(true);
    try {
      const c = await checkRules(live.server().api, source(), fetch, ctl.signal);
      if (inflight === ctl) { setCheck(c); setCheckErr(null); }
    } catch (e) {
      if (inflight === ctl && (e as Error).name !== "AbortError") setCheckErr((e as Error).message);
    } finally {
      if (inflight === ctl) setChecking(false);
    }
  }
  createEffect(on([source, () => live.server().api], () => {
    clearTimeout(timer);
    timer = setTimeout(() => void runCheck(), CHECK_DELAY_MS);
  }));
  onCleanup(() => { clearTimeout(timer); inflight?.abort(); });

  function jumpTo(p: Problem) {
    if (!editor) return;
    if (tab() !== "edit") setSp({ tab: undefined });
    const at = offsetOf(source(), p.line, p.col);
    editor.focus();
    editor.setSelectionRange(at, Math.min(source().length, at + 1));
    editor.scrollTop = Math.max(0, (p.line - 4) * LINE_PX);
  }

  function onKey(e: KeyboardEvent & { currentTarget: HTMLTextAreaElement }) {
    if (e.key !== "Tab" || e.altKey || e.ctrlKey || e.metaKey) return;
    e.preventDefault();
    const ta = e.currentTarget;
    const edit = tabKey(ta.value, ta.selectionStart, ta.selectionEnd, e.shiftKey);
    ta.value = edit.value;
    ta.setSelectionRange(edit.start, edit.end);
    setSource(edit.value);
  }

  function replace(text: string) {
    if (source() !== text && source().trim() && source() !== STARTER && !confirm("Replace the script in the editor?")) return;
    setSource(text);
    setTrace(null);
    picker?.close();
    if (tab() === "mine") setSp({ tab: undefined });
  }

  const [builtins] = createResource(() => live.server().api, (api) => fetchBuiltins(api).catch(() => []));
  const [mine, { refetch: refetchMine }] = createResource(
    () => { const me = identity.me(); return me ? [live.server().api, me.gamertag] as const : null; },
    ([api, tag]) => fetchScripts(api, tag).catch(() => []),
  );

  async function openSaved(hash: string) {
    try {
      const s = await fetchScript(live.server().api, hash);
      if (s) replace(s.source);
    } catch (e) {
      setSaveMsg(`couldn't open it: ${(e as Error).message}`);
    }
  }

  async function run() {
    const problems = scenarioProblems(scenario());
    if (problems.length) { setRunErr(problems.join("; ")); return; }
    setRunning(true);
    setRunErr(null);
    try { setTrace(await runRules(live.server().api, source(), scenario())); } catch (e) { setRunErr((e as Error).message); } finally { setRunning(false); }
  }

  async function save() {
    const me = identity.me(), wk = live.wellKnown();
    if (!me || !wk) return;
    setSaving(true);
    setSaveMsg(null);
    try {
      const r = await saveRules(live.server().api, wk.id, me, source());
      setSaveMsg(r.saved ? `Saved as ${shortHash(r.script?.hash ?? "")}.` : `Already saved as ${shortHash(r.script?.hash ?? "")}${r.script?.gamertag ? ` by ${r.script.gamertag}` : ""}.`);
      void refetchMine();
    } catch (e) {
      setSaveMsg(`couldn't save: ${(e as Error).message}`);
    } finally {
      setSaving(false);
    }
  }

  const errors = () => check()?.errors ?? [];

  return (
    <>
      <Crumbs trail={[HOME_CRUMB, [CRUMB[0]]]} />
      <div class="s-cols">
        <aside class="s-left">
          <Title>Game Types</Title>
          <nav class="s-pills" aria-label="Game types">
            <For each={TABS}>{(t) => <A href={`${statsPath("/rules")}${t.id === "edit" ? "" : `?tab=${t.id}`}`} activeClass="" classList={{ active: tab() === t.id }} aria-current={tab() === t.id ? "page" : undefined}>{t.label === "Script" ? "Workbench" : t.label}</A>}</For>
          </nav>
          <BudgetPanel check={check()} busy={checking()} />
          <CapabilityPanel check={check()} />
        </aside>
        <div class="s-right">
          <Title class="s-title--cap" help="Write a game type script, check it as you type, test it on a fake game and save it to Live. The server compiles and verifies every check; nothing runs in a real match from here.">Game Type Scripts</Title>
          <Tabs label="Workbench" active={tab()} tabs={TABS.map((t) => ({ id: t.id, label: t.label }))} onPick={(id) => setSp({ tab: id === "edit" ? undefined : id })} />
          <div class="s-matte s-rules-wrap">
            <div class="s-toolbar s-rules-bar">
              <button class="s-btn" onClick={() => picker?.showModal()}>Start from...</button>
              <button class="s-btn" onClick={() => { clearTimeout(timer); void runCheck(); }} disabled={checking()}>Check</button>
              <Show when={identity.me()} fallback={<A href="/dash/signup">Sign in to save</A>}>
                <button class="s-btn" onClick={() => void save()} disabled={saving() || !check()?.ok}>Save</button>
              </Show>
              <span class="s-rules-status">
                <Show when={check()} fallback={checking() ? "checking..." : ""}>
                  {(c) => (
                    <Show when={c().ok} fallback={<span class="s-rules-bad">{c().errors.length} {c().errors.length === 1 ? "error" : "errors"}</span>}>
                      <span class="s-rules-good">verifies</span> · <b>{c().name}</b> · <code title={c().hash ?? ""}>{shortHash(c().hash ?? "")}</code>
                    </Show>
                  )}
                </Show>
              </span>
            </div>
            <Show when={checkErr()}>{(e) => <p class="s-pad s-rules-bad">Check failed: {e()}</p>}</Show>
            <Show when={saveMsg()}>{(m) => <p class="s-pad s-rules-note">{m()}</p>}</Show>

            <div class="s-rules-editor" hidden={tab() !== "edit"}>
              <pre class="s-rules-gutter" ref={gutterEl} aria-hidden="true">{gutter(source())}</pre>
              <textarea
                ref={editor}
                class="s-rules-text"
                spellcheck={false}
                aria-label="Script source"
                value={source()}
                onInput={(e) => setSource(e.currentTarget.value)}
                onKeyDown={onKey}
                onScroll={(e) => { if (gutterEl) gutterEl.scrollTop = e.currentTarget.scrollTop; }}
              />
            </div>
            <Show when={tab() === "edit"}>
              <Show when={errors().length}>
                <ul class="s-rules-errors" aria-label="Errors">
                  <For each={errors()}>
                    {(p) => (
                      <li>
                        <button class="s-rules-errlink" onClick={() => jumpTo(p)} disabled={p.line === 0}>{problemPlace(p)}</button>
                        <span>{p.message}</span>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
            </Show>

            <Show when={tab() === "test"}>
              <ScenarioEditor scenario={scenario()} set={(f) => setScenario(f)} />
              <div class="s-toolbar">
                <button class="s-btn" onClick={() => void run()} disabled={running()}>{running() ? "Running..." : "Run"}</button>
                <Show when={runErr()}>{(e) => <span class="s-rules-bad">{e()}</span>}</Show>
              </div>
              <Show when={trace()}>
                {(t) => {
                  const v = t();
                  return runFailed(v)
                    ? (
                      <ul class="s-rules-errors">
                        <For each={v.errors}>{(p) => <li><button class="s-rules-errlink" onClick={() => jumpTo(p)} disabled={p.line === 0}>{problemPlace(p)}</button><span>{p.message}</span></li>}</For>
                      </ul>
                    )
                    : <TraceView trace={v} />;
                }}
              </Show>
            </Show>

            <Show when={tab() === "mine"}>
              <Show when={identity.me()} fallback={<p class="s-pad">Sign in to save scripts and see yours here. <A href="/dash/signup">Sign up</A></p>}>
                <Show when={!mine.loading} fallback={<p class="s-pad s-rules-muted">Loading...</p>}>
                  <Show when={(mine() ?? []).length} fallback={<p class="s-pad s-rules-muted">No saved scripts yet. Save one from the Script tab.</p>}>
                    <table class="s-grid">
                      <thead><tr><th>Name</th><th>Version</th><th>Hash</th><th>Saved</th><th /></tr></thead>
                      <tbody>
                        <For each={mine() ?? []}>
                          {(s) => (
                            <tr>
                              <td>{s.name}</td>
                              <td>{s.version}</td>
                              <td><code title={s.hash}>{shortHash(s.hash)}</code></td>
                              <td>{s.saved_at.slice(0, 10)}</td>
                              <td><button class="s-btn" onClick={() => void openSaved(s.hash)}>Open</button></td>
                            </tr>
                          )}
                        </For>
                      </tbody>
                    </table>
                  </Show>
                </Show>
              </Show>
            </Show>
          </div>
        </div>
      </div>

      <dialog ref={picker} class="s-rules-dialog" aria-labelledby="s-rules-pick-title" onClick={(e) => { if (e.target === e.currentTarget) picker?.close(); }}>
        <h3 id="s-rules-pick-title">Start from</h3>
        <div class="s-rules-dialog-body">
          <ul class="s-rules-picks">
            <li><button onClick={() => replace(STARTER)}><b>Blank game type</b><small>a kill-scoring starter</small></button></li>
            <For each={builtins() ?? []}>
              {(b) => <li><button onClick={() => replace(b.source)}><b>{b.name}</b><small>built-in · engine {b.engine}</small></button></li>}
            </For>
            <Show when={identity.me()}>
              <For each={mine() ?? []}>
                {(s) => <li><button onClick={() => void openSaved(s.hash)}><b>{s.name}</b><small>yours · {shortHash(s.hash)}</small></button></li>}
              </For>
            </Show>
          </ul>
        </div>
        <footer><button class="s-btn" onClick={() => picker?.close()}>Cancel</button></footer>
      </dialog>
    </>
  );
}
