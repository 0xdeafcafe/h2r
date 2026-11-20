import { A, useSearchParams } from "@solidjs/router";
import { For, Show, batch, createEffect, createMemo, createResource, createSignal, on, onCleanup, onMount } from "solid-js";
import { wsUrl } from "../api/server";
import type { PlayerKilled } from "../api/types";
import { GameView } from "../stats/Game";
import { Panel } from "../stats/Layout";
import { usDateTime } from "../stats/fmt";
import { identity } from "../state/identity";
import { mapTitle } from "../state/gameart";
import { frameHub, live } from "../state/live";
import { applyGameSnapshot, initialState } from "../state/reducer";
import { StreamClient } from "../state/stream";
import {
  TRAINING_BATCH_COMMAND, comparisonRows, determinismLabel, resultLabel, runsByBatch, scoreLabel, speedLabel, trainingHref, tuningLabel,
  type SimCombat, type SimDetail, type SimFilter, type SimPlayer, type SimRun,
} from "./simRuns";

const REFRESH_MS = 5000;

function refreshVisible(refresh: () => void) {
  onMount(() => {
    const timer = setInterval(() => { if (!document.hidden) refresh(); }, REFRESH_MS);
    onCleanup(() => clearInterval(timer));
  });
}

async function load<T>(read: () => Promise<T>) {
  try {
    return { data: await read(), error: null };
  } catch (e) {
    return { data: null, error: e instanceof Error ? e.message : String(e) };
  }
}

export default function ClankerTraining() {
  return (
    <Show when={identity.me() && `${live.server().base}|${identity.me()!.xuid}`} keyed>
      <Training />
    </Show>
  );
}

function Training() {
  const [params] = useSearchParams<{ run?: string; batch?: string }>();
  return (
    <Show when={typeof params.run === "string" && params.run} keyed
      fallback={<RunList batch={() => typeof params.batch === "string" ? params.batch : undefined} />}>
      {(id) => <RunDetail id={id} />}
    </Show>
  );
}

