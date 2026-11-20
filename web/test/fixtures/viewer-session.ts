import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { brotliDecompressSync } from "node:zlib";
import { chunkIndex } from "../../src/api/chunk";
import { createRoot, createSignal } from "solid-js";
import type { Frame } from "../../src/api/types";
import { frameHub, live } from "../../src/state/live";
import type { GameState } from "../../src/state/reducer";
import { createReplaySession } from "../../src/viewer/session";

// Run under Bun's browser condition so Solid executes the session effects.
globalThis.requestAnimationFrame = () => 1;
globalThis.cancelAnimationFrame = () => {};

const id = "7f3c0b5e2a9d4c1f8e6b3a2d1c0f9e8d";
const frame = (tick: number): Frame => ({ game_id: id, tick: 9000 + tick, players: [] });
const now = performance.now();
for (const [i, tick] of [90, 120, 150].entries()) frameHub.push(frame(tick), now - 3000 + i * 1000);

const api = live.api();
api.hostFilm = async () => null;
api.chunks = async () => null;
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
  assert.equal(session.tick(), 9150);
  session.seek(9120);
  assert.equal(session.followingLive(), false);
  setGame((g) => ({ ...g, last_tick: 9150 }));
  assert.equal(session.tick(), 9120);
  session.goLive();
  assert.equal(session.tick(), 9150);

  session.seek(9120);
  setGame((g) => ({ ...g, status: "ended" }));
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(requests, 1);
  assert.equal(session.isLive(), false);
  assert.equal(session.tick(), 9120);
  assert.equal(session.playing(), false);

  const observed = (tick: number, x: number): Frame => ({
    ...frame(tick), projectiles: [{ id: "01020003", kind: "frag_grenade", pos: [x, 0, 0] }],
  });
  deliverRecording([
    ...[0, 30, 60, 90].map(frame),
    observed(120, 1), observed(122, 5), observed(124, 3), frame(150), frame(180),
  ]);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(session.timeline()?.startTick, 9000);
  assert.equal(session.timeline()?.endTick, 9180);
  assert.equal(session.tick(), 9120);
  assert.equal(session.playing(), false);
  // Play across the bounce, then seek to the sample on the far side.
  // A seek keeps the observed head but empties all earlier trail segments.
  session.toggle();
  assert.deepEqual(session.frame(performance.now() + 220).projectileTrails?.[0]?.points, [
    [1, 0, 0], [5, 0, 0], [3, 0, 0],
  ]);
  session.toggle();
  session.seek(9124);
  assert.deepEqual(session.frame(performance.now()).projectileTrails?.[0]?.points, [[3, 0, 0]]);
  // A published archive can arrive after the raw recording opened. The
  // player remains paused while events are still in flight.
  const index = chunkIndex({game_id:id, ended:true, archive_sha256:"a".repeat(64), items:[
    {from_tick:9000,to_tick:9898,frames:450,bytes:15556,sha256:"b".repeat(64),keyframes:[0]}
  ]})!;
  session.seek(9180);
  api.chunks = async () => index;
  api.chunk = async () => brotliDecompressSync(readFileSync(new URL("./archive/mp16.br", import.meta.url)));
  api.streamEvents = () => new Promise(() => {});
  setGame(g => ({...g, archiveRevision:100}));
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(session.error(), null);
  assert.equal(session.timeline()?.endTick, 9898);
  assert.equal(session.timeline()?.frames.length, 450);
  assert.equal(session.tick(), 9180);
  assert.equal(session.playing(), false);
} finally {
  dispose();
}
