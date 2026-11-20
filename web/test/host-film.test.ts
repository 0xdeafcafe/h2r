import { expect, test } from "bun:test";
import { api } from "../src/api/http";
import { chunkAt } from "../src/api/chunk";
import { FilmCache } from "../src/api/film-cache";
import { decodeFilmLifecycle } from "../src/api/film-lifecycle";
import { filmGame } from "../src/theater/filmGame";
import { ReplayTimeline } from "../src/state/replay";
import { Movie } from "../src/api/film-ticks";
import { decodeFilmFront, decodeFilmIndex, openHostFilm } from "../src/api/host-film";

const text = (s: string) => new TextEncoder().encode(s);
const json = (v: unknown) => text(JSON.stringify(v));
const start = 32 + (128 << 10);
const summary = {level: "lockout", game_type: "Slayer", mode: "custom", variant_name: "Doubles", duration_ticks: 63,
  players: [{seat: 0, name: "Blue", xuid: "0x0000000000000001", team: 1, score: 4, medals: {double_kill: 1}}]};
const front = {id: "game", image_stamp: "image", runtime_stamp: "runtime", started_ms: 1, map_sha256: "a".repeat(64),
  map_origin: "retail", roster: [], match_start_tick: 130, summary};
function varint(n: number): number[] {
  const b = [];
  while (n >= 128) { b.push((n % 128) + 128); n = Math.floor(n / 128); }
  return [...b, n];
}
function framed(delta: number, tag: number, payload: ArrayLike<number>): number[] {
  return [...varint(delta), tag, ...varint(payload.length), ...Array.from(payload)];
}
// A single uncompressed block inside a standard zstd frame. This exercises
// the bundled browser WASM decoder without a second compressor in the tests.
function physical(tag: string, raw: Uint8Array) {
  const n = raw.length * 8 + 1;
  const out = new Uint8Array(20 + raw.length);
  const view = new DataView(out.buffer);
  out.set(text(tag)); view.setUint32(4, out.length - 8, true);
  out.set([0x28, 0xb5, 0x2f, 0xfd, 0xa0], 8); view.setUint32(13, raw.length, true);
  out.set([n & 255, n >>> 8 & 255, n >>> 16], 17); out.set(raw, 20);
  return out;
}
function fixture(trailingRecords: number[] = []) {
  const bodies = [
    Uint8Array.from([...varint(1), ...varint(128), ...framed(0, 3, [0, 0, 0, 0])]),
    Uint8Array.from([...varint(129), 3, ...framed(0, 3, [0, 0, 0, 0]), ...framed(1, 5, text("Lockout"))]),
    Uint8Array.from([...varint(132), 2, ...framed(0, 151, [0xff, 0x80]), ...framed(0, 1, [0xff]), ...framed(0, 4, [0xff]),
      ...framed(1, 2, [1, ...json({tick: 53, kind: "PlayerKilled", data: {tick: 53, victim: 1, killer: 0, kind: "kill"}})])]),
    Uint8Array.from([...varint(134), 2, ...framed(0, 3, [1, 0, 0]), ...trailingRecords]),
  ];
  let offset = start;
  const physicalChunks = bodies.map(b => physical("TICK", b));
  const rows = bodies.map((b, i) => {
    const first = [1, 129, 132, 134][i]!, count = [128, 3, 2, 2][i]!;
    const size = physicalChunks[i]!.length - 8;
    const row = {first, last: first + count - 1, offset, size, raw_size: b.length, scene: i ? 129 : 1};
    offset += 8 + size;
    return row;
  });
  const rawIndex = json({version: 1, chunks: rows, keyframes: []});
  const index = physical("CIDX", rawIndex);
  const indexOffset = offset, privateIndex = indexOffset + index.length;
  const size = privateIndex + 9 + 40;
  const bytes = new Uint8Array(size), v = new DataView(bytes.buffer);
  bytes.set(text("H2BLAM3!")); v.setUint32(8, 3, true); v.setUint32(12, 128 << 10, true);
  const header = json(front);
  v.setUint32(16, header.length, true); bytes.set(header, 32);
  physicalChunks.forEach((c, i) => bytes.set(c, rows[i]!.offset));
  bytes.set(index, indexOffset);
  bytes.set(text("H2RANGE3"), size - 40);
  v.setBigUint64(size - 32, BigInt(indexOffset), true); v.setBigUint64(size - 24, BigInt(privateIndex), true);
  v.setBigUint64(size - 16, BigInt(size), true); v.setUint32(size - 8, rawIndex.length, true);
  const calls: string[] = [];
  const fetch = async (range: string) => {
    calls.push(range);
    const [lo, hi] = range === "bytes=-40" ? [size - 40, size - 1] : range.substring(6).split("-").map(Number);
    return new Response(bytes.slice(lo!, hi! + 1), {status: 206, headers: {"content-range": `bytes ${lo}-${hi}/${size}`}});
  };
  return {bytes, rows, fetch, calls, header, rawIndex, indexOffset, privateIndex};
}
const signal = () => new AbortController().signal;

