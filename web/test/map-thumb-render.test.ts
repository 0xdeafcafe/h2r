// A game's map thumbnail rendered to HTML (compiled for the server as in
// stats-home-render.test.ts): the map's own picture when the server's art
// has one, Halo 1 maps included; a card with the map's name when it has
// neither picture nor outline, never an empty grid or a broken image.

import { beforeAll, describe, expect, test } from "bun:test";
import { plugin } from "bun";
import type { GameState } from "../src/state/reducer";

const ROUTER = `${import.meta.dir}/../node_modules/@solidjs/router/dist/index.jsx`;

// Invented names; nothing here is the game's.
const MANIFEST = {
  version: 1,
  images: {
    "map/tower/preview": { file: "map/tower/preview.png", width: 4, height: 3 },
    "map/h1_gulch/preview": { file: "map/h1_gulch/preview.png", width: 4, height: 3 },
  },
  maps: { h1_gulch: { geometry: "map/h1_gulch/map.glb", kind: "multiplayer", game: "halo1", name: "Gulch of Blood" } },
};

const game = (over: Partial<GameState>): GameState => ({
  game_id: "g1", console: null, host: null, map: "tower", map_id: null, mode: "multiplayer", kind: "custom",
  playlist: null, playlist_id: null, difficulty: null, online: null, skulls: [], variant: "King of the Hill",
  team_play: false, score_limit: null, time_limit_s: null, started_at: "2026-11-18T10:00:00Z", ended_at: "2026-11-18T10:10:00Z",
  status: "ended", end_reason: "finished", winner: null, start_tick: 0, players: {}, scores: {}, teams: {}, kills: [],
  last_tick: 0, first_position: 0, positions: [], ...over,
});

let thumb: (g: GameState) => string;
let art: { set(on: boolean): void };

beforeAll(async () => {
  const babel = await import("@babel/core");
  const solid = (await import("babel-preset-solid")).default;
  plugin({
    name: "solid-ssr",
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
  const { MapThumb } = await import("../src/stats/Side");
  const { gameArt, parseManifest } = await import("../src/lib/gameart");
  const { setGameArtForTest, setMapTitles } = await import("../src/state/gameart");
  thumb = (g) => renderToString(() => createComponent(MapThumb, { game: g }));
  art = {
    set(on) {
      setMapTitles(on ? { tower: "Tall Tower" } : {});
      setGameArtForTest(on ? gameArt("https://art.example/art/abc/", parseManifest(MANIFEST)!) : null);
    },
  };
});

describe("a game's map thumbnail", () => {
  test("with the server's art: the map's own picture, named by the disc", () => {
    art.set(true);
    const html = thumb(game({}));
    expect(html).toContain('src="https://art.example/art/abc/map/tower/preview.png"');
    expect(html).toContain('alt="Tall Tower preview"');
    expect(html).not.toContain("s-thumb-none");
    art.set(false);
  });

  test("a converted Halo 1 map gets its converted picture, by its bare name too", () => {
    art.set(true);
    for (const map of ["h1_gulch", "gulch"]) {
      const html = thumb(game({ map }));
      expect(html).toContain('src="https://art.example/art/abc/map/h1_gulch/preview.png"');
      expect(html).toContain('alt="Gulch of Blood preview"');
    }
    art.set(false);
  });

  test("without art or outline, a finished game shows a card with the map's name, not an empty grid", () => {
    art.set(false);
    const html = thumb(game({}));
    expect(html).toContain("s-thumb-none");
    expect(html).toContain("<b>Tower</b>");
    expect(html).toContain("<small>King of the Hill</small>");
    expect(html).not.toContain("<img");
    expect(html).not.toContain("map-canvas");
    const campaign = thumb(game({ map: "05a_deep_water", mode: "campaign", kind: "campaign", variant: null }));
    expect(campaign).toContain("<b>Deep Water</b>");
    expect(campaign).toContain("<small>Campaign</small>");
  });

  test("a live game keeps its canvas for the blips", () => {
    art.set(false);
    const html = thumb(game({ status: "live", ended_at: null }));
    expect(html).not.toContain("s-thumb-none");
    expect(html).toContain("LIVE");
  });
});