function RunList(props: { batch: () => string | undefined }) {
  const [filters, setFilters] = createSignal<SimFilter>({});
  const [cursors, setCursors] = createSignal<string[]>([]);
  const [selection, setSelection] = createSignal<SimRun[]>([]);
  const activeBatch = () => filters().batch ?? props.batch() ?? "";
  const [runs, { refetch }] = createResource(
    (): SimFilter => ({ ...filters(), batch: activeBatch(), before: cursors().at(-1), include_options: 1 }),
    (query) => load(() => live.api().simRuns(query)),
  );
  refreshVisible(() => { if (!runs.loading) void refetch(); });
  createEffect(on(props.batch, () => updateFilters({}), { defer: true }));

  function updateFilters(next: SimFilter) {
    batch(() => {
      setCursors([]);
      setFilters(next);
    });
  }

  const options = () => runs.latest?.data?.options;
  const hasFilters = () => !!(filters().map || filters().tuning_choice || activeBatch());
  const hasRuns = () => {
    const available = options();
    return available ? available.maps.length > 0 : !!(runs.latest?.data?.items.length || hasFilters() || cursors().length);
  };

  function pick(run: SimRun) {
    setSelection((old) => old.some((r) => r.game_id === run.game_id)
      ? old.filter((r) => r.game_id !== run.game_id)
      : old.length < 2 ? [...old, run] : old);
  }
  const compared = () => selection().map((saved) =>
    runs.latest?.data?.items.find((run) => run.game_id === saved.game_id) ?? saved);
  return (
    <>
      <Panel title="Clanker Training" right={<button class="s-btn" onClick={() => void refetch()}>Refresh</button>}>
        <Show when={hasRuns() && options()}>{(available) => (
          <div class="adm-row" role="group" aria-label="Filter training runs">
            <label class="adm-field">
              <span>Map</span>
              <select value={filters().map ?? ""} onChange={(e) => updateFilters({ ...filters(), map: e.currentTarget.value })}>
                <option value="">All maps</option>
                <For each={available().maps}>{(map) => <option value={map} selected={filters().map === map}>{mapTitle(map)}</option>}</For>
              </select>
            </label>
            <label class="adm-field">
              <span>Tuning</span>
              <select value={filters().tuning_choice ?? ""} onChange={(e) => updateFilters({ ...filters(), tuning_choice: e.currentTarget.value })}>
                <option value="">All tuning</option>
                <Show when={available().default_tuning}><option value="default">Default tuning</option></Show>
                <Show when={available().custom_tuning}><option value="custom">Custom tuning</option></Show>
                <For each={available().arms}>{(arm) => <option value={`arm:${arm}`} selected={filters().tuning_choice === `arm:${arm}`}>{arm}</option>}</For>
              </select>
            </label>
            <label class="adm-field">
              <span>Batch</span>
              <select value={activeBatch()} onChange={(e) => updateFilters({ ...filters(), batch: e.currentTarget.value })}>
                <option value="">All runs</option>
                <Show when={activeBatch() && !available().batches.includes(activeBatch())}>
                  <option value={activeBatch()} selected>{activeBatch()}</option>
                </Show>
                <For each={available().batches}>{(id) => <option value={id} selected={activeBatch() === id}>{id}</option>}</For>
              </select>
            </label>
            <button class="s-btn" disabled={!hasFilters()} onClick={() => updateFilters({ batch: "" })}>Clear filters</button>
          </div>
        )}</Show>
        <Show when={runs.latest?.error}>{(message) => <p class="s-bad" role="alert">{message()}</p>}</Show>
        <Show when={runs.latest?.data} fallback={<Show when={runs.loading}><p>Loading sim runs…</p></Show>}>
          {(page) => (
            <>
              <Show when={hasRuns()} fallback={<TrainingEmpty />}>
                <TrainingRuns runs={page().items} selected={selection().map((run) => run.game_id)} onSelect={pick} />
              </Show>
              <Show when={selection().length > 0}>
                <button class="s-btn" onClick={() => setSelection([])}>Clear comparison</button>
              </Show>
              <Show when={cursors().length > 0 || page().next}>
                <div class="adm-row">
                  <button class="s-btn" disabled={!cursors().length || runs.loading} onClick={() => setCursors((c) => c.slice(0, -1))}>Previous</button>
                  <button class="s-btn" disabled={!page().next || runs.loading} onClick={() => setCursors((c) => [...c, page().next!])}>Next</button>
                </div>
              </Show>
            </>
          )}
        </Show>
      </Panel>
      <Show when={compared().length === 2}>
        <RunComparison left={compared()[0]!} right={compared()[1]!} />
      </Show>
    </>
  );
}

export function TrainingEmpty() {
  return (
    <div class="adm-sim-empty">
      <h3>No sim runs have been uploaded yet.</h3>
      <p>From the h2recomp checkout, start a training batch using an admin account on this server.</p>
      <pre><code>{TRAINING_BATCH_COMMAND}</code></pre>
      <p>Uploaded runs show match results, simulation speed and per-bot stats. Open a run to watch it live or replay it, or select two runs to compare their tuning and results.</p>
    </div>
  );
}

function RunStatus(props: { run: SimRun }) {
  return (
    <Show when={props.run.status === "running"} fallback={props.run.status === "finished" ? "Finished" : "Interrupted"}>
      <span class="s-live-tag">Running</span>{props.run.sim.deferred ? " · Replay after match" : " · Live"}
    </Show>
  );
}