test("front/summary decode alone, including different players who occupied the same seat", async () => {
  const f = fixture();
  const film = await openHostFilm(f.fetch);
  expect(film?.front.summary).toEqual(summary);
  expect(f.calls).toEqual(["bytes=0-31", "bytes=32-" + (31 + f.header.length)]);
  const players = [summary.players[0], {...summary.players[0], name: "Late arrival"}];
  expect(decodeFilmFront(json({...front, summary: {...summary, players}})).summary.players).toHaveLength(2);
});

test("seek uses CIDX and only the nearest public checkpoint and deltas; forward playback resumes the dictionary", async () => {
  const f = fixture(), film = (await openHostFilm(f.fetch))!;
  const rows = await film.index();
  expect(f.calls.slice(2)).toEqual(["bytes=-40", `bytes=${f.indexOffset}-${f.privateIndex - 1}`]);
  const movie = new Movie(rows, front.match_start_tick, 50, "game", film.chunk);
  const source = chunkAt(movie.segments, 53)!;
  const {frames, events} = await movie.read(source, signal());
  const request = (i: number) => {
    const row = f.rows[i]!;
    return `bytes=${row.offset}-${row.offset + row.size + 7}`;
  };
  expect(f.calls.slice(4)).toEqual([request(1), request(2)]);
  expect(frames.map(f => f.tick)).toEqual([52, 53]);
  expect(events.map(e => [e.type, (e.data as {tick: number}).tick])).toEqual([["FilmChapter", 50], ["PlayerKilled", 53]]);
  await movie.read(chunkAt(movie.segments, 54)!, signal());
  expect(f.calls.at(-1)).toBe(request(3));
  expect(f.calls.length).toBe(7);
});

test("public joins, host migration and closing observations retain order and timing across seeks", async () => {
  const joined = {kind: "join", player: {seat: 2, name: "Orange", team: 1, kind: "remote"}};
  const migrated = {kind: "host_migration"}, quit = {kind: "quit", seat: 2};
  const f = fixture([
    ...framed(0, 2, [0, ...json([joined, migrated])]),
    ...framed(1, 2, [2, ...json([quit])]),
  ]);
  const film = (await openHostFilm(f.fetch))!, rows = await film.index();
  const movie = new Movie(rows, front.match_start_tick, 50, "game", film.chunk);
  const row = movie.segments.at(-1)!;
  const block = await movie.read(row, signal());
  const lifecycle = block.events.filter(e => e.type === "FilmLifecycle");
  expect(lifecycle.map(e => e.data)).toEqual([
    {tick: 54, closing: false, changes: [joined, migrated]},
    {tick: 55, closing: true, changes: [quit]},
  ]);
  const timeline = new ReplayTimeline(block.frames, block.events, movie.segments);
  expect(timeline.marks.filter(m => m.tick >= 54).map(m => m.label)).toEqual(["Orange joined", "Host migration", "Player 3 quit"]);
  const count = f.calls.length;
  await movie.read(movie.segments[0]!, signal());
  await movie.read(row, signal());
  expect(f.calls.length).toBe(count);
});

test("menu summary permits public player names without account IDs and preserves optional drop counts", () => {
  const players = [{seat: 1, name: "Red", team: 1, score: 3, medals: {}}];
  const projected = {...summary, players, dropped_samples: {pose: 2}};
  expect(decodeFilmFront(json({...front, summary: projected})).summary).toEqual(projected);
  expect(() => decodeFilmFront(json({...front, summary: {...projected, dropped_samples: {pose: -10}}}))).toThrow();
});

test("the per-film LRU respects both byte and count ceilings and refreshes touched entries", () => {
  const cache = new FilmCache<string>(3, 7);
  cache.set(0, "a", 2); cache.set(1, "b", 2); cache.set(2, "c", 2);
  expect(cache.get(0)).toBe("a");
  cache.set(3, "d", 1);
  expect(cache.get(1)).toBeUndefined();
  cache.set(4, "large", 5);
  expect([...cache.values()]).toEqual(["d", "large"]);
  cache.set(4, "oversized replacement", 10);
  expect([...cache.values()]).toEqual(["d"]);
});

test("Theater can open a standalone film with its lease ID and public roster", () => {
  const f = decodeFilmFront(json({...front, id: "local-file", ticks_per_second: 60,
    roster: [{seat: 0, team: 1, name: "Starter", kind: "bot"}],
    summary: {...summary, mode: "coop_campaign", players: [...summary.players,
      {seat: 3, team: 0, name: "Late", score: 1, medals: {}}]},
  }));
  const game = filmGame("lease-match", f);
  expect(game.game_id).toBe("lease-match");
  expect(game.status).toBe("ended");
  expect(game.mode).toBe("campaign");
  expect(game.players[0]).toMatchObject({name: "Starter", bot: true, xuid: null});
  expect(game.players[3]?.name).toBe("Late");
  expect(game.scores).toEqual({});
  expect(Date.parse(game.ended_at!) - Date.parse(game.started_at)).toBe(1050);
});

