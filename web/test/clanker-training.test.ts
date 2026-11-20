import { beforeAll, expect, spyOn, test } from "bun:test";
import { plugin } from "bun";
import type { JSX } from "solid-js";
import { api } from "../src/api/http";
import { targetFor } from "../src/api/server";
import { FrameBuffer } from "../src/state/frames";
import { ReplayTimeline } from "../src/state/replay";
import { ViewerPlayback } from "../src/viewer/playback";
import { comparisonRows, runsByBatch, trainingHref, type SimDetail, type SimRun } from "../src/admin/simRuns";

function run(id: string, extra: Partial<SimRun> = {}): SimRun {
  return {
    game_id: "sim-" + id, map: "lockout", mode: "multiplayer",
    started_at: "2026-11-19T10:00:00Z", players: [],
    sim: { run_id: id, batch_id: "a-batch", gametype: "slayer", seed: "18446744073709551615",
      pace: "unpaced", bots: [{ name: "Bolt", arm: "aggressive", tuning: { reach: 4 } }], deferred: false },
    status: "running", speedup: 2.8, ticks: 240,
    scoreboard: [{ index: 0, score: 2, kills: 2, deaths: 1, assists: 0 }],
    result: null, determinism: null, has_report: false, recording_limited: false,
    ...extra,
  };
}

let page: typeof import("../src/admin/ClankerTraining");
let render: (content: () => JSX.Element) => string;
let component: typeof import("solid-js").createComponent;

beforeAll(async () => {
  const router = `${import.meta.dir}/../node_modules/@solidjs/router/dist/index.jsx`;
  const babel = await import("@babel/core");
  const solid = (await import("babel-preset-solid")).default;
  plugin({
    name: "solid-ssr-training",
    setup(build) {
      build.onLoad({ filter: /(\/src\/.*\.tsx|\/@solidjs\/router\/dist\/.*\.jsx)$/ }, async ({ path }) => {
        const out = await babel.transformAsync(await Bun.file(path).text(), {
          filename: path, babelrc: false, configFile: false,
          parserOpts: { plugins: ["jsx", "typescript"] },
          presets: [[solid, { generate: "ssr", hydratable: false }]],
        });
        return { contents: out!.code!.replaceAll('"@solidjs/router"', JSON.stringify(router)), loader: "ts" };
      });
    },
  });
  const { renderToString } = await import("solid-js/web");
  const { createComponent } = await import("solid-js");
  const { Router, Route } = await import(router);
  component = createComponent;
  page = await import("../src/admin/ClankerTraining");
  render = (content) => renderToString(() => component(Router, {
    url: trainingHref(),
    get children() { return component(Route, { path: "/games/halo2/admin", component: content }); },
  }));
});

test("the empty state shows the upload command and previews the results", () => {
  const html = render(() => component(page.TrainingEmpty, {}));
  for (const expected of [
    "No sim runs have been uploaded yet.",
    "h2 sim batch docs/runtime/sim-examples/batch.toml --parallel 4 --upload",
    "match results, simulation speed and per-bot stats",
    "select two runs",
  ]) {
    expect(html).toContain(expected);
  }
});

test("reads the signed training list and detail routes using the uploader's simulation game ID", async () => {
  const uploaded = run("one", { status: "finished", has_report: true });
  const detail: SimDetail = { run: uploaded, game: uploaded, report: { format: "h2-sim-report/1", players: [] } };
  const options = { maps: ["lockout"], arms: ["aggressive"], batches: ["a-batch"], custom_tuning: false, default_tuning: false };
  const calls: { url: URL; headers: Headers }[] = [];
  const mocked = spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
    const url = new URL(String(input));
    calls.push({ url, headers: new Headers(init?.headers) });
    return Response.json(url.pathname.endsWith("/sim-one") ? detail : {
      items: [uploaded], next: "42", position: 43, options,
    });
  });
  try {
    const client = api(targetFor("http://127.0.0.1:8074"), async () => "read-token");
    const list = await client.simRuns({ map: "lockout", tuning_choice: "arm:aggressive", batch: "a-batch", before: "44", include_options: 1 });
    expect(list).toEqual({ items: [uploaded], next: "42", position: 43, options });
    expect(await client.simRun(list.items[0]!.game_id)).toEqual(detail);
    expect(calls.map(({ url }) => url.pathname)).toEqual(["/v1/admin/sim-runs", "/v1/admin/sim-runs/sim-one"]);
    expect(Object.fromEntries(calls[0]!.url.searchParams)).toEqual({
      map: "lockout", tuning_choice: "arm:aggressive", batch: "a-batch", before: "44", include_options: "1", auth: "1",
    });
    expect(calls.every(({ headers }) => headers.get("authorization") === "Bearer read-token")).toBe(true);
  } finally {
    mocked.mockRestore();
  }
});

test("groups batch matches and shows their running or finished state", () => {
  const running = run("running");
  const queued = run("queued", { sim: { ...running.sim, run_id: "queued", deferred: true }, speedup: null });
  const finished = run("ended", { status: "finished", result: { end: "tick_limit" }, determinism: { passed: true } });
  expect(runsByBatch([running, queued, finished])[0]?.runs).toHaveLength(3);
  const html = render(() => component(page.TrainingRuns, {
    runs: [running, queued, finished], selected: [running.game_id], onSelect() {},
  }));
  for (const expected of ["Running", "Replay after match", "Finished", "Determinism passed", "2.80×", "aggressive", "sim-running"]) {
    expect(html).toContain(expected);
  }
});

