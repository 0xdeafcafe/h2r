// What the web sends so the server shows a signed-in reader what its account
// may see (docs/live/live-api.md "Co-op games"): a `read` token on the
// games, account, log and stats reads, and an auth message on the stream.
import { describe, expect, test } from "bun:test";
import { api } from "../src/api/http";
import { StreamClient, type SocketLike } from "../src/state/stream";

const target = { base: "http://live.test", api: "http://live.test/v1" } as Parameters<typeof api>[0];

/** Swaps global fetch for one that records each URL and its bearer. */
function recordFetch(reply: (url: string) => unknown) {
  const seen: Array<{ url: string; auth: string | null }> = [];
  const real = globalThis.fetch;
  globalThis.fetch = (async (url: string, init?: RequestInit) => {
    const h = (init?.headers ?? {}) as Record<string, string>;
    seen.push({ url: String(url), auth: h.authorization ?? null });
    return new Response(JSON.stringify(reply(String(url))), { headers: { "content-type": "application/json" } });
  }) as typeof fetch;
  return { seen, restore: () => { globalThis.fetch = real; } };
}

describe("reads as the signed-in reader", () => {
  test("games, a game, an account, co-op games, the log and stats send the reader's token", async () => {
    const f = recordFetch(() => ({ items: [], next: null, position: 1 }));
    try {
      let n = 0;
      const a = api(target, async () => `tok${++n}`);
      await a.games("ended");
      await a.game("g1");
      await a.account("Pete");
      await a.coopGames({});
      await a.events({ from: 0 });
      await a.statsSummary("7d");
      expect(f.seen.map((s) => s.auth)).toEqual(["Bearer tok1", "Bearer tok2", "Bearer tok3", "Bearer tok4", "Bearer tok5", "Bearer tok6"]);
    } finally { f.restore(); }
  });

  test("signed out, nothing is sent", async () => {
    const f = recordFetch(() => ({ items: [], next: null }));
    try {
      await api(target).games("live");
      await api(target, async () => null).account("Pete");
      expect(f.seen.map((s) => s.auth)).toEqual([null, null]);
    } finally { f.restore(); }
  });

  test("the log follows next past empty pages (hidden events still move the cursor)", async () => {
    const pages: Record<string, unknown> = {
      "": { items: [{ position: 1 }], next: "3" },
      "3": { items: [], next: "6" },
      "6": { items: [{ position: 7 }], next: null },
    };
    const f = recordFetch((url) => pages[new URL(url).searchParams.get("cursor") ?? ""]);
    try {
      const evs = await api(target).streamEvents("game-x");
      expect(evs.map((e) => e.position)).toEqual([1, 7]);
      expect(f.seen.length).toBe(3);
    } finally { f.restore(); }
  });
});

class Sock implements SocketLike {
  onopen: SocketLike["onopen"] = null;
  onmessage: SocketLike["onmessage"] = null;
  onclose: SocketLike["onclose"] = null;
  onerror: SocketLike["onerror"] = null;
  sent: string[] = [];
  constructor(readonly url: string) {}
  close() {}
  send(d: string) { this.sent.push(d); }
}

function client(token: string | null) {
  const socks: Sock[] = [];
  const c = new StreamClient(
    {
      url: (from, auth) => `ws://x/v1/stream?from=${from ?? 0}${auth ? "&auth=1" : ""}`,
      connect: (u) => { const s = new Sock(u); socks.push(s); return s; },
      auth: async () => token,
      setTimer: () => 0,
      clearTimer: () => {},
    },
    { onEvent: () => {}, onFrame: () => {}, onCaughtUp: () => {} },
  );
  return { c, socks };
}

describe("the stream as the signed-in reader", () => {
  test("signed in: asks with auth=1 and sends the token first", async () => {
    const { c, socks } = client("tok");
    c.start(null);
    await Bun.sleep(0);
    expect(socks[0]!.url).toBe("ws://x/v1/stream?from=0&auth=1");
    socks[0]!.onopen?.({});
    expect(socks[0]!.sent.map((m) => JSON.parse(m))).toEqual([{ kind: "auth", token: "tok" }]);
  });

  test("signed out: an anonymous socket, nothing sent", async () => {
    const { c, socks } = client(null);
    c.start(null);
    await Bun.sleep(0);
    expect(socks[0]!.url).toBe("ws://x/v1/stream?from=0");
    socks[0]!.onopen?.({});
    expect(socks[0]!.sent).toEqual([]);
  });
});
