// Game type scripts: the rules workbench (docs/game/rules-engine.md,
// "Workbench"). One screen: the script in the editor with its problems
// (and their quick fixes in the editor), what a map needs, its budget and
// capabilities beside it, and under them the test: generated
// matches on the fake game summed up, beside one of them in full. The
// server checks the script as you type and plays the matches after each
// check; "Edit this match" opens one in the hand editor, and that match
// runs when it changes. A signed-in account saves the script. The pure half is ../lib/rules.ts (the editor's,
// ../lib/rulesIde.ts); the editor is RulesEditor (Monaco, loaded on first
// use); the test's parts are RulesTest; the draft lives in
// localStorage. "Make a game variant" turns a checked script into a game
// variant (../lib/variants.ts): a file to download, or published to Live
// when signed in. The tabs beside the workbench list your saved scripts
// and your game variants. `?script=<hash>` opens a saved or built-in
// script read-only (the variant page links here).

import { For, Show, createEffect, createMemo, createResource, createSignal, on, onCleanup } from "solid-js";
import { A, useSearchParams } from "@solidjs/router";
import { ApiError } from "../api/http";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { statsPath } from "../routes";
import { HOME_CRUMB } from "./modes";
import { Crumbs, Panel, Tabs, Title } from "./Layout";
import { shortHash } from "../admin/content";
import { issueToken } from "../admin/token";
import { usDate } from "./fmt";
import {
  buildScenario, carrierOf, checkRules, explainFailure, fetchBuiltins, fetchScript, fetchScripts, loadDraft, meter, meterText,
  problemCount, problemPlace, runBlocked, runFailed, runRules, saveBlocked, saveDraft, saveRules, scenarioProblems, simFailed,
  simulateRules, toolsAfter, variantBlocked, whyOff, SIM_MATCHES,
  type CheckResult, quickRows, type Gate, type Problem, type ScenarioDraft, type ScriptOption, type SimMatch, type SimResult, type SimSetup,
  type Tools, type Trouble,
} from "../lib/rules";
import {
  VARIANT_DESCRIPTION_MAX, VARIANT_NAME_MAX, coerceOption, coerceOptions, defaultOptions, deleteVariant, descriptionProblem,
  downloadBundle, fetchVariants, makeBundle, nameProblem, notFound, publishFromScript, publishVariant, variantHref,
  type OptionValue, type OptionValues, type ScriptRef, type Variant, type Visibility,
} from "../lib/variants";
import { fetchMenuCompletions, insertMenus, mapLabelItems, type InsertItem, type InsertMenu, type InsertMenuId } from "../lib/rulesIde";
import RulesEditor, { type RulesEditorApi } from "./RulesEditor";
import { BatchView, MapNeeds, MatchBar, ResultsView, ScenarioEditor, SimStrip, TroubleNote, type Ran } from "./RulesTest";
import "../styles/rules.css";

const CRUMB: [string, string] = ["Game Types", statsPath("/rules")];
type Tab = "edit" | "mine" | "types";
const TABS: Array<{ id: Tab; label: string }> = [
  { id: "edit", label: "Workbench" },
  { id: "mine", label: "My Scripts" },
  { id: "types", label: "My Game Types" },
];
/** A script shown read-only until "Use as start": a built-in, or a saved
 *  one opened from a link. */
interface Lock { name: string; kind: "built-in" | "saved" }
/** A line about something the page did, or couldn't. */
interface Notice { text: string; bad: boolean }

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
/** How long a checked script, a changed setup or a changed match waits
 *  before it is played. */
const RUN_DELAY_MS = 350;
/** How long a rate-limited check waits before it asks again. */
const LIMITED_RETRY_MS = 3000;

const storage = (): Storage | null => {
  try { return globalThis.localStorage ?? null; } catch { return null; }
};
const aborted = (e: unknown): boolean => (e as Error | null)?.name === "AbortError";

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

/** What a side panel says before there is a check to read. */
function Waiting(props: { tools: Tools; children: string }) {
  return <p class="s-pad s-rules-muted">{props.tools === "missing" ? "Needs the server's script tools." : props.children}</p>;
}

