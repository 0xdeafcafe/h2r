// The game page's inline viewer: full screen through the browser's API (or
// over the window without one), which view it shows, whose keys a key
// press is, and the viewer rendered in the page with its controls.

import { beforeAll, describe, expect, test } from "bun:test";
import { plugin } from "bun";
import { afterChange, canFullscreen, setFullscreen, type FullscreenDoc } from "../src/viewer/fullscreen";
import type { GameState } from "../src/state/reducer";
import { ReplayTimeline } from "../src/state/replay";
import type { ReplaySession } from "../src/viewer/session";

/** An element and a document that record what full screen asked of them. */
function fakes(opts: { api?: boolean; refuse?: boolean; enabled?: boolean } = {}) {
  const calls: string[] = [];
  const el = {
    requestFullscreen: opts.api === false ? undefined : async () => {
      calls.push("request");
      if (opts.refuse) throw new Error("no gesture");
      doc.fullscreenElement = el as unknown as Element;
    },
  } as unknown as Element & { requestFullscreen?: () => Promise<void> };
  const doc: FullscreenDoc = {
    fullscreenElement: null,
    fullscreenEnabled: opts.enabled ?? true,
    exitFullscreen: async () => { calls.push("exit"); doc.fullscreenElement = null; },
  };
  return { el, doc, calls };
}

describe("full screen", () => {
  test("the button asks the browser to put the viewer's box full screen, and to take it out", async () => {
    const { el, doc, calls } = fakes();
    expect(canFullscreen(el, doc)).toBe(true);
    const on = await setFullscreen(el, true, doc, null);
    expect(on).toBe("api");
    expect(doc.fullscreenElement).toBe(el);
    const off = await setFullscreen(el, false, doc, on);
    expect(off).toBeNull();
    expect(calls).toEqual(["request", "exit"]);
  });

  test("Escape (the browser letting go) takes the viewer out; another element's full screen isn't ours", async () => {
    const { el, doc } = fakes();
    const on = await setFullscreen(el, true, doc, null);
    expect(afterChange(el, doc, on)).toBe("api");
    doc.fullscreenElement = null; // the browser's Escape
    expect(afterChange(el, doc, on)).toBeNull();
    // Laid over the window, the browser's events don't change it.
    expect(afterChange(el, doc, "window")).toBe("window");
  });

  test("without the API (an iPhone), or when the browser refuses, the box is laid over the window", async () => {
    const none = fakes({ api: false });
    expect(canFullscreen(none.el, none.doc)).toBe(false);
    expect(await setFullscreen(none.el, true, none.doc, null)).toBe("window");
    expect(await setFullscreen(none.el, false, none.doc, "window")).toBeNull();
    expect(none.calls).toEqual([]);

    const refused = fakes({ refuse: true });
    expect(await setFullscreen(refused.el, true, refused.doc, null)).toBe("window");
    expect(refused.calls).toEqual(["request"]);

    const disabled = fakes({ enabled: false });
    expect(await setFullscreen(disabled.el, true, disabled.doc, null)).toBe("window");
    expect(disabled.calls).toEqual([]);
  });

  test("leaving doesn't exit a full screen someone else has", async () => {
    const { el, doc, calls } = fakes();
    doc.fullscreenElement = {} as Element;
    expect(await setFullscreen(el, false, doc, "api")).toBeNull();
    expect(calls).toEqual([]);
  });
});

const game = (over: Partial<GameState> = {}): GameState => ({
  game_id: "g1", console: null, host: null, map: "orchard", map_id: null, mode: "multiplayer", kind: "custom",
  playlist: null, playlist_id: null, difficulty: null, online: null, skulls: [], variant: "Slayer", team_play: false,
  score_limit: 15, time_limit_s: null, started_at: "2026-11-18T20:00:00.000Z", ended_at: "2026-11-18T20:05:00.000Z",
  status: "ended", end_reason: null, winner: null, start_tick: 0,
  players: { 0: { index: 0, name: "Sneaky Pete", xuid: null, team: 0, local: true } as unknown as GameState["players"][number] },
  scores: {}, teams: {}, kills: [], last_tick: 0, first_position: 0, positions: [],
  ...over,
});

let render: (url: string, g: GameState, extra?: Record<string, unknown>) => string;
let viewerMode: (view: string | undefined, ok: boolean) => string;
let keyIsForControl: (t: EventTarget | null) => boolean;
let renderControls: (session: ReplaySession) => string;

