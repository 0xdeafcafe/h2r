import assert from "node:assert/strict";
import { createRoot, createSignal } from "solid-js";
import type { Frame } from "../../src/api/types";
import { frameHub, live } from "../../src/state/live";
import type { GameState } from "../../src/state/reducer";
import { createReplaySession } from "../../src/viewer/session";

// Run under Bun's browser condition so Solid executes the session effects.
globalThis.requestAnimationFrame = () => 1;
globalThis.cancelAnimationFrame = () => {};

const id = "live-to-replay";
const frame = (tick: number): Frame => ({ game_id: id, tick, players: [] });
const now = performance.now();
for (const [i, tick] of [90, 120, 150].entries()) frameHub.push(frame(tick), now - 3000 + i * 1000);

const api = live.api();
let requests = 0;
let deliverRecording!: (frames: Frame[]) => void;
api.frames = () => {
  requests++;
  return new Promise((resolve) => { deliverRecording = resolve; });
};
api.streamEvents = async () => [];

const { session, setGame, dispose } = createRoot((dispose) => {
  const [game, setGame] = createSignal({ game_id: id, status: "live", kills: [] } as unknown as GameState);
  const session = createReplaySession(game, () => new Map(), () => undefined);
  return { session, setGame, dispose };
});

try {
  assert.equal(requests, 0);
  assert.equal(session.followingLive(), true);
  assert.equal(session.tick(), 150);
  session.seek(120);
  assert.equal(session.followingLive(), false);
  setGame((g) => ({ ...g, last_tick: 150 }));
  assert.equal(session.tick(), 120);
  session.goLive();
  assert.equal(session.tick(), 150);

  session.seek(120);
  setGame((g) => ({ ...g, status: "ended" }));
  assert.equal(requests, 1);
  assert.equal(session.isLive(), false);
  assert.equal(session.tick(), 120);
  assert.equal(session.playing(), false);

  deliverRecording([0, 30, 60, 90, 120, 150, 180].map(frame));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(session.timeline()?.startTick, 0);
  assert.equal(session.timeline()?.endTick, 180);
  assert.equal(session.tick(), 120);
  assert.equal(session.playing(), false);
} finally {
  dispose();
}