export function TrainingRuns(props: { runs: SimRun[]; selected: string[]; onSelect: (run: SimRun) => void }) {
  return (
    <Show when={props.runs.length} fallback={<p>No sim runs match these filters.</p>}>
      <div class="adm-sim-tables">
        <For each={runsByBatch(props.runs)}>
          {(group) => (
            <section class="adm-sim-batch">
              <h3><Show when={group.id} fallback="Single runs">{(id) => <A href={trainingHref(undefined, id())} title={id()}>Batch {id().slice(0, 8)}</A>}</Show></h3>
              <table class="s-stats">
                <thead><tr><th scope="col">Match</th><th scope="col">Bots</th><th scope="col">Result</th><th scope="col">Ticks</th><th scope="col">Speed</th><th scope="col">Compare</th></tr></thead>
                <tbody><For each={group.runs}>{(run) => (
                  <tr>
                    <td>
                      <A href={trainingHref(run.game_id, group.id ?? undefined)}>{mapTitle(run.map)} · {run.sim.gametype}</A>
                      <small>{usDateTime(run.started_at)}</small>
                      <RunStatus run={run} />
                    </td>
                    <td><For each={run.sim.bots}>{(bot) => (
                      <Show when={bot.tuning} fallback={<div>{bot.name ?? "Clanker"} <small>{tuningLabel(bot)}</small></div>}>
                        <details class="adm-sim-tuning">
                          <summary>{bot.name ?? "Clanker"} <small>{tuningLabel(bot)}</small></summary>
                          <pre>{JSON.stringify(bot.tuning, null, 2)}</pre>
                        </details>
                      </Show>
                    )}</For></td>
                    <td>{resultLabel(run)}<small>{scoreLabel(run)}</small>
                      <Show when={determinismLabel(run.determinism)}>{(label) => <small>{label()}</small>}</Show>
                    </td>
                    <td>{run.ticks.toLocaleString()}</td>
                    <td>{speedLabel(run.speedup)}</td>
                    <td><input
                      type="checkbox"
                      aria-label={`Compare ${run.map} run ${run.sim.run_id}`}
                      checked={props.selected.includes(run.game_id)}
                      disabled={props.selected.length >= 2 && !props.selected.includes(run.game_id)}
                      onChange={() => props.onSelect(run)}
                    /></td>
                  </tr>
                )}</For></tbody>
              </table>
            </section>
          )}
        </For>
      </div>
    </Show>
  );
}

export function RunComparison(props: { left: SimRun; right: SimRun }) {
  return (
    <Panel title="Compare runs">
      <table class="s-stats">
        <thead><tr>
          <th scope="col">Metric</th>
          <For each={[props.left, props.right]}>{(run) => <th scope="col"><A href={trainingHref(run.game_id)}>{run.sim.run_id.slice(0, 8)}</A></th>}</For>
        </tr></thead>
        <tbody>
          <For each={comparisonRows(props.left, props.right)}>{([label, a, b]) => (
            <tr><th scope="row">{label}</th><td>{a}</td><td>{b}</td></tr>
          )}</For>
          <tr>
            <th scope="row">Bot parameters</th>
            <For each={[props.left, props.right]}>{(run) => (
              <td><details class="adm-sim-tuning"><summary>Show parameters</summary><pre>{JSON.stringify(run.sim.bots, null, 2)}</pre></details></td>
            )}</For>
          </tr>
        </tbody>
      </table>
    </Panel>
  );
}

function RunDetail(props: { id: string }) {
  const [request, { refetch }] = createResource(() => props.id, (id) => load(() => live.api().simRun(id)));
  const detail = () => request.latest?.data;
  const game = createMemo(() => {
    const found = detail();
    if (!found) return;
    const state = initialState();
    applyGameSnapshot(state, found.game);
    return state.games[found.game.game_id];
  });

  refreshVisible(() => {
    const run = detail()?.run;
    if (!request.loading && (!run || run.status === "running" || (run.status === "finished" && !run.has_report))) {
      void refetch();
    }
  });
  createEffect(on(
    () => {
      const run = detail()?.run;
      return run?.status === "running" && !run.sim.deferred ? run.game_id : undefined;
    },
    (id) => {
      if (!id) return;
      const api = live.api();
      const stream = new StreamClient({
        url: (from) => wsUrl(api.streamUrl(from, true, id)),
        auth: api.readToken,
        connect: (url) => {
          const socket = new WebSocket(url);
          socket.binaryType = "arraybuffer";
          return socket as unknown as import("../state/stream").SocketLike;
        },
      }, {
        onEvent: (event) => {
          if (event.type === "GameEnded") void refetch();
          if (event.type === "PlayerKilled") {
            const kill = event.data as PlayerKilled;
            frameHub.get(id).addDeath(kill.victim, kill.tick, performance.now(), kill.pos);
          }
        },
        onFrame: (frame) => { if (frame.game_id === id) frameHub.push(frame, performance.now()); },
        onCaughtUp: () => {},
      });
      stream.start(detail()?.game.position ?? null);
      onCleanup(() => stream.stop());
    },
  ));
  onCleanup(() => frameHub.drop(props.id));

  return (
    <>
      <A href={trainingHref(undefined, detail()?.run.sim.batch_id)}>‹ Clanker Training</A>
      <Show when={request.latest?.error}>{(message) => <p class="s-bad" role="alert">{message()}</p>}</Show>
      <Show when={detail()} fallback={<Show when={request.loading}><p>Loading sim run…</p></Show>}>
        {(found) => (
          <>
            <RunReport detail={found()} />
            <Show when={!(found().run.status === "running" && found().run.sim.deferred)}
              fallback={<p class="adm-note">The replay will be available after this match finishes.</p>}>
              <Show when={game()}>{(g) => <div class="s-game"><GameView game={g()} embedded /></div>}</Show>
            </Show>
            <BotStats detail={found()} />
          </>
        )}
      </Show>
    </>
  );
}

