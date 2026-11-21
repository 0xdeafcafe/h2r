import assert from "node:assert/strict";
import { createRoot, createSignal } from "solid-js";
import { observeSide, type Side } from "../../src/dash/cross";

// The browser condition runs Solid's route effect in the Bun test process.
const visits: Side[] = [];
const { side, setPathname, dispose } = createRoot((dispose) => {
  const [pathname, setPathname] = createSignal("/games/halo2");
  const side = observeSide(pathname, (to) => { visits.push(to); });
  return { side, setPathname, dispose };
});

try {
  assert.equal(side(), "stats");
  assert.deepEqual(visits, []);
  setPathname("/games/halo2/mods");
  setPathname("/games/halo2/mods/packs/example");
  setPathname("/games/halo2/games/example");
  assert.deepEqual(visits, []);

  setPathname("/dash/games");
  assert.equal(side(), "dash");
  setPathname("/dash/settings");
  assert.deepEqual(visits, ["dash"]);

  setPathname("/games/halo2/players/example");
  assert.equal(side(), "stats");
  setPathname("/games/halo2/players/example/history");
  assert.deepEqual(visits, ["dash", "stats"]);
} finally {
  dispose();
}