test("shows the report, per-bot accuracy and comparison summary", () => {
  const left = run("finished", { status: "finished", has_report: true, result: { end: "tick_limit" }, determinism: { passed: true } });
  const right = run("second", { ticks: 400, speedup: 1 });
  const detail: SimDetail = {
    run: left, game: left,
    report: { format: "h2-sim-report/1", players: [
      { index: 0, name: "Bolt", bot: true, score: 2, kills: 2, deaths: 1, shots: 8, accuracy: 0.75,
        grenades: 1, combat: { melee_presses: 3, grenade_requests: 2, switch_presses: 5, reactions: 4,
          total_reaction_ticks: 10, think_ticks: 16, total_think_ms: 4 },
        weapon_accuracy: [{ damage_code: 8, weapon: "smg", range: "close", shots: 8, hits: 3, accuracy: 0.375 }] },
      { index: 1, name: "Chrome", bot: true, score: 0, kills: 0, deaths: 1, shots: 0, accuracy: 0 },
    ] },
  };
  const report = render(() => component(page.RunReport, { detail }));
  expect(report).toContain("Report JSON");
  expect(report).toContain("18446744073709551615");
  expect(report).toContain("Determinism");
  const stats = render(() => component(page.BotStats, { detail }));
  for (const label of ["75.0%", "37.5%", "Decision range", "close", "2.5 ticks", "0.250 ms", "Melee presses"]) {
    expect(stats).toContain(label);
  }
  expect(stats).toContain("Chrome");
  expect(stats).not.toContain(">0.0%");
  expect(comparisonRows(left, right)).toContainEqual(["Speed", "2.80×", "1.00×"]);
  const comparison = render(() => component(page.RunComparison, { left, right }));
  expect(comparison).toContain("Compare runs");
  expect(comparison).toContain("Bot parameters");
});

test("running combat samples resolve the bot by player index before the first score", () => {
  const running = run("combat", { scoreboard: [], players: [{ index: 4, name: "Chrome", team: 0, bot: true }] });
  const detail: SimDetail = {
    game: running, run: running, report: null,
    live_combat: [{
      index: 4, tick: 207,
      combat: {
        goal: "hold_angle", weapon: 2147483649, weapon_role: "scatter", range: 1.25, think_ms: 0.051,
        switch_presses: 9, weapon_switches: 2, melee_presses: 3, grenade_requests: 4, grenades_used: 1,
        reactions: 0, total_reaction_ticks: 0, think_ticks: 200, total_think_ms: 20,
        last_decisions: [{ tick: 201, action: "reload_or_interact" }],
      },
      weapon_accuracy: [{ damage_code: 9, weapon: "shotgun", range: "close", shots: 4, hits: 3, accuracy: 0.75 }],
    }],
  };
  const html = render(() => component(page.BotStats, { detail })).replace(/<!--.*?-->/g, "");
  for (const expected of ["Chrome", "hold angle", "207", "scatter", "0x80000001", "1.3 world units",
    "2 / 9", "1 / 4", "201", "reload or interact", "0.051 ms", "75.0%"]) {
    expect(html).toContain(expected);
  }
  expect(html).not.toContain("Waiting for the first score");
});

test("older running details show scores without combat counters", () => {
  const running = run("legacy", { players: [{ index: 0, name: "Bolt", team: 0, bot: true }] });
  const detail: SimDetail = { game: running, run: running, report: null };
  const html = render(() => component(page.BotStats, { detail }));
  expect(html).toContain("Bolt");
  expect(html).toContain("Kills");
  expect(html).not.toContain("Completed switches");
});

test("requests a signed stream scoped to the selected game and its log", () => {
  const client = api(targetFor("http://127.0.0.1:8074"));
  const scoped = new URL(client.streamUrl(123, true, "sim-demo"));
  expect(Object.fromEntries(scoped.searchParams)).toEqual({
    from: "123", auth: "1", exclude: "GameFrame", frames: "packed", frame_version: "8",
    games: "sim-demo", stream: "game-sim-demo",
  });
  expect(new URL(client.streamUrl(123, true)).searchParams.has("games")).toBe(false);
});

test("the shared viewer follows an unpaced sim at its live edge", () => {
  const frames = new FrameBuffer();
  const playback = new ViewerPlayback(true);
  // Six game ticks in each 67 ms batch, about 3× real time.
  for (let i = 0; i <= 60; i++) {
    frames.push({ game_id: "sim-fast", tick: i * 6, players: [] }, i * 67);
    playback.updateLive(new ReplayTimeline(frames.frames, []), i * 67);
  }
  const now = 60 * 67;
  const edge = frames.renderTick(now);
  expect(playback.tickAt(now, edge)).toBeGreaterThan(340);
  playback.seek(220, now);
  expect(playback.tickAt(now, edge)).toBe(220);
  playback.goLive(now);
  expect(playback.tickAt(now, edge)).toBe(edge);
});
