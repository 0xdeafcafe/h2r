import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { FrameObjects, type ObjectEntry, type ScenarioTarget } from "../src/viewer/objects3d";
import { sampleObjects } from "../src/state/frames";
import { StreamClient, type SocketLike } from "../src/state/stream";
import { PACKED_FRAMES, packedFrames } from "../src/api/http";
import { records } from "../src/api/pack";
import type { Frame } from "../src/api/types";

class FakeTarget implements ScenarioTarget {
  calls: string[] = [];
  apply(list: readonly ObjectEntry[]) {
    this.calls.push("apply " + list.map((o) => `${o.k}:${o.i}${o.gone ? "-" : ""}`).join(","));
    return true;
  }
  restAll() { this.calls.push("rest"); }
}

const frame = (tick: number, objects?: Frame["objects"]): Frame => ({ game_id: "g", tick, players: [], ...(objects ? { objects } : {}) });

describe("FrameObjects", () => {
  test("applies moves and gones, and puts back what drops off the list", () => {
    const t = new FakeTarget();
    const o = new FrameObjects(t);
    o.update([{ k: "bloc", i: 3, p: [1, 2, 3] }, { k: "netgame", i: 0, gone: true }]);
    o.update([{ k: "bloc", i: 3, p: [1, 2, 4] }, { k: "netgame", i: 0, gone: true }]);
    // The spawn's weapon came back: everything back, then the crate again.
    o.update([{ k: "bloc", i: 3, p: [1, 2, 5] }]);
    // Nothing out of place: back once, then nothing to do.
    o.update([]);
    o.update(null);
    expect(t.calls).toEqual([
      "apply bloc:3,netgame:0-",
      "apply bloc:3,netgame:0-",
      "rest",
      "apply bloc:3",
      "rest",
    ]);
  });

  test("a moved placed vehicle is hidden when the frames draw vehicles", () => {
    const t = new FakeTarget();
    const o = new FrameObjects(t);
    o.update([{ k: "vehi", i: 1, p: [0, 0, 0] }, { k: "weap", i: 2, p: [1, 1, 1] }], true);
    o.update([{ k: "vehi", i: 1, p: [0, 0, 0] }], false);
    expect(t.calls).toEqual(["apply vehi:1-,weap:2", "rest", "apply vehi:1"]);
  });

  test("a new level starts with nothing applied", () => {
    const t = new FakeTarget();
    const o = new FrameObjects(t);
    o.update([{ k: "scen", i: 0, gone: true }]);
    o.reset();
    o.update([]);
    expect(t.calls).toEqual(["apply scen:0-"]);
  });
});

describe("sampleObjects", () => {
  const frames = [
    frame(0),
    frame(10, [{ k: "bloc", i: 1, p: [0, 0, 0], f: [1, 0, 0], u: [0, 0, 1] }, { k: "weap", i: 4, gone: true }]),
    frame(20, [{ k: "bloc", i: 1, p: [10, 0, 0], f: [0, 1, 0], u: [0, 0, 1] }, { k: "scen", i: 2, p: [100, 0, 0], f: [1, 0, 0], u: [0, 0, 1] }]),
    frame(30, [{ k: "bloc", i: 1, p: [500, 0, 0], f: [1, 0, 0], u: [0, 0, 1] }]),
  ];

  test("nothing before the first frame, nothing out of place in a frame without objects", () => {
    expect(sampleObjects(frames, -1)).toBeNull();
    expect(sampleObjects(frames, 5)).toEqual([]);
  });

  test("moves ease towards the next frame; gone follows the frame behind", () => {
    const at = sampleObjects(frames, 15)!;
    expect(at.length).toBe(2);
    expect(at[0]!.p).toEqual([5, 0, 0]);
    const f = at[0]!.f!;
    expect(Math.hypot(...f)).toBeCloseTo(1, 9);
    expect(f[0]).toBeCloseTo(Math.SQRT1_2, 9);
    expect(at[1]).toEqual({ k: "weap", i: 4, gone: true });
  });

  test("a jump snaps rather than slides", () => {
    expect(sampleObjects(frames, 25)![0]!.p).toEqual([10, 0, 0]);
  });
});