/** A check's warnings, each with its place. */
function WarningList(props: { warnings: Problem[]; onJump: (p: Problem) => void }) {
  return (
    <ul class="s-rules-errors s-rules-warnings" aria-label="Warnings">
      <For each={props.warnings}>
        {(p) => (
          <li>
            <Show when={p.line > 0} fallback={<span class="s-rules-errplace">Whole script</span>}>
              <button class="s-rules-errlink" onClick={() => props.onJump(p)} title="Go to this place in the script">{problemPlace(p)}</button>
            </Show>
            <span><span class="s-rules-warn">Warning:</span> {p.message}</span>
          </li>
        )}
      </For>
    </ul>
  );
}

function ProblemsPanel(props: { check: CheckResult | null; busy: boolean; tools: Tools; trouble: Trouble | null; onJump: (p: Problem) => void; onRetry: () => void }) {
  const errors = () => props.check?.errors ?? [];
  const warnings = () => props.check?.warnings ?? [];
  const right = () => (props.busy ? "checking..." : !props.check ? "" : errors().length ? problemCount(errors().length) : props.check.ok ? "none" : "");
  return (
    <Panel title="Problems" sub="pick one to go to its place; Ctrl (Cmd)+. there offers its fixes" right={right()} class="s-rules-problems">
      <Show when={props.trouble && !props.trouble.missing ? props.trouble : null}>{(t) => <TroubleNote trouble={t()} onRetry={props.onRetry} />}</Show>
      <Show when={props.check} fallback={<Show when={!props.trouble || props.trouble.missing}><Waiting tools={props.tools}>Checking the script...</Waiting></Show>}>
        {(c) => (
          <Show
            when={errors().length}
            fallback={
              <Show when={c().ok} fallback={<p class="s-pad s-rules-bad">The script doesn't verify, and the server gave no reason.</p>}>
                <p class="s-pad s-rules-clean"><b>No problems.</b> {c().name ?? "The script"}{c().version !== null ? ` version ${c().version}` : ""} compiles and verifies.</p>
              </Show>
            }
          >
            <ul class="s-rules-errors" aria-label="Problems">
              <For each={errors()}>
                {(p) => (
                  <li>
                    <Show when={p.line > 0} fallback={<span class="s-rules-errplace">Whole script</span>}>
                      <button class="s-rules-errlink" onClick={() => props.onJump(p)} title="Go to this place in the script">{problemPlace(p)}</button>
                    </Show>
                    <span>{p.message}</span>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        )}
      </Show>
      <Show when={warnings().length}><WarningList warnings={warnings()} onJump={props.onJump} /></Show>
    </Panel>
  );
}

/** "What a map needs": the check's labels in Forge's words. */
function MapNeedsPanel(props: { check: CheckResult | null; tools: Tools }) {
  return (
    <Panel title="What a map needs" sub="the labels the script declares, checked before a game starts">
      <Show when={props.check?.report} fallback={<Waiting tools={props.tools}>{props.check ? "A script with problems has none to show." : "Shows once the script is checked."}</Waiting>}>
        {(r) => (
          <Show when={r().labels.length} fallback={<p class="s-pad s-rules-muted">The script declares no labels: any map will do.</p>}>
            <MapNeeds labels={r().labels} />
          </Show>
        )}
      </Show>
    </Panel>
  );
}

function BudgetPanel(props: { check: CheckResult | null; tools: Tools }) {
  return (
    <Panel title="Budget" sub="worst tick, compiled size and variable slots">
      <Show when={props.check?.report} fallback={<Waiting tools={props.tools}>{props.check ? "A script with problems has no budget to show." : "Shows once the script is checked."}</Waiting>}>
        {(r) => (
          <div class="s-rules-meters">
            <Meter label="Worst tick" used={r().worst_tick_ops} limit={r().budget} unit="ops" />
            <Meter label="Size" used={r().ir_bytes} limit={r().ir_limit} unit="bytes" />
            <For each={r().slots}>{(s) => <Meter label={`${s.scope} vars`} used={s.used} limit={s.limit} unit="slots" />}</For>
            <Show when={r().handlers.length}>
              <table class="s-rules-handlers" aria-label="Worst case of each handler">
                <tbody>
                  <For each={r().handlers}>{(h) => <tr classList={{ top: h.event === r().costliest }}><td><code>on {h.event}</code>{h.event === r().costliest ? " (costliest)" : ""}</td><td>{h.worst_ops.toLocaleString("en-US")} ops</td></tr>}</For>
                </tbody>
              </table>
            </Show>
          </div>
        )}
      </Show>
    </Panel>
  );
}

/** What a variant of the script starts from: who scores, the `settings`
 *  block in words, and the Quick Options marks. */
function SettingsPanel(props: { check: CheckResult | null; tools: Tools }) {
  return (
    <Panel title="Settings" sub="what a new variant of this script starts from">
      <Show when={props.check?.ok ? props.check : null} fallback={<Waiting tools={props.tools}>{props.check ? "A script with problems has none to show." : "Shows once the script is checked."}</Waiting>}>
        {(c) => (
          <>
            <Show when={c().about}><p class="s-pad s-rules-note">{c().about}</p></Show>
            <Show when={c().scoring}>
              {(s) => <p class="s-pad s-rules-muted">{s() === "carrier" ? `${c().carrier?.label ?? "The base game type"} scores the game; the script holds settings.` : "The script scores the game."}</p>}
            </Show>
            <Show when={c().settings.length} fallback={<p class="s-pad s-rules-muted">The script has no settings block: a variant starts from the base game type's stock settings.</p>}>
              <table class="s-rules-handlers" aria-label="The script's settings">
                <tbody>
                  <For each={c().settings}>{(s) => <tr><td>{s.label}</td><td>{s.value}</td></tr>}</For>
                </tbody>
              </table>
            </Show>
            <Show when={c().quick.length}>
              <h4 class="s-pad">Quick Options</h4>
              <table class="s-rules-handlers" aria-label="The script's Quick Options">
                <tbody>
                  <For each={quickRows(c())}>{(q) => <tr><td>{q.label}</td><td>{q.value ?? "chosen per variant"}</td></tr>}</For>
                </tbody>
              </table>
            </Show>
          </>
        )}
      </Show>
    </Panel>
  );
}

function CapabilityPanel(props: { check: CheckResult | null; tools: Tools }) {
  return (
    <Panel title="Capabilities" sub="what the script may do, as the lobby says it">
      <Show when={props.check?.report} fallback={<Waiting tools={props.tools}>{props.check ? "A script with problems has none to show." : "Shows once the script is checked."}</Waiting>}>
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
              <p class="s-pad s-rules-warn">
                <strong>Plays here, not in the game yet.</strong> It calls{" "}
                <For each={r().unhostable}>{(u, i) => <>{i() > 0 ? (i() === r().unhostable.length - 1 ? " and " : ", ") : ""}<code>{u}</code></>}</For>
                , which the game doesn't do yet. Until it does, a game with this script plays its carrier's own rules.
              </p>
            </Show>
          </>
        )}
      </Show>
    </Panel>
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
  const tab = (): Tab => (sp.tab === "mine" || sp.tab === "types" ? sp.tab : "edit");
  const first = loadDraft(storage(), STARTER);
  const [source, setSource] = createSignal(first.source);
  /** The match edited by hand, and whether it is the one in use. */
  const [scenario, setScenario] = createSignal<ScenarioDraft>(first.scenario);
  const [custom, setCustom] = createSignal(first.custom);
  /** What the generated matches are played with. */
  const [setup, setSetup] = createSignal<SimSetup>(first.setup);
  /** The last batch, and a match of it switched to. */
  const [sim, setSim] = createSignal<{ result: SimResult; key: string } | null>(null);
  const [picked, setPicked] = createSignal<{ match: SimMatch; key: string } | null>(null);
  const [simming, setSimming] = createSignal(false);
  const [simTrouble, setSimTrouble] = createSignal<Trouble | null>(null);
  const [simErrors, setSimErrors] = createSignal<Problem[]>([]);
  const [check, setCheck] = createSignal<CheckResult | null>(null);
  const [checking, setChecking] = createSignal(false);
  const [checkTrouble, setCheckTrouble] = createSignal<Trouble | null>(null);
  /** Whether the server's script tools answer, as the checks find. */
  const [tools, setTools] = createSignal<Tools>("unknown");
  const [ran, setRan] = createSignal<Ran | null>(null);
  const [running, setRunning] = createSignal(false);
  const [runTrouble, setRunTrouble] = createSignal<Trouble | null>(null);
  const [runErrors, setRunErrors] = createSignal<Problem[]>([]);
  /** What the match's map lacked when the run refused it, with the run's
   *  key (shown only while it is the match's). */
  const [runMap, setRunMap] = createSignal<{ key: string; texts: string[] } | null>(null);
  const [notice, setNotice] = createSignal<Notice | null>(null);
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

  const say = (text: string) => setNotice({ text, bad: false });
  /** "Couldn't save. " and why, with whose fault it is. */
  const sayFailed = (what: string, e: unknown, route = false) => setNotice({ text: `${what} ${explainFailure(e, route).text}`, bad: true });

  createEffect(on([source, scenario, setup, custom], ([s, d, su, c]) => saveDraft(storage(), { source: s, scenario: d, setup: su, custom: c })));

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
      if (inflight !== ctl) return;
      setCheck(c);
      setCheckTrouble(null);
      setTools("ok");
    } catch (e) {
      if (inflight !== ctl || aborted(e)) return;
      const t = explainFailure(e);
      if (t.fault === "script") {
        // The server answered and refused this script (too long, say):
        // that is a problem of the whole script, listed with the others.
        setCheck({ ok: false, hash: null, name: null, version: null, errors: [{ line: 0, col: 0, message: t.text }], warnings: [], report: null, options: [], carrier: null, scoring: null, about: "", settings: [], quick: [] });
        setCheckTrouble(null);
        setTools("ok");
        return;
      }
      setCheckTrouble(t);
      const state = toolsAfter(t);
      if (state) setTools(state);
      // The last answer is of an older script: don't show it as this one's.
      setCheck(null);
      if (e instanceof ApiError && e.status === 429) {
        clearTimeout(timer);
        timer = setTimeout(() => void runCheck(), LIMITED_RETRY_MS);
      }
    } finally {
      if (inflight === ctl) setChecking(false);
    }
  }
  const recheck = () => { clearTimeout(timer); void runCheck(); };
  createEffect(on([source, () => live.server().api], () => {
    clearTimeout(timer);
    timer = setTimeout(() => void runCheck(), CHECK_DELAY_MS);
  }));

  // An error's place: back to the workbench (the editor is only hidden on
  // the other tabs), then select its marker once the box is shown.
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
    setRan(null);
    setRunErrors([]);
    setNotice(null);
    picker?.close();
    if (tab() !== "edit") setSp({ tab: undefined });
    return true;
  }

  // The Insert menus: the completion route's events, API and flag labels
  // over the page's own lists, and a declaration for each label of the
  // test map the script does not declare.
  const [routeMenus] = createResource(() => live.server().api, (api) => fetchMenuCompletions(api));
  // A function, not a memo: it reads the shown match, declared below.
  const menus = (): InsertMenu[] => {
    const r = routeMenus.latest;
    const map = (custom() ? scenario() : shownMatch()?.scenario)?.objects ?? [];
    const declared = check()?.report?.labels.map((l) => l.text) ?? [];
    return insertMenus(r?.top ?? null, r?.body ?? null, r?.label ?? null, mapLabelItems(map, declared));
  };
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
      sayFailed("Couldn't open the script.", e);
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
      else setNotice({ text: `No script on this server has the hash ${shortHash(hash)}. The link may be to another server's script.`, bad: true });
    } catch (e) {
      sayFailed("Couldn't open the linked script.", e);
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

  // ---------- why a button is off ----------

  const gate = (): Gate => ({ tools: tools(), check: check(), locked: !!locked() });
  const saveWhy = () => saveBlocked(gate());
  const variantWhy = () => variantBlocked(gate());
  const issues = createMemo(() => scenarioProblems(scenario()));
  const runWhy = () => runBlocked(gate(), issues());
  /** The reasons under the toolbar. A server without the tools says so in
   *  the notice above them instead. */
  const off = () => {
    if (tools() === "missing") return [];
    const buttons: Array<[string, string | null]> = [["Make a game variant", variantWhy()]];
    if (identity.me()) buttons.unshift(["Save", saveWhy()]);
    return whyOff(buttons);
  };

  // ---------- generated matches ----------

  const simWhy = () => runBlocked(gate(), []);
  // The script (by its hash) and the setup a batch answers; null while the
  // script can't run. A batch is asked when this changes, after a pause.
  const simKey = createMemo((): string | null => {
    const h = check()?.hash;
    return !simWhy() && h ? `${h}\n${JSON.stringify(setup())}` : null;
  });
  let simTimer: ReturnType<typeof setTimeout> | undefined;
  let simFlight: AbortController | undefined;
  /** Plays the batch, or with a seed that one match of it. */
  async function simulate(seed?: number) {
    const key = simKey();
    if (!key) return;
    simFlight?.abort();
    const ctl = new AbortController();
    simFlight = ctl;
    const su = setup();
    const one = seed !== undefined;
    setSimming(true);
    try {
      const r = await simulateRules(live.server().api, source(), su, one ? 1 : SIM_MATCHES, seed ?? su.seed, fetch, ctl.signal);
      if (simFlight !== ctl) return;
      setSimTrouble(null);
      if (simFailed(r)) { setSimErrors(r.errors); return; }
      setSimErrors([]);
      if (one) setPicked({ match: r.match, key });
      else { setSim({ result: r, key }); setPicked(null); }
    } catch (e) {
      if (simFlight !== ctl || aborted(e)) return;
      setSimTrouble(explainFailure(e));
    } finally {
      if (simFlight === ctl) setSimming(false);
    }
  }
  createEffect(on(simKey, (key) => {
    clearTimeout(simTimer);
    if (key && key !== sim()?.key) simTimer = setTimeout(() => void simulate(), RUN_DELAY_MS);
  }));
  const simStale = () => !!sim() && sim()?.key !== simKey();
  /** The generated match shown: the one switched to, else the batch's first. */
  const shownMatch = (): SimMatch | null => {
    const s = sim(), p = picked();
    if (!s) return null;
    return p && p.key === s.key ? p.match : s.result.match;
  };
  const generated = createMemo((): Ran | null => {
    const m = shownMatch(), s = sim();
    return m && s ? { trace: m.trace, scenario: m.scenario, key: s.key } : null;
  });
  /** Switches to another match of the batch; the first is already here. */
  const showSeed = (seed: number) => {
    const s = sim();
    if (s && s.key === simKey() && seed === s.result.match.seed) setPicked(null);
    else void simulate(seed);
  };
  /** Opens the shown match in the hand editor. */
  const editMatch = () => {
    const m = shownMatch();
    if (m) setScenario(m.scenario);
    setCustom(true);
  };

  // ---------- running the match edited by hand ----------

  // The script (by its hash) and the match a run answers; null while
  // either can't run or the generated matches are in use. A run is asked
  // when this changes, after a pause.
  const runKey = createMemo((): string | null => {
    const h = check()?.hash;
    return custom() && !runWhy() && h ? `${h}\n${JSON.stringify(buildScenario(scenario()))}` : null;
  });
  let runTimer: ReturnType<typeof setTimeout> | undefined;
  let runFlight: AbortController | undefined;
  async function run() {
    const key = runKey();
    if (!key) return;
    runFlight?.abort();
    const ctl = new AbortController();
    runFlight = ctl;
    const d = scenario();
    setRunning(true);
    try {
      const t = await runRules(live.server().api, source(), d, fetch, ctl.signal);
      if (runFlight !== ctl) return;
      setRunTrouble(null);
      if (runFailed(t)) { setRunErrors(t.errors); setRunMap(t.map?.length ? { key, texts: t.map } : null); return; }
      setRunErrors([]);
      setRunMap(null);
      setRan({ trace: t, scenario: d, key });
    } catch (e) {
      if (runFlight !== ctl || aborted(e)) return;
      setRunTrouble(explainFailure(e));
    } finally {
      if (runFlight === ctl) setRunning(false);
    }
  }
  createEffect(on(runKey, (key) => {
    clearTimeout(runTimer);
    if (key && key !== ran()?.key) runTimer = setTimeout(() => void run(), RUN_DELAY_MS);
  }));
  const stale = () => !!ran() && ran()?.key !== runKey();
  /** What the run found the match's map lacks, while it is of this match. */
  const mapProblems = (): string[] => { const m = runMap(); return m && m.key === runKey() ? m.texts : []; };
  onCleanup(() => { clearTimeout(timer); clearTimeout(runTimer); clearTimeout(simTimer); inflight?.abort(); runFlight?.abort(); simFlight?.abort(); });

  async function save() {
    const me = identity.me(), wk = live.wellKnown();
    if (!me || !wk || saveWhy()) return;
    setSaving(true);
    setNotice(null);
    try {
      const r = await saveRules(live.server().api, wk.id, me, source());
      markOnServer(r.script?.hash);
      say(r.saved ? `Saved as ${shortHash(r.script?.hash ?? "")}. It is under My Scripts.` : `This exact script is already saved as ${shortHash(r.script?.hash ?? "")}${r.script?.gamertag ? ` by ${r.script.gamertag}` : ""}.`);
      void refetchMine();
    } catch (e) {
      sayFailed("Couldn't save.", e, true);
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
  const variantOk = () => !variantWhy() && !nameErr() && !descErr();

  function openMaker() {
    const c = check();
    if (!c?.ok || variantWhy()) return;
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
      setVMsg(`Couldn't make the file. ${explainFailure(e, false).text}`);
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
      setVMsg(`Couldn't publish it. ${explainFailure(e, false).text}`);
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
      sayFailed("Couldn't delete it.", e);
    } finally {
      setDeleting(null);
    }
  }

  const errors = () => check()?.errors ?? [];

  return (
    <>
      <Crumbs trail={[HOME_CRUMB, [CRUMB[0]]]} />
      <div class="s-full s-rules">
        <Title class="s-title--cap" help="Write a game type script, see its problems as you type, run it on a fake game and save it to Live. The server compiles and verifies every check; nothing runs in a real match from here.">Game Type Scripts</Title>
        <Tabs label="Game types" active={tab()} tabs={TABS.map((t) => ({ id: t.id, label: t.label }))} onPick={(id) => setSp({ tab: id === "edit" ? undefined : id })} />
        <div class="s-matte s-rules-wrap">
          <Show when={notice()}>
            {(n) => (
              <p class="s-rules-notice" classList={{ bad: n().bad }} role="status">
                <span>{n().text}</span>
                <button class="s-btn" onClick={() => setNotice(null)}>Dismiss</button>
              </p>
            )}
          </Show>

          <div class="s-rules-bench" hidden={tab() !== "edit"}>
            <div class="s-toolbar s-rules-bar">
              <button class="s-btn" onClick={() => picker?.showModal()} title="A blank game type, a built-in or one of your saved scripts">Start from...</button>
              <Show when={identity.me()} fallback={<A href="/dash/signup">Sign in to save</A>}>
                <button class="s-btn" onClick={() => void save()} disabled={saving() || !!saveWhy()} title={saveWhy() ?? "Save this script to Live"} aria-describedby={saveWhy() ? "s-rules-why" : undefined}>{saving() ? "Saving..." : "Save"}</button>
              </Show>
              <button class="s-btn" onClick={() => openMaker()} disabled={!!variantWhy()} title={variantWhy() ?? "Name it, set its options, then download or publish it"} aria-describedby={variantWhy() ? "s-rules-why" : undefined}>Make a game variant</button>
              <span class="s-rules-status" role="status">
                <Show when={tools() !== "missing"} fallback={<span class="s-rules-warn">Not checked</span>}>
                  <Show when={check()} fallback={checking() ? "Checking..." : checkTrouble() ? <span class="s-rules-warn">Not checked</span> : ""}>
                    {(c) => (
                      <Show when={c().ok} fallback={<span class="s-rules-bad">{c().errors.length ? problemCount(c().errors.length) : "Doesn't verify"}</span>}>
                        <span class="s-rules-good">Verifies</span> · <b>{c().name}</b> · <code title={`The script's hash: ${c().hash ?? ""}`}>{shortHash(c().hash ?? "")}</code>
                        <Show when={savedHash()}> · saved</Show>
                      </Show>
                    )}
                  </Show>
                </Show>
              </span>
            </div>
            <Show when={off().length}>
              <p class="s-rules-why" id="s-rules-why">
                <For each={off()}>{(o) => <span><b>{o.what}</b> {o.reason}</span>}</For>
              </p>
            </Show>
            <Show when={tools() === "missing" ? checkTrouble() : null}>
              {(t) => (
                <div class="s-rules-missing" role="status">
                  <b>This Live server doesn't have the script tools yet.</b>
                  <p>Nothing is wrong with your script. You can keep writing and your draft stays in this browser, but checking, testing, saving and making game variants need a newer server. The editor suggests plain words until then.</p>
                  <button class="s-btn" onClick={recheck} disabled={checking()} title={t().text}>{checking() ? "Asking..." : "Ask the server again"}</button>
                </div>
              )}
            </Show>

            <div class="s-rules-top">
              <div class="s-rules-main">
                <Show when={locked()}>
                  {(l) => (
                    <div class="s-rules-locked">
                      <span>{l().kind === "built-in" ? "The built-in" : "The script"} <b>{l().name}</b> is read-only. To change it, make it your own.</span>
                      <button class="s-btn" onClick={() => { setLocked(null); editor?.focus(); }}>Use as start</button>
                    </div>
                  )}
                </Show>
                <div class="s-toolbar s-rules-insert" role="toolbar" aria-label="Insert">
                  <span class="s-rules-insert-label">Insert</span>
                  <For each={menus()}>
                    {(m) => <button class="s-btn" disabled={!!locked()} title={locked() ? "The script is read-only. Choose Use as start first." : `Insert from ${m.label}`} onClick={() => openMenu(m.id)}>{m.label}...</button>}
                  </For>
                </div>
                <RulesEditor
                  value={source()}
                  onChange={(v) => setSource(v)}
                  readOnly={!!locked()}
                  problems={errors()}
                  warnings={check()?.warnings ?? []}
                  api={live.server().api}
                  onReady={(api) => { editor = api; }}
                />
                <p class="s-rules-keys">Ctrl (Cmd)+. fixes a problem · F2 renames · Shift+Alt+F formats · F12 goes to a definition · Shift+F12 finds references · Ctrl (Cmd)+Shift+O outlines</p>
              </div>
              <aside class="s-rules-side" aria-label="The script's check">
                <ProblemsPanel check={check()} busy={checking()} tools={tools()} trouble={checkTrouble()} onJump={jumpTo} onRetry={recheck} />
                <MapNeedsPanel check={check()} tools={tools()} />
                <BudgetPanel check={check()} tools={tools()} />
                <SettingsPanel check={check()} tools={tools()} />
                <CapabilityPanel check={check()} tools={tools()} />
              </aside>
            </div>

            <div class="s-rules-test">
              <Panel title="Test" sub="generated matches on a fake game" right={simming() ? "playing..." : simStale() ? "out of date" : ""} class="s-rules-testbox">
                <div class="s-rules-scenario">
                  <SimStrip setup={setup()} set={(f) => setSetup(f)} />
                  <BatchView
                    sim={sim()?.result ?? null}
                    setup={setup()}
                    busy={simming()}
                    blocked={simWhy()}
                    stale={simStale()}
                    trouble={simTrouble()}
                    errors={simErrors()}
                    shown={custom() ? null : shownMatch()?.seed ?? null}
                    onSeed={(seed) => { setCustom(false); showSeed(seed); }}
                    onRetry={() => void simulate()}
                    onJump={jumpTo}
                  />
                  <details class="s-rules-hand" open={custom()} onToggle={(e) => { const open = e.currentTarget.open; if (open !== custom()) { if (open) editMatch(); else setCustom(false); } }}>
                    <summary>Edit this match <small>for an exact case, event by event</small></summary>
                    <Show when={custom()}>
                      <ScenarioEditor
                        scenario={scenario()} set={(f) => setScenario(f)} problems={issues()}
                        labels={check()?.report?.labels ?? []} minimal={check()?.report?.minimal ?? []} mapProblems={mapProblems()}
                      />
                    </Show>
                  </details>
                </div>
              </Panel>
              <Panel
                title="Match"
                sub={custom() ? "your edited match: who scored, when and why" : "one of the matches: who scored, when and why"}
                right={custom() ? (running() ? "running..." : stale() ? "out of date" : "") : simming() ? "playing..." : simStale() ? "out of date" : ""}
                class="s-rules-testbox"
              >
                <MatchBar
                  seed={shownMatch()?.seed ?? null}
                  rows={sim()?.result.rows ?? []}
                  custom={custom()}
                  onSeed={showSeed}
                  onEdit={editMatch}
                  onBack={() => setCustom(false)}
                />
                <Show
                  when={custom()}
                  fallback={
                    <ResultsView
                      ran={generated()} running={simming()} blocked={simWhy()} stale={simStale()} trouble={null} errors={[]}
                      again="Play again" hint="Plays by itself when the script or the setup changes."
                      onRun={() => void simulate(picked() ? shownMatch()?.seed : undefined)} onJump={jumpTo}
                    />
                  }
                >
                  <ResultsView ran={ran()} running={running()} blocked={runWhy()} stale={stale()} trouble={runTrouble()} errors={runErrors()} map={mapProblems()} onRun={() => void run()} onJump={jumpTo} />
                </Show>
              </Panel>
            </div>
          </div>

          <Show when={tab() === "mine"}>
            <Show when={identity.me()} fallback={<p class="s-pad">Sign in to save scripts and see yours here. <A href="/dash/signup">Sign up</A></p>}>
              <Show when={!mine.loading} fallback={<p class="s-pad s-rules-muted">Loading...</p>}>
                <Show when={(mine() ?? []).length} fallback={<p class="s-pad s-rules-muted">No saved scripts yet. Save one from the Workbench tab.</p>}>
                  <table class="s-grid s-rules-list">
                    <thead><tr><th>Name</th><th>Version</th><th>Hash</th><th>Saved</th><th /></tr></thead>
                    <tbody>
                      <For each={mine() ?? []}>
                        {(s) => (
                          <tr>
                            <td>{s.name}</td>
                            <td>{s.version}</td>
                            <td><code title={s.hash}>{shortHash(s.hash)}</code></td>
                            <td>{s.saved_at.slice(0, 10)}</td>
                            <td class="s-rules-end"><button class="s-btn" onClick={() => void openSaved(s.hash)}>Open</button></td>
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
                <Show when={!myTypes.error} fallback={<p class="s-pad s-rules-bad">Couldn't load your game variants. {explainFailure(myTypes.error, false).text}</p>}>
                  <Show when={(myTypes() ?? []).length} fallback={<p class="s-pad s-rules-muted">No game variants yet. On the Workbench tab, write a script with no problems, then choose Make a game variant.</p>}>
                    <table class="s-grid s-rules-list">
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
                              <td class="s-rules-end"><button class="s-btn" onClick={() => void removeType(v)} disabled={deleting() === v.id}>{deleting() === v.id ? "Deleting..." : "Delete"}</button></td>
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
            <textarea rows={3} maxLength={VARIANT_DESCRIPTION_MAX} placeholder={check()?.about ? `Left empty, the script's: ${check()?.about}` : undefined} value={vDesc()} onInput={(e) => setVDesc(e.currentTarget.value)} aria-invalid={!!descErr()} />
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
          <Show when={variantWhy()}>{(m) => <p class="s-rules-warn s-rules-hint">Download and Publish are off. {m()}</p>}</Show>
          <Show when={identity.me() && !live.wellKnown()}><p class="s-rules-muted s-rules-hint">Publish is off until this server's details have loaded.</p></Show>
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
              {(b) => <li><button onClick={() => replace(b.source, { name: b.name, kind: "built-in" })}><b>{b.name}</b><small>{b.example ? "example" : "built-in"} · engine {b.engine} · opens read-only</small></button></li>}
            </For>
            <Show when={identity.me()}>
              <For each={mine() ?? []}>
                {(s) => <li><button onClick={() => void openSaved(s.hash)}><b>{s.name}</b><small>yours · {shortHash(s.hash)}</small></button></li>}
              </For>
            </Show>
          </ul>
          <Show when={tools() === "missing"}><p class="s-rules-muted s-rules-none">This server lists no built-ins: it doesn't have the script tools yet.</p></Show>
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
