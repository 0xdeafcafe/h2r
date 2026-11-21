// Game type scripts: the rules workbench (docs/game/rules-engine.md,
// "Workbench"). Write a script, check it as you type (the server compiles
// and verifies it), read its budget and capabilities, step a scenario over
// the fake game, and save it when signed in. The pure half is
// ../lib/rules.ts (the editor's, ../lib/rulesIde.ts); the editor is
// RulesEditor (Monaco, loaded on first use); the draft lives in
// localStorage. "Make a game variant" turns a checked script into a
// game variant (../lib/variants.ts): a file to download, or published to
// Live when signed in; My Game Types lists yours. `?script=<hash>` opens
// a saved or built-in script read-only (the variant page links here).

import { For, Index, Show, createEffect, createResource, createSignal, on, onCleanup } from "solid-js";
import { A, useSearchParams } from "@solidjs/router";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { statsPath } from "../routes";
import { HOME_CRUMB } from "./modes";
import { Crumbs, Panel, Tabs, Title } from "./Layout";
import { shortHash } from "../admin/content";
import { issueToken } from "../admin/token";
import { usDate } from "./fmt";
import {
  EVENT_TYPES, MAX_TICKS, TICKS_PER_SECOND, carrierOf, checkRules, defaultScenario, eventFields, fetchBuiltins, fetchScript,
  fetchScripts, formatCall, formatVars, loadDraft, meter, meterText, newEvent, nextSlot,
  problemPlace, runFailed, runRules, saveDraft, saveRules, scenarioProblems, tickTime, traceLines,
  type CheckResult, type EventField, type EventType, type Problem, type RunFailed, type ScenarioDraft,
  type ScenarioEvent, type ScriptOption, type Trace,
} from "../lib/rules";
import {
  VARIANT_DESCRIPTION_MAX, VARIANT_NAME_MAX, coerceOption, coerceOptions, defaultOptions, deleteVariant, descriptionProblem,
  downloadBundle, fetchVariants, makeBundle, nameProblem, notFound, publishFromScript, publishVariant, variantHref,
  type OptionValue, type OptionValues, type ScriptRef, type Variant, type Visibility,
} from "../lib/variants";
import { fetchMenuCompletions, insertMenus, type InsertItem, type InsertMenu, type InsertMenuId } from "../lib/rulesIde";
import RulesEditor, { type RulesEditorApi } from "./RulesEditor";
import "../styles/rules.css";

const CRUMB: [string, string] = ["Game Types", statsPath("/rules")];
type Tab = "edit" | "test" | "mine" | "types";
const TABS: Array<{ id: Tab; label: string }> = [
  { id: "edit", label: "Script" },
  { id: "test", label: "Test" },
  { id: "mine", label: "My Scripts" },
  { id: "types", label: "My Game Types" },
];
/** A script shown read-only until "Use as start": a built-in, or a saved
 *  one opened from a link. */
interface Lock { name: string; kind: "built-in" | "saved" }

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
/** The Insert menus before (or without) the completion route's lists. */
const STATIC_MENUS = insertMenus(null, null);

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

/** One declared option's input in the variant dialog. */
function OptionInput(props: { option: ScriptOption; value: OptionValue | undefined; onChange: (v: OptionValue) => void }) {
  const o = () => props.option;
  return (
    <label class="s-rules-row" title={o().description ?? undefined}>
      <span>{o().label}</span>
      {o().kind === "bool"
        ? <input type="checkbox" checked={props.value === true} onChange={(e) => props.onChange(e.currentTarget.checked)} />
        : o().kind === "enum"
          ? (
            <select value={typeof props.value === "string" ? props.value : ""} onChange={(e) => props.onChange(e.currentTarget.value)}>
              <For each={o().values}>{(v) => <option value={v}>{v}</option>}</For>
            </select>
          )
          : <input type="number" min={o().lo} max={o().hi} step={1} value={typeof props.value === "number" ? props.value : ""} onChange={(e) => props.onChange(e.currentTarget.value)} />}
      <small class="s-rules-muted">{o().kind === "int" ? `${o().lo} to ${o().hi}` : o().description ?? ""}</small>
    </label>
  );
}

