import { beforeAll, expect, test } from "bun:test";
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
      { index: 0, name: "Bolt", bot: true, score: 2, kills: 2, deaths: 1, shots: 8, accuracy: 0.75 },
      { index: 1, name: "Chrome", bot: true, score: 0, kills: 0, deaths: 1, shots: 0, accuracy: 0 },
    ] },
  };
  const report = render(() => component(page.RunReport, { detail }));
  expect(report).toContain("Report JSON");
  expect(report).toContain("18446744073709551615");
  expect(report).toContain("Determinism");
  const stats = render(() => component(page.BotStats, { detail }));
  expect(stats).toContain("75.0%");
  expect(stats).toContain("Chrome");
  expect(stats).not.toContain(">0.0%");
  expect(comparisonRows(left, right)).toContainEqual(["Speed", "2.80×", "1.00×"]);
  const comparison = render(() => component(page.RunComparison, { left, right }));
  expect(comparison).toContain("Compare runs");
  expect(comparison).toContain("Bot parameters");
});

test("requests a signed stream scoped to the selected game and its log", () => {
  const client = api(targetFor("http://127.0.0.1:8074"));
  const scoped = new URL(client.streamUrl(123, true, "sim-demo"));
  expect(Object.fromEntries(scoped.searchParams)).toEqual({
    from: "123", auth: "1", exclude: "GameFrame", frames: "packed",
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
