// Bans (docs/live/live-admin.md, "Bans"): the list's parsing, the ban
// rules, the calls with their token scopes and bodies, the player's notice,
// the commendation counts, and the mock's routes. Every gamertag, id and
// reason is invented.

import { describe, expect, test } from "bun:test";
import { banAccount, banProblem, banUntil, fetchBan, fetchBans, liftBan, parseBanStatus, parseBans, parseCommendations, untilText } from "../src/admin/bans";
import type { LocalAccount } from "../src/state/account";
import { parseJws } from "../src/state/verify";
import { serverPods } from "../src/admin/server";

const RFC = { d: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A", x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo", kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k" };
const me = (): LocalAccount => ({ xuid: "0009000000000042", gamertag: "afr", id: "acct:0009000000000042@localhost", server: "http://127.0.0.1:8074", server_id: "localhost", kid: RFC.kid, key: RFC.d, x: RFC.x });

function recorder(reply: (url: string, init: RequestInit) => Response) {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const f = (async (url: string, init: RequestInit = {}) => { calls.push({ url, init }); return reply(url, init); }) as unknown as typeof fetch;
  return { f, calls };
}
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const lxm = (init: RequestInit) => (parseJws((init.headers as Record<string, string>).authorization!.slice(7))!.payload as { lxm: string }).lxm;

const T = { xuid: "0009000000000001", gamertag: "Grunt Birthday" };

describe("the ban list", () => {
  test("reads bans newest first, dropping ones it can't read", () => {
    const l = parseBans({
      items: [
        { id: 4, target: T, reason: "aimbot", by: "Boss", at: "2026-11-18T09:00:00Z", state: "replaced", report: 3 },
        { id: 9, target: T, reason: "again", until: "2026-11-19T09:00:00Z", at: "2026-11-18T10:00:00Z", state: "active" },
        { id: 6, target: T, reason: "x", at: "a", state: "lifted", lifted: { note: "appeal", at: "b" } },
        { id: 0, target: T, reason: "bad id" },
        { id: 7, target: {}, reason: "no target" },
        { id: 8, target: T, reason: "odd state", state: "melted" },
      ],
      position: 50,
    });
    expect(l.items.map((b) => b.id)).toEqual([9, 8, 6, 4]);
    expect(l.items[0]).toMatchObject({ until: "2026-11-19T09:00:00Z", by: null, report: null, state: "active", lifted: null });
    expect(l.items[1]!.state).toBe("active");
    expect(l.items[2]!.lifted).toEqual({ note: "appeal", by: null, at: "b" });
    expect(l.items[3]).toMatchObject({ by: "Boss", report: 3, until: null, state: "replaced" });
    expect(l.position).toBe(50);
    expect(parseBans(null)).toEqual({ items: [], position: null });
  });

  test("the notice and the commendations", () => {
    expect(parseBanStatus({ banned: false })).toBeNull();
    expect(parseBanStatus({ banned: true, ban: { id: 9, reason: "r", at: "t" } })).toEqual({ id: 9, reason: "r", until: null, at: "t" });
    expect(parseCommendations({ xuid: T.xuid, good_attitude: 3, good_session: -1 })).toEqual({ good_attitude: 3, good_session: 0 });
  });

  test("a ban needs a reason; lengths become an end date", () => {
    expect(banProblem("  ")).toBe("say why, for the player");
    expect(banProblem("x".repeat(501))).toBe("a reason is at most 500 characters");
    expect(banProblem("aimbot")).toBeNull();
    const now = Date.parse("2026-11-18T12:00:00.250Z");
    expect(banUntil(1, now)).toBe("2026-11-19T12:00:00Z");
    expect(banUntil(null, now)).toBeNull();
    expect(untilText(null)).toBe("until lifted");
    expect(untilText("nonsense")).toBe("until nonsense");
  });

  test("the bans screen sits with the reports for admins", () => {
    expect(serverPods("bans", true).map((p) => p.to)).toEqual(["/dash/server/accounts", "/dash/server/reports", "/dash/players", "/dash/server"]);
    expect(serverPods("bans", false)).toEqual([]);
    expect(serverPods("reports", true).map((p) => p.to)).toContain("/dash/server/bans");
  });
});

describe("the calls", () => {
  test("list, ban, lift and the notice, each with its scope", async () => {
    const { f, calls } = recorder((url) => (url.includes("/ban") && !url.includes("/admin/") ? json({ banned: true, ban: { id: 9, reason: "r", at: "t" } }) : json({ ok: true, position: 9, items: [] }, url.endsWith("/0009000000000001") ? 201 : 200)));
    await fetchBans("http://h/v1", "localhost", me(), "all", f);
    expect(await banAccount("http://h/v1", "localhost", me(), T.xuid, { reason: " aimbot ", days: 7, report: 3 }, f, Date.parse("2026-11-18T12:00:00Z"))).toBe(9);
    await banAccount("http://h/v1", "localhost", me(), T.xuid, { reason: "again", days: null }, f);
    await liftBan("http://h/v1", "localhost", me(), T.xuid, " appeal ", f);
    expect(await fetchBan("http://h/v1", "localhost", me(), T.xuid, f)).toMatchObject({ id: 9 });
    expect(calls.map((c) => c.url)).toEqual([
      "http://h/v1/admin/bans?state=all",
      `http://h/v1/admin/bans/${T.xuid}`,
      `http://h/v1/admin/bans/${T.xuid}`,
      `http://h/v1/admin/bans/${T.xuid}/lift`,
      `http://h/v1/accounts/by-xuid/${T.xuid}/ban`,
    ]);
    expect(calls.map((c) => lxm(c.init))).toEqual(["reports.moderate", "reports.moderate", "reports.moderate", "reports.moderate", "read"]);
    expect(JSON.parse(calls[1]!.init.body as string)).toEqual({ reason: "aimbot", until: "2026-11-25T12:00:00Z", report: 3 });
    expect(JSON.parse(calls[2]!.init.body as string)).toEqual({ reason: "again" });
    expect(JSON.parse(calls[3]!.init.body as string)).toEqual({ note: "appeal" });
  });

  test("a ban without a reason is never sent; the server's refusal comes back", async () => {
    const { f, calls } = recorder(() => json({ error: "forbidden", message: "admins can't be banned" }, 403));
    await expect(banAccount("http://h/v1", "localhost", me(), T.xuid, { reason: " ", days: 1 }, f)).rejects.toThrow("say why");
    expect(calls).toHaveLength(0);
    await expect(banAccount("http://h/v1", "localhost", me(), T.xuid, { reason: "x", days: 1 }, f)).rejects.toMatchObject({ status: 403, code: "forbidden" });
  });
});

describe("the mock's bans", async () => {
  const { Bans } = await import("../mock/bans");
  const { Reports } = await import("../mock/reports");
  const { Admin } = await import("../mock/admin");
  const { World } = await import("../mock/world");
  const world = new World();
  for (const tag of ["Alpha", "Bravo", "Charlie", "Delta", "Echo"]) world.createAccount(tag);
  const admin = new Admin(world, false);
  const reports = new Reports(world, admin);
  const bans = new Bans(world, admin, reports);
  const call = (method: string, path: string, auth?: string, body?: unknown) =>
    bans.handle(new Request("http://m" + path, { method, headers: auth ? { authorization: auth } : {}, body: body === undefined ? undefined : JSON.stringify(body) }), new URL("http://m" + path));

  test("reads like the server's list, and only admins read it", async () => {
    expect((await call("GET", "/v1/admin/bans"))!.status).toBe(401);
    expect(await call("GET", "/v1/maps")).toBeNull();
    expect(parseBans(bans.view("active")).items).toEqual([]);
    const all = parseBans(bans.view("all"));
    expect(all.items.map((b) => b.state)).toEqual(["expired"]);
  });

  test("commendations count the reports' positive kinds, by gamertag or XUID", async () => {
    const target = reports.reports.find((r) => r.kind === "good_session")!.target;
    for (const who of [target.xuid, target.gamertag]) {
      const r = await call("GET", `/v1/accounts/${encodeURIComponent(who)}/commendations`);
      expect(r!.status).toBe(200);
      expect(parseCommendations(r!.body)).toEqual({ good_attitude: 0, good_session: 1 });
    }
    expect((await call("GET", "/v1/accounts/Nobody/commendations"))!.status).toBe(404);
  });
});