export default function RulesPage() {
  const [sp, setSp] = useSearchParams();
  const tab = (): Tab => (sp.tab === "test" || sp.tab === "mine" || sp.tab === "types" ? sp.tab : "edit");
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
  /** The script shown read-only until "Use as start". */
  const [locked, setLocked] = createSignal<Lock | null>(null);
  /** Hashes this page has seen saved on the server (opened or saved here),
   *  beside the built-ins and your own: a variant names those by hash. */
  const [onServer, setOnServer] = createSignal<ReadonlySet<string>>(new Set());
  const markOnServer = (hash: string | null | undefined) => { if (hash) setOnServer((s) => new Set([...s, hash])); };
  const [menuId, setMenuId] = createSignal<InsertMenuId | null>(null);
  let editor: RulesEditorApi | undefined;
  let picker: HTMLDialogElement | undefined;
  let inserter: HTMLDialogElement | undefined;
  let maker: HTMLDialogElement | undefined;

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

  // An error's place: back to the Script tab (the editor is only hidden
  // on the others), then select its marker once the box is shown.
  function jumpTo(p: Problem) {
    if (tab() !== "edit") setSp({ tab: undefined });
    setTimeout(() => editor?.reveal(p), 0);
  }

  /** Puts a script in the editor; a lock opens it read-only. Whether it
   *  did (the replace can be refused). */
  function replace(text: string, lock: Lock | null = null): boolean {
    if (source() !== text && source().trim() && source() !== STARTER && !locked() && !confirm("Replace the script in the editor?")) return false;
    setSource(text);
    setLocked(lock);
    setTrace(null);
    picker?.close();
    if (tab() === "mine" || tab() === "types") setSp({ tab: undefined });
    return true;
  }

  // The Insert menus: the completion route's events and API over the
  // page's own lists.
  const [routeMenus] = createResource(
    () => live.server().api,
    async (api) => { const r = await fetchMenuCompletions(api); return insertMenus(r.top, r.body); },
  );
  const menus = (): InsertMenu[] => routeMenus.latest ?? STATIC_MENUS;
  const menu = (): InsertMenu | undefined => menus().find((m) => m.id === menuId());
  function openMenu(id: InsertMenuId) {
    setMenuId(id);
    inserter?.showModal();
  }
  function insertItem(it: InsertItem) {
    inserter?.close();
    editor?.insert(it.snippet);
  }

  const [builtins] = createResource(() => live.server().api, (api) => fetchBuiltins(api).catch(() => []));
  const [mine, { refetch: refetchMine }] = createResource(
    () => { const me = identity.me(); return me ? [live.server().api, me.gamertag] as const : null; },
    ([api, tag]) => fetchScripts(api, tag).catch(() => []),
  );

  async function openSaved(hash: string) {
    try {
      const s = await fetchScript(live.server().api, hash);
      if (s && replace(s.source)) markOnServer(s.hash);
    } catch (e) {
      setSaveMsg(`couldn't open it: ${(e as Error).message}`);
    }
  }

  /** A script by hash, read-only: a saved one, else a built-in. */
  async function openLinked(hash: string) {
    const api = live.server().api;
    try {
      const s = await fetchScript(api, hash).catch((e: unknown) => { if (notFound(e)) return null; throw e; });
      if (s) {
        if (replace(s.source, { name: s.name, kind: "saved" })) markOnServer(s.hash);
        return;
      }
      const b = (await fetchBuiltins(api)).find((x) => x.hash === hash);
      if (b) replace(b.source, { name: b.name, kind: "built-in" });
      else setSaveMsg(`No script on this server has the hash ${shortHash(hash)}.`);
    } catch (e) {
      setSaveMsg(`couldn't open it: ${(e as Error).message}`);
    }
  }
  createEffect(on([() => sp.script, () => live.server().api], ([hash]) => {
    if (typeof hash !== "string" || !/^[0-9a-f]{64}$/.test(hash)) return;
    setSp({ script: undefined }, { replace: true });
    void openLinked(hash);
  }));

  /** Whether the checked script is on the server, so a variant can name it
   *  by hash rather than send its source. */
  const savedHash = (): string | null => {
    const h = check()?.ok ? check()?.hash ?? null : null;
    if (!h) return null;
    const known = onServer().has(h) || (builtins() ?? []).some((b) => b.hash === h) || (mine() ?? []).some((s) => s.hash === h);
    return known ? h : null;
  };

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
      markOnServer(r.script?.hash);
      setSaveMsg(r.saved ? `Saved as ${shortHash(r.script?.hash ?? "")}.` : `Already saved as ${shortHash(r.script?.hash ?? "")}${r.script?.gamertag ? ` by ${r.script.gamertag}` : ""}.`);
      void refetchMine();
    } catch (e) {
      setSaveMsg(`couldn't save: ${(e as Error).message}`);
    } finally {
      setSaving(false);
    }
  }

  // ---------- game variants ----------

  const [vName, setVName] = createSignal("");
  const [vDesc, setVDesc] = createSignal("");
  const [vVis, setVVis] = createSignal<Visibility>("public");
  const [vOpts, setVOpts] = createSignal<OptionValues>({});
  const [vBusy, setVBusy] = createSignal(false);
  const [vMsg, setVMsg] = createSignal<string | null>(null);
  const [vMade, setVMade] = createSignal<Variant | null>(null);
  // A check that lists no options (none declared, or a server before the
  // check listed them) shows no inputs and sends `options: {}`: the server
  // fills the defaults.
  const declared = (): ScriptOption[] => check()?.options ?? [];
  const carrier = () => check()?.carrier ?? carrierOf(source());
  const nameErr = () => nameProblem(vName());
  const descErr = () => descriptionProblem(vDesc());
  const variantOk = () => !!check()?.ok && !!check()?.hash && !nameErr() && !descErr();

  function openMaker() {
    const c = check();
    if (!c?.ok) return;
    setVName((c.name ?? "").trim().slice(0, VARIANT_NAME_MAX).trim());
    setVDesc("");
    setVOpts(defaultOptions(c.options));
    setVMsg(null);
    setVMade(null);
    maker?.showModal();
  }

  /** The script as a variant names it: its hash when the server has it,
   *  else its source. */
  const scriptRef = (): ScriptRef => {
    const h = savedHash();
    return h ? { script_hash: h } : { source: source() };
  };
  const variantRequest = () => ({ name: vName(), description: vDesc(), options: coerceOptions(declared(), vOpts()) });

  async function downloadVariant() {
    if (!variantOk()) return;
    setVBusy(true);
    setVMsg(null);
    try {
      const r = await makeBundle(live.server().api, scriptRef(), variantRequest());
      downloadBundle(r.bundle, r.file_name);
    } catch (e) {
      setVMsg(`couldn't make the file: ${(e as Error).message}`);
    } finally {
      setVBusy(false);
    }
  }

  // Your variants, unlisted ones too: the list is read with your token.
  const [myTypes, { refetch: refetchTypes }] = createResource(
    () => { const me = identity.me(), wk = live.wellKnown(); return me && wk ? [live.server().api, me, wk.id] as const : null; },
    async ([api, me, aud]) => fetchVariants(api, me.xuid, await issueToken(me, aud, "read")),
  );

  async function publish() {
    const me = identity.me(), wk = live.wellKnown();
    if (!me || !wk || !variantOk()) return;
    setVBusy(true);
    setVMsg(null);
    try {
      const api = live.server().api, ref = scriptRef(), req = variantRequest();
      const v = "script_hash" in ref
        ? await publishFromScript(api, wk.id, me, { script_hash: ref.script_hash, ...req, visibility: vVis() })
        : await publishVariant(api, wk.id, me, (await makeBundle(api, ref, req)).bundle, vVis());
      setVMade(v);
      void refetchTypes();
    } catch (e) {
      setVMsg(`couldn't publish it: ${(e as Error).message}`);
    } finally {
      setVBusy(false);
    }
  }

  const [deleting, setDeleting] = createSignal<string | null>(null);
  async function removeType(v: Variant) {
    const me = identity.me(), wk = live.wellKnown();
    if (!me || !wk || !confirm(`Delete the game variant "${v.name}"? Its link stops working.`)) return;
    setDeleting(v.id);
    try {
      await deleteVariant(live.server().api, wk.id, me, v.id);
      void refetchTypes();
    } catch (e) {
      setSaveMsg(`couldn't delete it: ${(e as Error).message}`);
    } finally {
      setDeleting(null);
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
                <button class="s-btn" onClick={() => void save()} disabled={saving() || !!locked() || !check()?.ok}>Save</button>
              </Show>
              <button class="s-btn" onClick={() => openMaker()} disabled={!check()?.ok || !check()?.hash}>Make a game variant</button>
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

            <div class="s-rules-editwrap" hidden={tab() !== "edit"}>
              <Show when={locked()}>
                {(l) => (
                  <div class="s-rules-locked">
                    <span>{l().kind === "built-in" ? "The built-in" : "The script"} <b>{l().name}</b> is read-only.</span>
                    <button class="s-btn" onClick={() => { setLocked(null); editor?.focus(); }}>Use as start</button>
                  </div>
                )}
              </Show>
              <div class="s-toolbar s-rules-insert" role="toolbar" aria-label="Insert">
                <span class="s-rules-insert-label">Insert</span>
                <For each={menus()}>
                  {(m) => <button class="s-btn" disabled={!!locked()} onClick={() => openMenu(m.id)}>{m.label}...</button>}
                </For>
                <span class="s-rules-keys">Shift+Alt+F formats · F12 goes to a definition · Shift+F12 finds references · Ctrl (Cmd)+Shift+O outlines</span>
              </div>
              <RulesEditor
                value={source()}
                onChange={(v) => setSource(v)}
                readOnly={!!locked()}
                problems={errors()}
                api={live.server().api}
                onReady={(api) => { editor = api; }}
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

            <Show when={tab() === "types"}>
              <Show when={identity.me()} fallback={<p class="s-pad">Sign in to publish game variants and see yours here. <A href="/dash/signup">Sign up</A></p>}>
                <Show when={!myTypes.loading} fallback={<p class="s-pad s-rules-muted">Loading...</p>}>
                  <Show when={!myTypes.error} fallback={<p class="s-pad s-rules-bad">Couldn't load your game variants.</p>}>
                    <Show when={(myTypes() ?? []).length} fallback={<p class="s-pad s-rules-muted">No game variants yet. Check a script, then use Make a game variant.</p>}>
                      <table class="s-grid">
                        <thead><tr><th>Name</th><th>Game type</th><th>Script</th><th>Visibility</th><th>Created</th><th /></tr></thead>
                        <tbody>
                          <For each={myTypes() ?? []}>
                            {(v) => (
                              <tr>
                                <td><A href={variantHref(v.id)}>{v.name}</A></td>
                                <td>{v.game_type}</td>
                                <td>{v.script_name ?? <span class="s-rules-muted">none</span>}</td>
                                <td>{v.visibility === "unlisted" ? "Unlisted" : "Public"}</td>
                                <td>{usDate(v.created)}</td>
                                <td><button class="s-btn" onClick={() => void removeType(v)} disabled={deleting() === v.id}>Delete</button></td>
                              </tr>
                            )}
                          </For>
                        </tbody>
                      </table>
                    </Show>
                  </Show>
                </Show>
              </Show>
            </Show>
          </div>
        </div>
      </div>

      <dialog ref={maker} class="s-rules-dialog s-rules-variant-dialog" aria-labelledby="s-rules-variant-title" onClick={(e) => { if (e.target === e.currentTarget) maker?.close(); }}>
        <h3 id="s-rules-variant-title">Make a game variant</h3>
        <form class="s-rules-dialog-body s-rules-variant-form" onSubmit={(e) => e.preventDefault()}>
          <p class="s-rules-note">
            Script: <b>{check()?.name ?? "untitled"}</b> · Base game type: <b>{carrier()?.label ?? "unknown"}</b>
          </p>
          <label class="s-rules-row">
            <span>Name</span>
            <input type="text" maxLength={VARIANT_NAME_MAX} value={vName()} onInput={(e) => setVName(e.currentTarget.value)} aria-invalid={!!nameErr()} />
          </label>
          <Show when={nameErr()}>{(m) => <p class="s-rules-bad s-rules-hint">{m()}</p>}</Show>
          <label class="s-rules-row">
            <span>Description</span>
            <textarea rows={3} maxLength={VARIANT_DESCRIPTION_MAX} value={vDesc()} onInput={(e) => setVDesc(e.currentTarget.value)} aria-invalid={!!descErr()} />
          </label>
          <Show when={descErr()}>{(m) => <p class="s-rules-bad s-rules-hint">{m()}</p>}</Show>
          <Show when={identity.me()}>
            <label class="s-rules-row">
              <span>Visibility</span>
              <select value={vVis()} onChange={(e) => setVVis(e.currentTarget.value === "unlisted" ? "unlisted" : "public")}>
                <option value="public">Public: on your player page</option>
                <option value="unlisted">Unlisted: only by its link</option>
              </select>
            </label>
          </Show>
          <Show when={declared().length} fallback={<p class="s-rules-muted s-rules-hint">The script declares no options.</p>}>
            <h4>Options</h4>
            <For each={declared()}>{(o) => <OptionInput option={o} value={vOpts()[o.name]} onChange={(v) => setVOpts((m) => ({ ...m, [o.name]: coerceOption(o, v) }))} />}</For>
          </Show>
          <Show when={vMsg()}>{(m) => <p class="s-rules-bad s-rules-hint">{m()}</p>}</Show>
          <Show when={vMade()}>{(v) => <p class="s-rules-good s-rules-hint">Published: <A href={variantHref(v().id)}>{v().name}</A></p>}</Show>
        </form>
        <footer class="s-rules-variant-foot">
          <button class="s-btn" onClick={() => void downloadVariant()} disabled={vBusy() || !variantOk()}>Download .h2variant</button>
          <Show when={identity.me()} fallback={<A href="/dash/signup">Sign in to publish</A>}>
            <button class="s-btn" onClick={() => void publish()} disabled={vBusy() || !variantOk() || !live.wellKnown()}>Publish</button>
          </Show>
          <button class="s-btn" onClick={() => maker?.close()}>Close</button>
        </footer>
      </dialog>

      <dialog ref={picker} class="s-rules-dialog" aria-labelledby="s-rules-pick-title" onClick={(e) => { if (e.target === e.currentTarget) picker?.close(); }}>
        <h3 id="s-rules-pick-title">Start from</h3>
        <div class="s-rules-dialog-body">
          <ul class="s-rules-picks">
            <li><button onClick={() => replace(STARTER)}><b>Blank game type</b><small>a kill-scoring starter</small></button></li>
            <For each={builtins() ?? []}>
              {(b) => <li><button onClick={() => replace(b.source, { name: b.name, kind: "built-in" })}><b>{b.name}</b><small>built-in · engine {b.engine} · opens read-only</small></button></li>}
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

      <dialog ref={inserter} class="s-rules-dialog s-rules-insert-dialog" aria-labelledby="s-rules-insert-title" onClick={(e) => { if (e.target === e.currentTarget) inserter?.close(); }}>
        <h3 id="s-rules-insert-title">Insert: {menu()?.label ?? ""}</h3>
        <div class="s-rules-dialog-body">
          <For each={menu()?.groups ?? []}>
            {(g) => (
              <section class="s-rules-insert-group">
                <h4>{g.title} <small>{g.words}</small></h4>
                <ul class="s-rules-picks">
                  <For each={g.items}>
                    {(it) => (
                      <li>
                        <button onClick={() => insertItem(it)}>
                          <b><code>{it.label}</code></b>
                          <small><code>{it.detail}</code>{it.words ? ` · ${it.words}` : ""}</small>
                        </button>
                      </li>
                    )}
                  </For>
                </ul>
              </section>
            )}
          </For>
        </div>
        <footer><button class="s-btn" onClick={() => inserter?.close()}>Cancel</button></footer>
      </dialog>
    </>
  );
}