beforeAll(async () => {
  const ROUTER = `${import.meta.dir}/../node_modules/@solidjs/router/dist/index.jsx`;
  const babel = await import("@babel/core");
  const solid = (await import("babel-preset-solid")).default;
  plugin({
    name: "solid-ssr-viewer",
    setup(build) {
      build.onLoad({ filter: /(\/src\/.*\.tsx|\/@solidjs\/router\/dist\/.*\.jsx)$/ }, async ({ path }) => {
        const src = await Bun.file(path).text();
        const out = await babel.transformAsync(src, {
          filename: path, babelrc: false, configFile: false,
          parserOpts: { plugins: ["jsx", "typescript"] },
          presets: [[solid, { generate: "ssr", hydratable: false }]],
        });
        return { contents: out!.code!.replaceAll('"@solidjs/router"', JSON.stringify(ROUTER)), loader: "ts" };
      });
    },
  });
  const { renderToString } = await import("solid-js/web");
  const { createComponent } = await import("solid-js");
  const { Router, Route } = await import(ROUTER);
  const mod = await import("../src/viewer/GameViewer");
  const { default: Transport } = await import("../src/viewer/Transport");
  viewerMode = mod.viewerMode;
  keyIsForControl = mod.keyIsForControl;
  renderControls = (session) => renderToString(() => createComponent(Transport, { session, matchStartTick: 0 }));
  render = (url, g, extra = {}) => renderToString(() => createComponent(Router, {
    url,
    get children() {
      return createComponent(Route, {
        path: "/games/halo2/games/:id",
        component: () => createComponent(mod.default, {
          game: g, title: "Slayer on Orchard", status: "Recorded replay",
          frame: () => ({ players: [] }), roster: () => new Map(), highlight: () => null, label: "Replay of Orchard",
          controls: "TRANSPORT", children: "LEGEND",
          ...extra,
        } as unknown as Parameters<typeof mod.default>[0]),
      });
    },
  }));
});

describe("the inline viewer", () => {
  test("the Live control shows when following or behind and disappears for a replay", () => {
    const timeline = new ReplayTimeline([
      { game_id: "g1", tick: 0, players: [] },
      { game_id: "g1", tick: 30, players: [] },
    ], []);
    const noop = () => {};
    const renderAt = (live: boolean, following: boolean) => renderControls({
      timeline: () => timeline, error: () => null, tick: () => following ? 30 : 10, clipTick: t => t,
      playing: () => following, speed: () => 1, isLive: () => live, followingLive: () => following,
      goLive: noop, toggle: noop, seek: noop, setSpeed: noop, stepFrame: noop, stepMark: noop, watchKill: noop,
      frame: () => ({ players: [], markers: [], seen: null }), soughtAt: () => 0, camera: () => null,
    });
    expect(renderAt(true, true)).toMatch(/class="replay-live" aria-pressed="true"[^>]*>Live<\/button>/);
    expect(renderAt(true, false)).toMatch(/class="replay-live" aria-pressed="false"[^>]*>Live<\/button>/);
    expect(renderAt(false, false)).not.toContain("replay-live");
  });

  test("mounts in the page: its bar, the 2D map in its stage, the controls under it, and a full screen button", () => {
    const html = render("/games/halo2/games/g1", game());
    expect(html).toMatch(/<section class="gv\s*"/);
    expect(html).toContain("Game Viewer<small>Slayer on Orchard</small>");
    expect(html).toMatch(/class="gv-stage\s*"[^>]*tabindex="-1"/);
    // The 2D map shows first, with its tools.
    expect(html).toContain("Player positions");
    expect(html).toMatch(/<button[^>]*aria-pressed="true"[^>]*>2D map<\/button>/);
    expect(html).toMatch(/<button[^>]*aria-pressed="false"[^>]*>3D view<\/button>/);
    expect(html).toMatch(/class="gv-full-button"[^>]*aria-label="Full screen"/);
    // The transport under the stage, the legend under that.
    expect(html.indexOf("gv-stage")).toBeLessThan(html.indexOf("TRANSPORT"));
    expect(html.indexOf("TRANSPORT")).toBeLessThan(html.indexOf("LEGEND"));
    // No floating window, no pop-out.
    expect(html).not.toContain("v3-window");
    expect(html).not.toContain("Pop out");
  });

  test("a link to the 3D view (the old pop-out's too) opens the inline viewer on it", () => {
    const html = render("/games/halo2/games/g1?view=3d&cam=top", game());
    expect(html).toContain("gv-stage gv-stage--3d");
    expect(html).toMatch(/<button[^>]*aria-pressed="true"[^>]*>3D view<\/button>/);
    expect(html).not.toContain("Player positions");
  });

  test("the 3D view is offered only when the map's geometry is the base map's", () => {
    expect(viewerMode("3d", true)).toBe("3d");
    expect(viewerMode("3d", false)).toBe("2d");
    expect(viewerMode(undefined, true)).toBe("2d");
    expect(viewerMode("2d", true)).toBe("2d");
  });

  test("keys on its buttons, the scrubber and its menus are theirs, not the replay's", () => {
    const at = (sel: string | null) => ({ closest: (s: string) => (sel && s.split(", ").includes(sel) ? {} : null) }) as unknown as EventTarget;
    expect(keyIsForControl(at("button"))).toBe(true);
    expect(keyIsForControl(at("input"))).toBe(true);
    expect(keyIsForControl(at("select"))).toBe(true);
    expect(keyIsForControl(at(null))).toBe(false);
    expect(keyIsForControl(null)).toBe(false);
  });
});