test("public lifecycle ignores extra private members and rejects invalid seats", () => {
  const player = {seat: 0, team: 1, kind: "bot", name: "A"};
  expect(decodeFilmLifecycle(json([{kind: "join", player: {...player, xuid: "private", bot_params: {skill: 2}}},
    {kind: "host_migration", host_id: "private"}]))).toEqual([{kind: "join", player}, {kind: "host_migration"}]);
  expect(() => decodeFilmLifecycle(json([{kind: "join", player: {...player, seat: 16}}]))).toThrow();
});

test("legacy or absent film goes through archive fallback; invalid v3 fails closed", async () => {
  for (const status of [401, 403, 404]) {
    expect(await openHostFilm(async () => new Response(null, {status}))).toBeNull();
  }
  expect(await openHostFilm(async () => new Response("no Range support"))).toBeNull();
  const f = fixture();
  f.bytes[8] = 7;
  await expect(openHostFilm(f.fetch)).rejects.toThrow("version");
  await expect(openHostFilm(async () => new Response(new Uint8Array(32), {
    status: 206, headers: {"Content-Range": "bytes 2-33/999999"},
  }))).rejects.toThrow("HTTP range");
});

test("rejects truncated HTTP bodies, overlapping offsets, an unavailable scene and bad record lengths", async () => {
  const f = fixture();
  await expect(openHostFilm(async () => new Response(null, {status: 206,
    headers: {"content-range": "bytes 0-31/1000000"}}))).rejects.toThrow();
  const altered = (i: number, changes: object) => json({
    version: 1, keyframes: [], chunks: f.rows.map((r, n) => n === i ? {...r, ...changes} : r),
  });
  expect(() => decodeFilmIndex(altered(0, {first: 2, scene: 2}), f.indexOffset)).toThrow("time index bounds");
  expect(() => decodeFilmIndex(altered(2, {first: 133}), f.indexOffset)).toThrow("time index bounds");
  expect(() => decodeFilmIndex(altered(1, {offset: start}), f.indexOffset)).toThrow();
  expect(() => decodeFilmIndex(altered(0, {scene: 15}), f.indexOffset)).toThrow();
  const film = (await openHostFilm(f.fetch))!, rows = await film.index();
  // Damage the unknown record's length in the stored zstd block.
  f.bytes[f.rows[2]!.offset + 20 + varint(132).length + 3] = 127;
  const movie = new Movie(rows, 129, 0, "game", film.chunk);
  await expect(movie.read(movie.segments[1]!, signal())).rejects.toThrow("Truncated");
});


test("saved clip points use the indexed guest clock at a revert, including before a direct seek", () => {
  const f = fixture();
  const rows = decodeFilmIndex(json({version: 1, keyframes: [], chunks: f.rows.map((row, i) => ({
    ...row, clock: [[], [[1, 50]], [[0, 105]], [[0, 105], [1, 110]]][i],
  }))}), f.indexOffset);
  const movie = new Movie(rows, 130, 50, "game", async () => { throw new Error("unexpected range"); });
  expect(movie.playTick(105)).toBe(52); // first occurrence in a persisted clip
  expect(movie.clipTick(54)).toBe(105); // second observation after the rewind
  expect(movie.playTick(110)).toBe(55);
  expect(movie.clipTick(51)).toBe(51);
});


test("the REST reader pins each range URL to the film revision from its first response", async () => {
  const f = fixture(), urls: URL[] = [], rev = "a".repeat(64);
  const original = globalThis.fetch;
  try {
    globalThis.fetch = (async (input: string, init: RequestInit) => {
      const url = new URL(input);
      // Three initialises its decoder by fetching an embedded WASM data
      // URL, including when this test runs on its own with a cold decoder.
      if (url.protocol === "data:") return original(input, init);
      urls.push(url);
      const range = new Headers(init.headers).get("Range")!;
      if (urls.length > 1) {
        expect(url.searchParams.get("rev")).toBe(rev);
        expect(url.searchParams.get("span")).toBe(range.slice(6));
      }
      const res = await f.fetch(range);
      res.headers.set("etag", `"film-${rev}"`);
      return res;
    }) as unknown as typeof fetch;
    const film = await api({base:"http://test", api:"http://test/v1"}).hostFilm("match");
    const rows = await film!.index();
    await film!.chunk(rows[0]!);
    expect(urls[0]!.search).toBe("");
    expect(new Set(urls.map(u => u.href)).size).toBe(5);
  } finally {
    globalThis.fetch = original;
  }
});

test("a successful range from a different film revision cannot enter the viewer cache", async () => {
  const f = fixture(), original = globalThis.fetch;
  try {
    globalThis.fetch = (async (_: string, init: RequestInit) => {
      const range = new Headers(init.headers).get("Range")!;
      const response = await f.fetch(range);
      const revision = (range === "bytes=0-31" ? "a" : "b").repeat(64);
      response.headers.set("etag", `"film-${revision}"`);
      return response;
    }) as unknown as typeof fetch;
    await expect(api({base: "http://test", api: "http://test/v1"}).hostFilm("g")).rejects.toThrow("revision changed");
  } finally {
    globalThis.fetch = original;
  }
});
