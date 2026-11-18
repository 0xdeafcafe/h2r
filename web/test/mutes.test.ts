// The mute list on the web (docs/live/live-api.md "Mute list"): parsing,
// the calls with their token scopes, and the mock's routes. Every gamertag
// and id is invented.

import { describe, expect, test } from "bun:test";
import { fetchMutes, parseMutes, setMute } from "../src/admin/mutes";
import type { LocalAccount } from "../src/state/account";
import { parseJws } from "../src/state/verify";
import { issueToken } from "../src/admin/token";

const RFC = { d: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A", x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo", kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k" };
const me = (xuid = "0009000000000042"): LocalAccount => ({ xuid, gamertag: "afr", id: `acct:${xuid}@localhost`, server: "http://127.0.0.1:8074", server_id: "localhost", kid: RFC.kid, key: RFC.d, x: RFC.x });

function recorder(reply: (url: string, init: RequestInit) => Response) {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const f = (async (url: string, init: RequestInit = {}) => { calls.push({ url, init }); return reply(url, init); }) as unknown as typeof fetch;
  return { f, calls };
}
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const lxm = (init: RequestInit) => (parseJws((init.headers as Record<string, string>).authorization!.slice(7))!.payload as { lxm: string }).lxm;
const T = "0009000000000001";

describe("the mute list", () => {
  test("reads entries oldest first, dropping bad ones", () => {
    expect(parseMutes({ items: [{ xuid: T, gamertag: "Loud" }, { xuid: "nope" }, { xuid: "0009000000000002" }] }))
      .toEqual([{ xuid: T, gamertag: "Loud" }, { xuid: "0009000000000002", gamertag: "unknown" }]);
    expect(parseMutes(null)).toEqual([]);
  });

  test("read with a read token, changed with mutes.write", async () => {
    const { f, calls } = recorder((_url, init) => json(init.method ? { ok: true, changed: true } : { items: [{ xuid: T, gamertag: "Loud" }] }));
    expect(await fetchMutes("http://h/v1", "localhost", me(), f)).toEqual([{ xuid: T, gamertag: "Loud" }]);
    expect(await setMute("http://h/v1", "localhost", me(), T, true, f)).toBe(true);
    await setMute("http://h/v1", "localhost", me(), T, false, f);
    expect(calls.map((c) => [c.init.method ?? "GET", c.url, lxm(c.init)])).toEqual([
      ["GET", "http://h/v1/accounts/by-xuid/0009000000000042/mutes", "read"],
      ["PUT", `http://h/v1/accounts/by-xuid/0009000000000042/mutes/${T}`, "mutes.write"],
      ["DELETE", `http://h/v1/accounts/by-xuid/0009000000000042/mutes/${T}`, "mutes.write"],
    ]);
    await expect(setMute("http://h/v1", "localhost", me(), me().xuid, true, f)).rejects.toThrow("yourself");
  });
});

describe("the mock's mute lists", async () => {
  const { Mutes } = await import("../mock/mutes");
  const { Admin } = await import("../mock/admin");
  const { World } = await import("../mock/world");
  const world = new World();
  const a = world.createAccount("Alpha"), b = world.createAccount("Bravo");
  const mutes = new Mutes(world, new Admin(world, false));
  const call = async (method: string, path: string, scope: string) => {
    const token = await issueToken(me(a.xuid), "localhost", scope);
    return mutes.handle(new Request("http://m" + path, { method, headers: { authorization: `Bearer ${token}` } }), new URL("http://m" + path));
  };

  test("mutes, repeats and unmutes like the server", async () => {
    const base = `/v1/accounts/by-xuid/${a.xuid}/mutes`;
    expect(mutes.handle(new Request("http://m/v1/maps"), new URL("http://m/v1/maps"))).toBeNull();
    expect((await call("PUT", `${base}/${b.xuid}`, "mutes.write"))!.status).toBe(201);
    expect((await call("PUT", `${base}/${b.xuid}`, "mutes.write"))!.body).toEqual({ ok: true, changed: false });
    expect((await call("PUT", `${base}/${a.xuid}`, "mutes.write"))!.status).toBe(400);
    expect((await call("PUT", `${base}/${b.xuid}`, "read"))!.status).toBe(401);
    expect(parseMutes((await call("GET", base, "read"))!.body)).toEqual([{ xuid: b.xuid, gamertag: "Bravo" }]);
    expect((await call("DELETE", `${base}/${b.xuid}`, "mutes.write"))!.status).toBe(200);
    expect(parseMutes((await call("GET", base, "read"))!.body)).toEqual([]);
  });
});