export function RunReport(props: { detail: SimDetail }) {
  const run = () => props.detail.run;
  return (
    <Panel title={`${mapTitle(run().map)} · ${run().sim.gametype}`} right={<RunStatus run={run()} />}>
      <dl class="adm-sim-facts">
        <div><dt>Started</dt><dd>{usDateTime(run().started_at)}</dd></div>
        <div><dt>Run ID</dt><dd>{run().sim.run_id}</dd></div>
        <Show when={run().sim.batch_id}>{(id) => <div><dt>Batch</dt><dd><A href={trainingHref(undefined, id())}>{id()}</A></dd></div>}</Show>
        <div><dt>Seed</dt><dd>{run().sim.seed}</dd></div>
        <div><dt>Pace</dt><dd>{run().sim.pace}</dd></div>
        <div><dt>Sim speed</dt><dd>{run().speedup === null ? "—" : `${speedLabel(run().speedup)} real time`}</dd></div>
        <div><dt>Ticks</dt><dd>{run().ticks.toLocaleString()}</dd></div>
        <div><dt>Result</dt><dd>{resultLabel(run())}</dd></div>
        <div><dt>Score</dt><dd>{scoreLabel(run())}</dd></div>
      </dl>
      <Show when={run().recording_limited}><p>The recording reached its size limit. The replay shows the captured portion.</p></Show>
      <details class="adm-hint"><summary>Bot tuning</summary><pre>{JSON.stringify(run().sim.bots, null, 2)}</pre></details>
      <Show when={run().determinism != null}>
        <details class="adm-hint" open><summary>Determinism</summary><pre>{JSON.stringify(run().determinism, null, 2)}</pre></details>
      </Show>
      <Show when={props.detail.report} fallback={<Show when={run().status === "finished"}><p>Report pending.</p></Show>}>
        {(report) => <details class="adm-hint"><summary>Report JSON</summary><pre>{JSON.stringify(report(), null, 2)}</pre></details>}
      </Show>
    </Panel>
  );
}

type CombatRow = Pick<SimPlayer, "index" | "name" | "combat" | "weapon_accuracy" | "grenades"> & { tick?: number };

function heldWeapon(combat: SimCombat): string {
  const tag = combat.weapon == null ? "" : `0x${combat.weapon.toString(16)}`;
  return [combat.weapon_role?.replaceAll("_", " "), tag].filter(Boolean).join(" · ") || "—";
}

function measured(value: number | null | undefined, places: number, unit: string): string {
  return value == null ? "—" : `${value.toFixed(places)} ${unit}`;
}

