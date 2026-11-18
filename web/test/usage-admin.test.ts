// The Live admin's usage reads (src/admin/usage.ts): defensive parsing of
// the server's answers, the signed requests, the mock's endpoints, and the
// table column each server sort fills.
import { describe, expect, test } from "bun:test";
import {
  ACCOUNT_SORTS, fetchUsage, fetchUsageAccounts, parseAccount, parseAccounts, parseOverview,
} from "../src/admin/usage";
import type { LocalAccount } from "../src/state/account";
import { parseJws } from "../src/state/verify";
import { usageReply } from "../mock/admin";
import { USAGE_SORTS, sortCol, usageView } from "../src/deck/usageView";

const ME: LocalAccount = {
  xuid: "0009000000000042", gamertag: "afr", id: "acct:0009000000000042@localhost", server: "http://127.0.0.1:8074",
  server_id: "localhost", kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k",
  key: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A", x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
};

// As docs/live/live-api.md "Usage" shows it.
const OVERVIEW = {
  from: "2026-11-17", to: "2026-11-18", keep_days: 400,
  days: [
    { day: "2026-11-17", up: { events: 10, frames: 20, content: 0, saves: 5 }, events: 7, messages: { friend_requests: 1, invites: 2 },
      games: 1, sessions_hosted: 1, sign_ins: 3, reports: 0, active_accounts: 2 },
    { day: "2026-11-18", up: { events: 4 }, events: 2 },
  ],
  totals: { day: "", up: { events: 14, frames: 20, content: 0, saves: 5 }, events: 9 },
  accounts_active: 2,
  event_types: [{ type: "GameStarted", count: 1 }, { type: "SignedIn", count: 3 }],
  stored: { events: 900, content: 0, saves: 5 },
};

function recorder(body: unknown, status = 200) {
  const urls: string[] = [], auth: string[] = [];
  const f = (async (url: string, init: RequestInit = {}) => {
    urls.push(url);
    auth.push((init.headers as Record<string, string>).authorization ?? "");
    return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  }) as unknown as typeof fetch;
  return { f, urls, auth };
}

describe("parsing", () => {
  test("the documented overview reads whole; missing fields are zero", () => {
    const o = parseOverview(OVERVIEW);
    expect(o.days).toHaveLength(2);
    expect(o.days[0]!.messages).toEqual({ friend_requests: 1, invites: 2 });
    expect(o.days[1]).toMatchObject({ day: "2026-11-18", up: { events: 4, frames: 0, content: 0, saves: 0 }, events: 2, games: 0, sign_ins: 0 });
    expect(o.stored).toEqual({ events: 900, content: 0, saves: 5 });
    expect(o.event_types.map((t) => t.type)).toEqual(["GameStarted", "SignedIn"]);
  });

  test("junk reads as an empty overview, not a throw; bad days and types are dropped", () => {
    for (const junk of [null, 7, "x", [], { days: "no", event_types: {}, stored: [] }]) {
      const o = parseOverview(junk);
      expect(o.days).toEqual([]);
      expect(o.event_types).toEqual([]);
      expect(o.keep_days).toBe(400);
    }
    const o = parseOverview({ days: [{ day: "yesterday" }, { day: "2026-11-01", events: "many" }], event_types: [{ count: 3 }] });
    expect(o.days).toEqual([expect.objectContaining({ day: "2026-11-01", events: 0 })]);
    expect(o.event_types).toEqual([]);
  });

  test("an account's totals fall back to the sum of its kinds; an account without a xuid is dropped", () => {
    const a = parseAccount({ xuid: "0009", up: { events: 1, frames: 2 }, stored: { saves: 4 }, messages: { invites: 3 }, last_active: "soon" });
    expect(a).toMatchObject({ uploaded: 3, stored_total: 4, messages_total: 3, gamertag: null, last_active: null });
    const p = parseAccounts({ days: 30, sort: "uploaded", total: 2, items: [{ xuid: "0009", gamertag: "A" }, { gamertag: "no xuid" }] });
    expect(p.items.map((x) => x.gamertag)).toEqual(["A"]);
    expect(p.total).toBe(2);
  });

  test("the parsed overview draws: the tiles and charts read straight from it", () => {
    const v = usageView(parseOverview(OVERVIEW), null, "2026-11-18", 2);
    expect(v.tiles.find((t) => t.key === "uploaded")!.value).toBe(39);
    expect(v.tiles.find((t) => t.key === "messages")!.value).toBe(3);
    expect(v.storedTotal).toBe(905);
  });
});

describe("reads", () => {
  test("both reads carry an admin.read token for this server and the query", async () => {
    const r = recorder(OVERVIEW);
    const o = await fetchUsage("http://h/v1", "live.example.net", ME, 30, r.f);
    expect(o.to).toBe("2026-11-18");
    const a = recorder({ days: 7, sort: "stored", total: 0, items: [] });
    await fetchUsageAccounts("http://h/v1", "live.example.net", ME, 7, "stored", 25, a.f);
    expect(r.urls).toEqual(["http://h/v1/admin/usage?days=30"]);
    expect(a.urls).toEqual(["http://h/v1/admin/usage/accounts?days=7&sort=stored&limit=25"]);
    for (const h of [...r.auth, ...a.auth]) {
      const claims = parseJws(h.slice(7))!.payload as Record<string, unknown>;
      expect(claims).toMatchObject({ lxm: "admin.read", aud: "live.example.net" });
    }
  });

  test("a refusal is an error with the server's code", async () => {
    const r = recorder({ error: "forbidden", message: "this needs an admin" }, 403);
    await expect(fetchUsage("http://h/v1", "x", ME, 30, r.f)).rejects.toMatchObject({ status: 403, code: "forbidden" });
  });
});

describe("mock endpoints", () => {
  const q = (s: string) => new URLSearchParams(s);

  test("the overview covers the asked days, ending today", () => {
    const r = usageReply("/v1/admin/usage", q("days=7"), "2026-11-18");
    expect(r.status).toBe(200);
    const o = parseOverview(r.body);
    expect(o.days).toHaveLength(7);
    expect(o.to).toBe("2026-11-18");
  });

  test("accounts come sorted and limited, as the server's do", () => {
    const r = usageReply("/v1/admin/usage/accounts", q("sort=messages&limit=5"), "2026-11-18");
    const p = parseAccounts(r.body);
    expect(p.items).toHaveLength(5);
    const m = p.items.map((a) => a.messages_total);
    expect(m).toEqual([...m].sort((a, b) => b - a));
  });

  test("out-of-range days, sorts and limits are refused like the server's", () => {
    for (const [path, s] of [["/v1/admin/usage", "days=0"], ["/v1/admin/usage", "days=401"],
      ["/v1/admin/usage/accounts", "sort=vibes"], ["/v1/admin/usage/accounts", "limit=501"]] as const) {
      expect(usageReply(path, q(s)).status).toBe(400);
    }
  });
});

describe("sorting", () => {
  test("every sort offered on the screen is the server's and fills a table column", () => {
    for (const s of USAGE_SORTS) expect(ACCOUNT_SORTS).toContain(s.value);
    expect(USAGE_SORTS.map((s) => sortCol(s.value))).toEqual(["up", "stored", "events", "msgs", "reports"]);
    expect(sortCol("anything else")).toBe("up");
  });
});
