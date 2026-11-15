import { describe, expect, test } from "bun:test";
import { backoffDelay, StreamClient, type SocketLike, type StreamStatus } from "../src/state/stream";
import type { WireEvent } from "../src/api/types";

class FakeSocket implements SocketLike {
  onopen: SocketLike["onopen"] = null;
  onmessage: SocketLike["onmessage"] = null;
  onclose: SocketLike["onclose"] = null;
  onerror: SocketLike["onerror"] = null;
  closed = false;
  constructor(readonly url: string) {}
  close() { this.closed = true; }
  open() { this.onopen?.({}); }
  send(m: unknown) { this.onmessage?.({ data: JSON.stringify(m) }); }
  drop() { this.onclose?.({}); }
}

function harness() {
  const sockets: FakeSocket[] = [];
  const timers: Array<{ fn: () => void; ms: number }> = [];
  const events: WireEvent[] = [];
  const statuses: StreamStatus[] = [];
  let caught = 0;
  const c = new StreamClient(
    {
      url: (from) => `ws://x/v1/stream?from=${from ?? 0}`,
      connect: (u) => { const s = new FakeSocket(u); sockets.push(s); return s; },
      setTimer: (fn, ms) => { timers.push({ fn, ms }); return timers.length; },
      clearTimer: () => {},
      random: () => 1, // no jitter reduction: delay = nominal
      baseDelayMs: 500,
      maxDelayMs: 4000,
    },
    {
      onEvent: (e) => events.push(e),
      onFrame: () => {},
      onCaughtUp: () => { caught++; },
      onStatus: (s) => statuses.push(s),
    },
  );
  return { c, sockets, timers, events, statuses, caught: () => caught };
}

const event = (position: number): WireEvent => ({ position, stream: "s", version: position, type: "T", at: "", data: {} });

describe("StreamClient", () => {
  test("backoff doubles up to the cap, with jitter in [0.5, 1)", () => {
    expect(backoffDelay(0, 500, 4000, 1)).toBe(500);
    expect(backoffDelay(3, 500, 4000, 1)).toBe(4000);
    expect(backoffDelay(10, 500, 4000, 1)).toBe(4000);
    expect(backoffDelay(1, 500, 4000, 0)).toBe(500);
  });

  test("starts from 0, then resumes from the last applied position", () => {
    const h = harness();
    h.c.start(null);
    expect(h.sockets[0]!.url).toBe("ws://x/v1/stream?from=0");
    h.sockets[0]!.open();
    h.sockets[0]!.send({ kind: "event", event: event(1) });
    h.sockets[0]!.send({ kind: "event", event: event(2) });
    h.sockets[0]!.send({ kind: "caught_up", position: 2 });
    h.sockets[0]!.send({ kind: "event", event: event(3) });
    expect(h.statuses).toContain("live");
    h.sockets[0]!.drop();
    expect(h.c.status).toBe("reconnecting");
    expect(h.timers).toHaveLength(1);
    expect(h.timers[0]!.ms).toBe(500);
    h.timers[0]!.fn();
    expect(h.sockets[1]!.url).toBe("ws://x/v1/stream?from=3");
    // The server may resend position 3 (inclusive `from`): it is dropped.
    h.sockets[1]!.open();
    h.sockets[1]!.send({ kind: "event", event: event(3) });
    h.sockets[1]!.send({ kind: "event", event: event(4) });
    h.sockets[1]!.send({ kind: "caught_up", position: 4 });
    expect(h.events.map((e) => e.position)).toEqual([1, 2, 3, 4]);
    expect(h.caught()).toBe(2);
  });

  test("backs off on repeated failures and resets after caught_up", () => {
    const h = harness();
    h.c.start(null);
    for (let i = 0; i < 4; i++) {
      h.sockets[i]!.drop();
      h.timers[i]!.fn();
    }
    expect(h.timers.map((t) => t.ms)).toEqual([500, 1000, 2000, 4000]);
    h.sockets[4]!.open();
    h.sockets[4]!.send({ kind: "caught_up", position: 0 });
    h.sockets[4]!.drop();
    expect(h.timers[4]!.ms).toBe(500);
  });

  test("caught_up ahead of us moves the resume point", () => {
    const h = harness();
    h.c.start(null);
    h.sockets[0]!.send({ kind: "caught_up", position: 77 });
    h.sockets[0]!.drop();
    h.timers[0]!.fn();
    expect(h.sockets[1]!.url).toBe("ws://x/v1/stream?from=77");
  });

  test("stop closes the socket and cancels reconnects; stale sockets are ignored", () => {
    const h = harness();
    h.c.start(null);
    const s = h.sockets[0]!;
    h.c.stop();
    expect(s.closed).toBe(true);
    s.send({ kind: "event", event: event(1) });
    s.drop();
    expect(h.events).toHaveLength(0);
    expect(h.timers).toHaveLength(0);
    expect(h.c.status).toBe("stopped");
  });

  test("ignores junk messages", () => {
    const h = harness();
    h.c.start(null);
    h.c.handle("not json");
    h.c.handle({ kind: "event" });
    h.c.handle({ kind: "mystery" });
    expect(h.events).toHaveLength(0);
  });
});
