import { afterEach, describe, expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { deflateRawSync } from "node:zlib";
import { CHUNK_TYPE, chunkAt, chunkHeader, chunkIndex, chunksIn, decodeChunk, type ChunkMeta } from "../src/api/chunk";
import { api } from "../src/api/http";
import { records } from "../src/api/pack";
import type { Frame } from "../src/api/types";

const dir = join(import.meta.dir, "fixtures/pack");
const container = new Uint8Array(readFileSync(join(dir, "multiplayer.bin")));
const frames = JSON.parse(readFileSync(join(dir, "multiplayer.json"), "utf8")) as Frame[];

function leb(n: number): number[] {
  const out: number[] = [];
  for (;;) {
    const b = n & 0x7f;
    n = Math.floor(n / 128);
    if (n === 0) return [...out, b];
    out.push(b | 0x80);
  }
}

/** A chunk as crates/live-proto/src/chunk.rs writes one. */
function chunkOf(raw: Uint8Array, codec: 0 | 1, from = frames[0]!.tick, to = frames[frames.length - 1]!.tick, rawLen = raw.length): Uint8Array<ArrayBuffer> {
  const body = codec === 1 ? new Uint8Array(deflateRawSync(raw)) : raw;
  const head = [0x48, 0x32, 0x43, 0x4b, 1, codec, ...leb(rawLen), ...leb(records(raw).length), ...leb(from), ...leb(to)];
  const out = new Uint8Array(head.length + body.length);
  out.set(head);
  out.set(body, head.length);
  return out;
}

function meta(from: number, to: number): ChunkMeta {
  return { from_tick: from, to_tick: to, frames: 1, bytes: 1, sha256: "", codec: "deflate", raw_bytes: 1, versions: [4] };
}

describe("frame chunks", () => {
  test("a deflated chunk reads into the stream's frames", async () => {
    const bytes = chunkOf(container, 1);
    const h = chunkHeader(bytes);
    expect(h.codec).toBe(1);
    expect(h.rawBytes).toBe(container.length);
    expect(h.frames).toBe(frames.length);
    expect([h.fromTick, h.toTick]).toEqual([frames[0]!.tick, frames[frames.length - 1]!.tick]);
    expect(await decodeChunk(bytes)).toEqual(frames);
  });

  test("a stored chunk reads too", async () => {
    expect(await decodeChunk(chunkOf(container, 0))).toEqual(frames);
  });

  test("bad chunks are refused", async () => {
    expect(() => chunkHeader(new TextEncoder().encode("nope!!"))).toThrow("not a frame chunk");
    const format = chunkOf(container, 1);
    format[4] = 9;
    expect(() => chunkHeader(format)).toThrow("format 9");
    const codec = chunkOf(container, 1);
    codec[5] = 7;
    expect(() => chunkHeader(codec)).toThrow("codec 7");
    await expect(decodeChunk(chunkOf(container, 0, 0, 0, container.length + 1))).rejects.toThrow("does not match");
    await expect(decodeChunk(chunkOf(container, 0, 0, 0, (16 << 20) + 1))).rejects.toThrow("too large");
  });

  // Written by the Rust writer (`H2_UPDATE_GOLDEN=1 cargo test -p
  // live-proto chunk`); read here when it is there.
  const golden = join(dir, "chunk.bin");
  test.skipIf(!existsSync(golden))("the Rust writer's golden chunk reads", async () => {
    const want = JSON.parse(readFileSync(join(dir, "chunk.json"), "utf8")) as Frame[];
    expect(await decodeChunk(new Uint8Array(readFileSync(golden)))).toEqual(want);
  });

  for (const name of ["v6-chunk", "v7-chunk"]) {
    test(`the stored Rust ${name} decodes to its original frames`, async () => {
      const want = JSON.parse(readFileSync(join(dir, name + ".json"), "utf8"));
      expect(await decodeChunk(readFileSync(join(dir, name + ".bin")))).toEqual(want);
    });
  }

  test("a seek picks the chunk that holds the tick", () => {
    const index = [meta(0, 298), meta(300, 598), meta(600, 640)];
    expect(chunkAt(index, -1)).toBeNull();
    expect(chunkAt(index, 0)!.from_tick).toBe(0);
    expect(chunkAt(index, 299)!.from_tick).toBe(0);
    expect(chunkAt(index, 300)!.from_tick).toBe(300);
    expect(chunkAt(index, 9999)!.from_tick).toBe(600);
    expect(chunksIn(index, 310, 610).map((c) => c.from_tick)).toEqual([300, 600]);
    expect(chunksIn(index).length).toBe(3);
  });

  test("the index is read oldest first, and junk is not an index", () => {
    const got = chunkIndex({ game_id: "g", chunk_ticks: 300, ended: true, items: [meta(300, 400), meta(0, 10), { nope: 1 }] });
    expect(got!.items.map((c) => c.from_tick)).toEqual([0, 300]);
    expect(got!.ended).toBe(true);
    expect(chunkIndex({ error: "x" })).toBeNull();
  });
});

describe("api.frames", () => {
  const real = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = real;
  });

  function serve(routes: Record<string, () => Response>): string[] {
    const asked: string[] = [];
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      const url = new URL(String(input));
      asked.push(url.pathname + url.search);
      const route = routes[url.pathname];
      return route ? route() : new Response(JSON.stringify({ error: "not_found", message: "no" }), { status: 404 });
    }) as typeof fetch;
    return asked;
  }

  test("reads a game's chunks when it has them", async () => {
    const list = records(container);
    // Two chunks: each its own stream from a keyframe (the golden stream
    // has keyframes at 0 and 4).
    const cat = (rs: Uint8Array[]) => {
      const out: number[] = [];
      for (const r of rs) out.push(...leb(r.length), ...r);
      return new Uint8Array(out);
    };
    const first = cat(list.slice(0, 4)), second = cat(list.slice(4));
    const t = (i: number) => frames[i]!.tick;
    const asked = serve({
      "/v1/games/g/chunks": () =>
        Response.json({ game_id: "g", chunk_ticks: 300, ended: true, items: [meta(t(4), t(frames.length - 1)), meta(t(0), t(3))] }),
      [`/v1/games/g/chunks/${t(0)}`]: () => new Response(chunkOf(first, 1, t(0), t(3)), { headers: { "content-type": CHUNK_TYPE } }),
      [`/v1/games/g/chunks/${t(4)}`]: () => new Response(chunkOf(second, 1, t(4), t(frames.length - 1)), { headers: { "content-type": CHUNK_TYPE } }),
    });
    const a = api({ base: "http://h", api: "http://h/v1" });
    expect(await a.frames("g")).toEqual(frames);
    expect(asked.some((u) => u.includes("/frames"))).toBe(false);
    // A range fetches only the chunks it touches.
    asked.length = 0;
    expect(await a.frames("g", t(4), t(4))).toEqual([frames[4]!]);
    expect(asked.filter((u) => u.includes("/chunks/"))).toEqual([`/v1/games/g/chunks/${t(4)}`]);
  });

  test("an older game (or server) without chunks reads /frames", async () => {
    const asked = serve({
      "/v1/games/g/chunks": () => Response.json({ game_id: "g", chunk_ticks: 300, ended: true, items: [] }),
      "/v1/games/g/frames": () => Response.json({ items: [frames[0]], next: null, position: 1 }),
    });
    const a = api({ base: "http://h", api: "http://h/v1" });
    expect(await a.frames("g")).toEqual([frames[0]!]);
    expect(asked.some((u) => u.startsWith("/v1/games/g/frames"))).toBe(true);
    serve({ "/v1/games/g/frames": () => Response.json({ items: [frames[1]], next: null, position: 1 }) });
    expect(await a.frames("g")).toEqual([frames[1]!]);
  });
});
