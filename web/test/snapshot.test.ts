// The page load's first read: one request where the server has
// `GET /v1/snapshot`, the old reads (with the log's tail without GameFrames)
// where it does not.
import { describe, expect, test } from "bun:test";
import { api } from "../src/api/http";
import { ENDED_DETAILS, loadSnapshot } from "../src/state/snapshot";

const target = { base: "http://live.test", api: "http://live.test/v1" } as Parameters<typeof api>[0];

function serve(reply: (url: URL) => { status?: number; body: unknown }) {
  const seen: URL[] = [];
  const real = globalThis.fetch;
  globalThis.fetch = (async (url: string) => {
    const u = new URL(String(url));
    seen.push(u);
    const r = reply(u);
    return new Response(JSON.stringify(r.body), { status: r.status ?? 200, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
  return { seen, restore: () => { globalThis.fetch = real; } };
}

const summary = (id: string) => ({ game_id: id, map: "lockout", mode: "slayer" });
const pageOf = (items: unknown[], position = 10) => ({ items, next: null, position });

describe("page-load snapshot", () => {
  test("one request: the server's snapshot, details joined to their games", async () => {
    const ended = Array.from({ length: ENDED_DETAILS + 2 }, (_, i) => summary(`e${i}`));
    const f = serve(() => ({
      body: {
        position: 9,
        accounts: pageOf([{ xuid: "1", gamertag: "A" }]),
        presence: pageOf([]),
        live: pageOf([summary("l0")]),
        ended: pageOf(ended),
        details: [{ ...summary("l0"), host: "A" }, { ...summary("e0"), host: "A" }],
        tail: [{ position: 8, type: "GameStarted" }],
      },
    }));
    try {
      const s = (await loadSnapshot(api(target)))!;
      expect(f.seen.map((u) => u.pathname)).toEqual(["/v1/snapshot"]);
      expect(s.position).toBe(9);
      expect(s.games.length).toBe(1 + ENDED_DETAILS + 2);
      expect(s.games.filter((g) => "host" in g).map((g) => g.game_id).sort()).toEqual(["e0", "l0"]);
      expect(s.tail.length).toBe(1);
    } finally { f.restore(); }
  });

  test("an older server: the reads one by one, the tail without GameFrames", async () => {
    const f = serve((u) => {
      if (u.pathname === "/v1/snapshot") return { status: 404, body: { error: "not_found", message: "no such endpoint" } };
      if (u.pathname === "/v1/games") return { body: pageOf(u.searchParams.get("state") === "live" ? [summary("l0")] : []) };
      if (u.pathname === "/v1/games/l0") return { body: { ...summary("l0"), host: "A", position: 10 } };
      if (u.pathname === "/v1/events") return { body: pageOf([{ position: 5, type: "GameStarted" }]) };
      return { body: pageOf([]) };
    });
    try {
      const s = (await loadSnapshot(api(target)))!;
      expect(s.games.map((g) => g.game_id)).toEqual(["l0"]);
      const events = f.seen.find((u) => u.pathname === "/v1/events")!;
      expect(events.searchParams.get("exclude")).toBe("GameFrame");
    } finally { f.restore(); }
  });
});