function CombatFigures(props: { combat: SimCombat; grenades?: number }) {
  const c = () => props.combat;
  return (
    <>
      <dl class="adm-sim-facts">
        <div><dt>Held weapon</dt><dd>{heldWeapon(c())}</dd></div>
        <div><dt>Target range</dt><dd>{measured(c().range, 1, "world units")}</dd></div>
        <div><dt>Melee presses</dt><dd>{c().melee_presses}</dd></div>
        <div><dt>Completed switches / Y presses</dt><dd>{c().weapon_switches ?? "—"} / {c().switch_presses}</dd></div>
        <div><dt>Grenades used / requested</dt><dd>{props.grenades ?? c().grenades_used ?? "—"} / {c().grenade_requests}</dd></div>
        <div><dt>Mean attack reaction</dt><dd>{c().reactions ? `${(c().total_reaction_ticks / c().reactions).toFixed(1)} ticks` : "—"}</dd></div>
        <div><dt>Last think time</dt><dd>{measured(c().think_ms, 3, "ms")}</dd></div>
        <div><dt>Mean think time</dt><dd>{c().think_ticks ? `${(c().total_think_ms / c().think_ticks).toFixed(3)} ms` : "—"}</dd></div>
      </dl>
      <Show when={c().last_decisions?.length}>
        <table class="s-stats">
          <caption>Recent decisions</caption>
          <thead><tr><th scope="col">Tick</th><th scope="col">Action</th></tr></thead>
          <tbody><For each={c().last_decisions?.slice().reverse()}>{(d) => (
            <tr><td>{d.tick}</td><td>{d.action.replaceAll("_", " ")}</td></tr>
          )}</For></tbody>
        </table>
      </Show>
    </>
  );
}

export function BotStats(props: { detail: SimDetail }) {
  const name = (index: number) => props.detail.game.players.find((p) => p.index === index)?.name ?? `Clanker ${index + 1}`;
  const players = createMemo((): SimPlayer[] => props.detail.report?.players.filter((p) => p.bot) ??
    props.detail.run.scoreboard.map((score) => ({ ...score, bot: true, name: name(score.index) })));
  const combat = createMemo(() => {
    const rows = new Map<number, CombatRow>();
    if (props.detail.report) {
      for (const p of players()) {
        if (p.combat || p.weapon_accuracy?.length) rows.set(p.index, p);
      }
    } else {
      for (const sample of props.detail.live_combat ?? []) {
        rows.set(sample.index, { ...sample, name: name(sample.index) });
      }
    }
    return rows;
  });
  return (
    <Panel title="Bot stats">
      <Show when={players().length}>
        <table class="s-stats">
          <thead><tr><th scope="col">Bot</th><th scope="col">Score</th><th scope="col">Kills</th><th scope="col">Deaths</th><th scope="col">Damage reports / shot</th></tr></thead>
          <tbody><For each={players()}>{(p) => (
            <tr><th scope="row">{p.name}{p.arm ? ` · ${p.arm}` : ""}</th><td>{p.score}</td><td>{p.kills}</td><td>{p.deaths}</td>
              <td>{p.accuracy != null && p.shots !== 0 ? `${(p.accuracy * 100).toFixed(1)}%` : "—"}</td>
            </tr>
          )}</For></tbody>
        </table>
      </Show>
      <For each={Array.from(combat().keys())}>{(index) => (
        <Show when={combat().get(index)}>{(p) => (
          <details class="adm-hint">
            <summary>
              {p().name} · {p().combat?.goal?.replaceAll("_", " ") ?? "combat"}
              <Show when={(p().tick ?? p().combat?.tick) != null}> · tick {p().tick ?? p().combat?.tick}</Show>
            </summary>
            <Show when={p().combat}>{(c) => <CombatFigures combat={c()} grenades={p().grenades} />}</Show>
            <Show when={p().weapon_accuracy?.length}>
              <table class="s-stats">
                <caption>Shots credited with an enemy hit</caption>
                <thead><tr><th scope="col">Weapon</th><th scope="col">Decision range</th><th scope="col">Hits / shots</th><th scope="col">Accuracy</th></tr></thead>
                <tbody><For each={p().weapon_accuracy}>{(w) => (
                  <tr><th scope="row">{w.weapon.replaceAll("_", " ")}</th><td>{w.range}</td>
                    <td>{w.hits} / {w.shots}</td><td>{(100 * w.accuracy).toFixed(1)}%</td></tr>
                )}</For></tbody>
              </table>
            </Show>
          </details>
        )}</Show>
      )}</For>
      <Show when={!players().length && !combat().size}>
        <p>{props.detail.run.status === "running" ? "Waiting for the first score or combat sample." : "No bot stats were recorded."}</p>
      </Show>
    </Panel>
  );
}