const dir = join(import.meta.dir, "fixtures", "pack");
const golden = {
  bin: new Uint8Array(readFileSync(join(dir, "multiplayer.bin"))),
  json: JSON.parse(readFileSync(join(dir, "multiplayer.json"), "utf8")) as Frame[],
};

function client(frames: Frame[]) {
  let sock: SocketLike | null = null;
  const c = new StreamClient(
    {
      url: () => "ws://x",
      connect: () => (sock = { onopen: null, onmessage: null, onclose: null, onerror: null, close() {} }),
      setTimer: () => 0,
      clearTimer: () => {},
    },
    { onEvent: () => {}, onFrame: (f) => frames.push(f), onCaughtUp: () => {} },
  );
  c.start(null);
  return { c, sock: () => sock! };
}

const container = (list: Uint8Array[]) => {
  const out: number[] = [];
  for (const r of list) {
    let n = r.length;
    while (n >= 0x80) { out.push((n & 0x7f) | 0x80); n >>>= 7; }
    out.push(n);
    out.push(...r);
  }
  return new Uint8Array(out);
};

describe("packed stream", () => {
  test("binary messages decode to the encoder's frames", () => {
    const got: Frame[] = [];
    const { sock } = client(got);
    sock().onmessage!({ data: golden.bin.buffer.slice(golden.bin.byteOffset, golden.bin.byteOffset + golden.bin.byteLength) });
    expect(got).toEqual(golden.json);
  });

  test("a missed frame skips deltas until the next keyframe", () => {
    const got: Frame[] = [];
    const { sock } = client(got);
    const list = records(golden.bin);
    // Keyframes every 4: 0 and 4. Lose 1.
    for (const [n, r] of list.entries()) if (n !== 1) sock().onmessage!({ data: container([r]) });
    expect(got.map((f) => f.tick)).toEqual([golden.json[0]!.tick, golden.json[4]!.tick, golden.json[5]!.tick]);
    expect(got[1]).toEqual(golden.json[4]!);
  });

  test("garbage is dropped quietly, JSON frames still work", () => {
    const got: Frame[] = [];
    const { c, sock } = client(got);
    sock().onmessage!({ data: new Uint8Array([0xff, 0xff, 0xff]) });
    c.handle(JSON.stringify({ kind: "frame", frame: golden.json[0] }));
    expect(got).toEqual([golden.json[0]!]);
  });
});

describe("packedFrames", () => {
  test("follows x-next through packed pages", async () => {
    const list = records(golden.bin);
    const keyframes = [list[0]!, list[4]!];
    const urls: string[] = [];
    const frames = await packedFrames(
      (cursor) => `http://h/v1/games/g/frames?format=packed${cursor ? "&cursor=" + cursor : ""}`,
      async (url) => {
        urls.push(url);
        const second = url.includes("cursor=");
        return new Response(container([keyframes[second ? 1 : 0]!]), {
          headers: { "content-type": PACKED_FRAMES, ...(second ? {} : { "x-next": "abc" }) },
        });
      },
    );
    expect(urls.length).toBe(2);
    expect(frames).toEqual([golden.json[0]!, golden.json[4]!]);
  });

  test("reads JSON pages from a server without packed frames", async () => {
    const frames = await packedFrames(
      () => "http://h/frames",
      async () => new Response(JSON.stringify({ items: [golden.json[0]], next: null, position: 7 }), { headers: { "content-type": "application/json" } }),
    );
    expect(frames).toEqual([golden.json[0]!]);
  });

  test("errors carry the server's code", async () => {
    await expect(packedFrames(
      () => "http://h/frames",
      async () => new Response(JSON.stringify({ error: "not_found", message: "no game" }), { status: 404 }),
    )).rejects.toMatchObject({ status: 404, code: "not_found" });
  });
});
