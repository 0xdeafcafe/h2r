// Where the stats site lives (src/routes.ts), its old address, and the
// servers' copy of the same root (crates/live/live-server/src/page_meta.rs).

import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { dashboardPlayerHref, GAMES_ROOT, OLD_STATS_ROOT, STATS_ROOT, gameHref, movedStatsPath, movedTheaterPath, statsPath, theaterHref } from "../src/routes";

const ROOT = resolve(import.meta.dir, "../..");

test("a Live account link opens the dashboard player and encodes the gamertag", () => {
  expect(dashboardPlayerHref("Sneaky Pete")).toBe("/dash/players/Sneaky%20Pete");
  expect(dashboardPlayerHref("R&D #1")).toBe("/dash/players/R%26D%20%231");
});

describe("the stats site's address", () => {
  test("it is Halo 2's, under /games", () => {
    expect(STATS_ROOT).toBe("/games/halo2");
    expect(STATS_ROOT.startsWith(`${GAMES_ROOT}/`)).toBe(true);
    expect(statsPath()).toBe("/games/halo2");
    expect(statsPath("/players/x")).toBe("/games/halo2/players/x");
  });

  test("the old /stats pages move to the same page", () => {
    expect(movedStatsPath("/stats")).toBe("/games/halo2");
    expect(movedStatsPath("/stats/")).toBe("/games/halo2");
    expect(movedStatsPath("/stats/players/Two%20Words/career")).toBe("/games/halo2/players/Two%20Words/career");
    expect(movedStatsPath("/stats/games/abc/3d")).toBe("/games/halo2/games/abc/3d");
    for (const not of ["/", "/statsx", "/games/halo2", "/dash/stats", "/v1/stats/summary"]) expect(movedStatsPath(not)).toBeNull();
  });

  test("the servers know the same addresses", () => {
    const rs = readFileSync(join("crates/live/live-server/src/page_meta.rs"), "utf8");
    expect(rs).toContain(`pub const STATS_ROOT: &str = "${STATS_ROOT}";`);
    expect(rs).toContain(`pub const OLD_STATS_ROOT: &str = "${OLD_STATS_ROOT}";`);
    // The Worker is sent the old pages (to redirect them) and the new.
    const toml = readFileSync(join("crates/live/live-worker/wrangler.toml"), "utf8");
    for (const p of [`"${OLD_STATS_ROOT}"`, `"${OLD_STATS_ROOT}/*"`, `"${GAMES_ROOT}"`, `"${GAMES_ROOT}/*"`]) expect(toml).toContain(p);
  });

  test("the app redirects the old root and /games, replacing the entry", () => {
    const src = readFileSync(join("web/src/index.tsx"), "utf8");
    expect(src).toContain("<Route path={[OLD_STATS_ROOT, `${OLD_STATS_ROOT}/*rest`]} component={OldStats} />");
    expect(src).toContain('<Route path="/theater/:id" component={OldTheater} />');
    expect(src).toContain("<Route path={GAMES_ROOT} component={() => <Navigate href={STATS_ROOT} />} />");
  });
});

describe("whole-film links", () => {
  test("the old URL and Watch in Theater lead to the Game Details viewer", () => {
    expect(movedTheaterPath("/theater/abc")).toBe(theaterHref("abc"));
    expect(movedTheaterPath("/theater/a%2Fb/", "?t=2.5&view=3d"))
      .toBe("/games/halo2/games/a%2Fb?t=2.5&view=3d#game-viewer");
  });

  test("shared clips keep their separate link and playback range", () => {
    expect(movedTheaterPath("/theater/abc/clip/xyz")).toBeNull();
    expect(theaterHref("abc", "xyz")).toBe("/theater/abc/clip/xyz");
    expect(movedTheaterPath("/theater/")).toBeNull();
  });
});

function join(p: string): string {
  return resolve(ROOT, p);
}

test("game links encode the game id and opening player", () => {
  expect(gameHref("g/1")).toBe("/games/halo2/games/g%2F1");
  expect(gameHref("g/1", { player: "Two & Co" })).toBe("/games/halo2/games/g%2F1?player=Two%20%26%20Co");
});
