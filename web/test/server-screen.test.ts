import { describe, expect, test } from "bun:test";
import {
  INVITE_MAX_DAYS,
  artWords,
  createInvite,
  customCodeError,
  fetchAdminStatus,
  fetchInvites,
  fetchMyInvites,
  writeInviteGrant,
  fetchServerFacts,
  formatUptime,
  inviteBody,
  inviteData,
  parseAdminStatus,
  parseInvites,
  parseServerFacts,
  reachWords,
  revokeInvite,
  serverPods,
  adminSourceWords,
  HALO_ADMIN,
  LIVE_ADMIN_USAGE,
} from "../src/admin/server";
import { applyAccountsSnapshot, applyEvent, initialState } from "../src/state/reducer";
import type { LocalAccount } from "../src/state/account";
import { canonicalJson, parseJws, b64urlDecode } from "../src/state/verify";
import { Admin, inviteKey } from "../mock/admin";
import { HOST, World } from "../mock/world";

const RFC = {
  d: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A",
  x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
  kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k",
};

function me(xuid = "0009000000000042", server = "localhost"): LocalAccount {
  return { xuid, gamertag: "afr", id: `acct:${xuid}@${server}`, server: "http://127.0.0.1:8074", server_id: server, kid: RFC.kid, key: RFC.d, x: RFC.x };
}

async function verifies(jws: string): Promise<boolean> {
  const p = parseJws(jws)!;
  const k = await crypto.subtle.importKey("jwk", { kty: "OKP", crv: "Ed25519", x: RFC.x }, { name: "Ed25519" }, false, ["verify"]);
  return crypto.subtle.verify({ name: "Ed25519" }, k, p.signature, p.signingInput);
}

/** A fetch that records requests and answers from `reply`. */
function recorder(reply: (url: string, init: RequestInit) => Response) {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const f = (async (url: string, init: RequestInit = {}) => {
    calls.push({ url, init });
    return reply(url, init);
  }) as unknown as typeof fetch;
  return { f, calls };
}
const jsonResponse = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const STATUS = {
  id: "live.example.net", software: "h2recomp live-server 0.1.0", url: "https://a-b.trycloudflare.com",
  invite_only: true, private: false, federation: false,
  counts: { accounts: 12, online: 3, live_games: 1, sessions: 2 }, position: 900,
  started_at: "2026-11-16T10:00:00Z", uptime_s: 93784,
  public: { url: "https://a-b.trycloudflare.com", tunnel: "quick" },
  game_art: { present: true, shared: false, manifest_version: 1, extractor: 7, source: "abc", maps: 12 },
  admins: [{ gamertag: "afr", source: "configured" }, { gamertag: "Mod", xuid: "0009000000000007", source: "granted" }],
  log: { position: 900, head: 900, store: "file" },
  invites: { open: 2, used: 5, revoked: 1, expired: 0 },
};

describe("server facts and status", () => {
  test("the public facts parse, and an empty body is safe", () => {
    const f = parseServerFacts(STATUS);
    expect(f.id).toBe("live.example.net");
    expect(f.invite_only).toBe(true);
    expect(f.counts).toEqual({ accounts: 12, online: 3, live_games: 1, sessions: 2 });
    const empty = parseServerFacts(null);
    expect(empty).toEqual({ id: "", software: null, url: null, invite_only: false, private: false, federation: false, counts: null, position: null });
  });

  test("the admin status parses, with no data dir unless sent", () => {
    const s = parseAdminStatus(STATUS);
    expect(s.public).toEqual({ url: "https://a-b.trycloudflare.com", tunnel: "quick" });
    expect(s.game_art.extractor).toBe(7);
    expect(s.admins.map((a) => a.gamertag)).toEqual(["afr", "Mod"]);
    expect(s.admins[1]!.source).toBe("granted");
    expect(s.log).toEqual({ position: 900, head: 900, store: "file" });
    expect(s.invites.open).toBe(2);
    expect(s.data_dir).toBeNull();
    expect(parseAdminStatus({ ...STATUS, data_dir: "/Users/x/live" }).data_dir).toBe("/Users/x/live");
    // An unknown tunnel kind and a manifest without an extractor version.
    const odd = parseAdminStatus({ ...STATUS, public: { url: null, tunnel: "carrier-pigeon" }, game_art: { present: true, shared: true, extractor: 0 } });
    expect(odd.public.tunnel).toBeNull();
    expect(odd.game_art.extractor).toBeNull();
    expect(artWords(odd.game_art)).toBe("shared with everyone");
  });

  test("words and numbers on the plate", () => {
    expect(formatUptime(40)).toBe("40s");
    expect(formatUptime(12 * 60)).toBe("12m");
    expect(formatUptime(2 * 3600 + 5 * 60)).toBe("2h 05m");
    expect(formatUptime(93784)).toBe("1d 2h");
    expect(reachWords({ url: null, tunnel: null })).toBe("this machine only");
    expect(reachWords({ url: "https://x", tunnel: "quick" })).toContain("quick tunnel");
    expect(reachWords({ url: "https://x", tunnel: "named" })).toBe("named tunnel");
    expect(artWords(parseAdminStatus(STATUS).game_art)).toBe("this machine only · extractor v7");
    expect(artWords(parseAdminStatus({}).game_art)).toBe("none extracted");
    expect(adminSourceWords("configured")).toBe("set at startup");
    expect(adminSourceWords("granted")).toBe("granted");
  });

  test("an old server without /v1/server reads as null", async () => {
    const { f } = recorder(() => jsonResponse({ error: "not_found", message: "no such endpoint" }, 404));
    expect(await fetchServerFacts("http://h/v1", f)).toBeNull();
  });

  test("the status read carries an admin.read token for this server", async () => {
    const { f, calls } = recorder(() => jsonResponse(STATUS));
    const s = await fetchAdminStatus("http://h/v1", "live.example.net", me(), f);
    expect(s.uptime_s).toBe(93784);
    expect(calls[0]!.url).toBe("http://h/v1/admin/status");
    const token = (calls[0]!.init.headers as Record<string, string>).authorization!.slice(7);
    const claims = parseJws(token)!.payload as Record<string, unknown>;
    expect(claims.lxm).toBe("admin.read");
    expect(claims.aud).toBe("live.example.net");
    expect(await verifies(token)).toBe(true);
  });

  test("a refused status read is an error with the server's code", async () => {
    const { f } = recorder(() => jsonResponse({ error: "forbidden", message: "this needs an admin" }, 403));
    await expect(fetchAdminStatus("http://h/v1", "x", me(), f)).rejects.toMatchObject({ status: 403, code: "forbidden" });
  });
});

describe("admin gating", () => {
  test("visitors get the visitor pods; admin tools only for a known admin", () => {
    for (const who of [false, null] as const) {
      const pods = serverPods("overview", who);
      expect(pods.map((p) => p.label)).toEqual(["another server", "event log", "players", "stats site"]);
      expect(pods.some((p) => p.to?.startsWith("/games/halo2/admin") || p.to === "/dash/server/invites")).toBe(false);
      expect(serverPods("invites", who)).toEqual([]);
      expect(serverPods("accounts", who)).toEqual([]);
      // Importing is how a signed-out browser becomes a CLI account: open to all, no admin tools.
      const imp = serverPods("import", who);
      expect(imp.map((p) => p.to)).toEqual(["/dash/signin", "/dash/signup", "/dash/server/connect", "/dash/server"]);
    }
    const admin = serverPods("overview", true);
    expect(admin.map((p) => p.to)).toEqual(["/dash/server/invites", "/dash/server/accounts", HALO_ADMIN, "/dash/server/connect"]);
    expect(serverPods("invites", true).map((p) => p.run ?? p.to)).toEqual(["create", "copy", "/dash/server/accounts", "/dash/server"]);
    expect(serverPods("accounts", true).map((p) => p.to)).toEqual(["/dash/server/invites", "/dash/server/import", LIVE_ADMIN_USAGE, "/dash/server"]);
    expect(serverPods("import", true).map((p) => p.to)).toEqual(["/dash/server/accounts", "/dash/server/invites", HALO_ADMIN, "/dash/server"]);
    // Player reports are the service's, on the dashboard; visitors get no pods there.
    expect(serverPods("reports", true).map((p) => p.to)).toEqual(["/dash/server/accounts", "/dash/server/bans", "/dash/players", "/dash/server"]);
    expect(serverPods("reports", false)).toEqual([]);
    expect(serverPods("reports", null)).toEqual([]);
  });

  test("the dashboard keeps the service; the stats admin keeps only Halo's things", () => {
    // Every admin pod on the dashboard is a dashboard screen, apart from the one way into the Halo admin.
    for (const view of ["overview", "invites", "accounts", "import", "reports", "bans"] as const) {
      for (const p of serverPods(view, true)) {
        if (p.to?.startsWith("/games/halo2")) expect(p.to).toBe(HALO_ADMIN);
      }
    }
    expect(HALO_ADMIN).toBe("/games/halo2/admin");
    expect(LIVE_ADMIN_USAGE).toBe("/dash/admin/usage");
  });
});

describe("invites", () => {
  test("what an admin signs: a jti and an expiry clamped to 1..30 days", () => {
    const now = 1_800_000_000_000;
    const d = inviteData(7, now, "j".repeat(22));
    expect(d).toEqual({ jti: "j".repeat(22), exp: 1_800_000_000 + 7 * 86400 });
    expect(inviteData(365, now).exp).toBe(1_800_000_000 + INVITE_MAX_DAYS * 86400);
    expect(inviteData(0, now).exp).toBe(1_800_000_000 + 86400);
    expect(inviteData(1, now).jti).toMatch(/^[A-Za-z0-9_-]{22}$/);
  });

  test("creating one posts a signed InviteCreated event with an invites.write token", async () => {
    const now = Date.now();
    const { f, calls } = recorder(() => jsonResponse({ invite: "code.code.code", jti: "x", exp: 5 }, 201));
    const r = await createInvite("http://h/v1", "live.example.net", me(), 30, f, now);
    expect(r.invite).toBe("code.code.code");
    const c = calls[0]!;
    expect(c.url).toBe("http://h/v1/invites");
    expect(c.init.method).toBe("POST");
    const h = c.init.headers as Record<string, string>;
    expect(h["content-type"]).toBe("application/jose");
    const claims = parseJws(h.authorization!.slice(7))!.payload as Record<string, unknown>;
    expect(claims.lxm).toBe("invites.write");
    const jws = c.init.body as string;
    const raw = new TextDecoder().decode(b64urlDecode(jws.split(".")[1]!));
    expect(canonicalJson(JSON.parse(raw))).toBe(raw);
    const ev = JSON.parse(raw) as { stream: string; type: string; author: string; kid: string; data: { jti: string; exp: number } };
    expect(ev.stream).toBe("invites");
    expect(ev.type).toBe("InviteCreated");
    expect(ev.author).toBe("acct:0009000000000042@localhost");
    expect(ev.kid).toBe(RFC.kid);
    expect(ev.data.exp).toBe(Math.floor(now / 1000) + 30 * 86400);
    expect(ev.data.jti.length).toBeGreaterThanOrEqual(16);
    expect((parseJws(jws)!.header as { typ?: string }).typ).toBe("h2live-event");
    expect(await verifies(jws)).toBe(true);
  });

  test("the body is the event alone, or JSON {event, code} for a chosen code", () => {
    expect(inviteBody("a.b.c")).toEqual({ contentType: "application/jose", body: "a.b.c" });
    const b = inviteBody("a.b.c", "Pancake Night");
    expect(b.contentType).toBe("application/json");
    expect(JSON.parse(b.body)).toEqual({ event: "a.b.c", code: "Pancake Night" });
  });

  test("a custom code: 4 to 32 printable ASCII characters", () => {
    expect(customCodeError("abcd")).toBeNull();
    expect(customCodeError("Pancake Night - 2026!")).toBeNull();
    expect(customCodeError("x".repeat(32))).toBeNull();
    expect(customCodeError("abc")).not.toBeNull();
    expect(customCodeError("x".repeat(33))).not.toBeNull();
    expect(customCodeError("café")).not.toBeNull();
    expect(customCodeError("tab\there")).not.toBeNull();
  });

  test("creating one with a custom code posts JSON {event, code}; blank means generated", async () => {
    const { f, calls } = recorder((_u, init) => {
      const custom = new Headers(init.headers).get("content-type") === "application/json";
      return jsonResponse({ invite: custom ? "Pancake Night" : "BCDFG-HJKMP-QRTVW-XY234-6789B", jti: "x", exp: 5, custom }, 201);
    });
    const made = await createInvite("http://h/v1", "aud", me(), 7, f, Date.now(), "  Pancake Night ");
    expect(made).toMatchObject({ invite: "Pancake Night", custom: true });
    const h = calls[0]!.init.headers as Record<string, string>;
    expect(h["content-type"]).toBe("application/json");
    const body = JSON.parse(calls[0]!.init.body as string) as { event: string; code: string };
    expect(body.code).toBe("Pancake Night");
    const ev = parseJws(body.event)!.payload as { type: string };
    expect(ev.type).toBe("InviteCreated");
    expect(await verifies(body.event)).toBe(true);

    const gen = await createInvite("http://h/v1", "aud", me(), 7, f, Date.now(), "   ");
    expect(gen.custom).toBe(false);
    expect((calls[1]!.init.headers as Record<string, string>)["content-type"]).toBe("application/jose");
    await expect(createInvite("http://h/v1", "aud", me(), 7, f, Date.now(), "abc")).rejects.toMatchObject({ status: 400 });
    expect(calls).toHaveLength(2);
  });

  test("revoking signs InviteRevoked naming the same invite", async () => {
    const { f, calls } = recorder(() => jsonResponse({ jti: "abc", state: "revoked" }));
    await revokeInvite("http://h/v1", "aud", me(), "abcdefghijklmnop", f);
    expect(calls[0]!.url).toBe("http://h/v1/invites/abcdefghijklmnop/revoke");
    const ev = parseJws(calls[0]!.init.body as string)!.payload as { type: string; data: { jti: string } };
    expect(ev.type).toBe("InviteRevoked");
    expect(ev.data.jti).toBe("abcdefghijklmnop");
  });

  test("the list parses states and keeps codes only where sent", () => {
    const items = parseInvites({ items: [
      { jti: "a", by: "acct:1@x", by_gamertag: "afr", at: "t", exp: 9, state: "open", invite: "c1" },
      { jti: "b", by: "acct:1@x", at: "t", exp: 9, state: "used", used_by_gamertag: "Newcomer" },
      { jti: "c", by: "acct:1@x", at: "t", exp: 9, state: "open", invite: null, custom: true },
      { nope: true },
    ] });
    expect(items.map((i) => [i.jti, i.state, i.invite, i.custom])).toEqual([["a", "open", "c1", false], ["b", "used", null, false], ["c", "open", null, true]]);
    expect(items[1]!.used_by_gamertag).toBe("Newcomer");
    expect(parseInvites({})).toEqual([]);
  });
});

describe("the mock's server endpoints", () => {
  async function setup(everyoneAdmin: boolean) {
    const world = new World();
    const a = world.createExternalAccount("afr", { kty: "OKP", crv: "Ed25519", x: RFC.x, kid: RFC.kid });
    const admin = new Admin(world, everyoneAdmin);
    const account = me(a.xuid, HOST);
    const f = (async (url: string, init: RequestInit = {}) => {
      const req = new Request(url, init);
      const r = await admin.handle(req, new URL(url));
      return r ? jsonResponse(r.body, r.status) : jsonResponse({ error: "not_found" }, 404);
    }) as unknown as typeof fetch;
    return { world, admin, account, f };
  }

  test("public facts for anyone; status only for an admin (H2_MOCK_ADMIN)", async () => {
    const off = await setup(false);
    expect((await fetchServerFacts("http://m/v1", off.f))!.id).toBe(HOST);
    await expect(fetchAdminStatus("http://m/v1", HOST, off.account, off.f)).rejects.toMatchObject({ status: 403 });
    const anon = await off.f("http://m/v1/admin/status");
    expect(anon.status).toBe(401);

    const on = await setup(true);
    const s = await fetchAdminStatus("http://m/v1", HOST, on.account, on.f);
    expect(s.admins.map((a) => a.gamertag)).toEqual(["afr"]);
    expect(on.admin.isAdmin(on.world.accounts.get(on.account.xuid))).toBe(true);
  });

  test("an invite round trip: create, list with its code, revoke", async () => {
    const { account, f } = await setup(true);
    const made = await createInvite("http://m/v1", HOST, account, 7, f);
    expect(made.invite).toMatch(/^[BCDFGHJKMPQRTVWXY2346789]{5}(-[BCDFGHJKMPQRTVWXY2346789]{5}){4}$/);
    expect(made.custom).toBe(false);
    let list = await fetchInvites("http://m/v1", HOST, account, f);
    expect(list[0]!.state).toBe("open");
    expect(list[0]!.invite).toBe(made.invite);
    await revokeInvite("http://m/v1", HOST, account, made.jti, f);
    list = await fetchInvites("http://m/v1", HOST, account, f);
    expect(list[0]!.state).toBe("revoked");
    expect(list[0]!.invite).toBeNull();
    await expect(revokeInvite("http://m/v1", HOST, account, made.jti, f)).rejects.toMatchObject({ status: 409 });
  });

  test("enabling and revoking invite access signs admin requests and updates the player projection", async () => {
    const { world, account, f } = await setup(true);
    const target = world.createAccount("Player One");
    const player = { ...me(target.xuid, HOST), gamertag: target.gamertag };
    const s = initialState();
    applyAccountsSnapshot(s, [world.publicAccount(target)]);
    expect(s.accounts[target.xuid]?.invite_grant).toBeNull();
    await expect(fetchMyInvites("http://m/v1", HOST, player, f)).rejects.toMatchObject({ status: 403 });

    const calls: Request[] = [];
    const send = (async (url: string, init: RequestInit) => {
      calls.push(new Request(url, init));
      return f(url, init);
    }) as unknown as typeof fetch;
    await writeInviteGrant("http://m/v1", HOST, account, target.gamertag, { open_cap: 1 }, send);
    const grant = world.log.at(-1)!;
    expect(grant.type).toBe("InviteGrantSet");
    applyEvent(s, grant);
    expect(s.accounts[target.xuid]?.invite_grant).toEqual({ open_cap: 1 });
    const made = await createInvite("http://m/v1", HOST, player, 7, f);
    expect(await fetchMyInvites("http://m/v1", HOST, player, f)).toMatchObject([{ jti: made.jti, invite: made.invite }]);
    await expect(createInvite("http://m/v1", HOST, player, 7, f)).rejects.toMatchObject({ status: 409 });

    await writeInviteGrant("http://m/v1", HOST, account, target.gamertag, null, send);
    const revoke = world.log.at(-1)!;
    expect(revoke.type).toBe("InviteGrantRevoked");
    applyEvent(s, revoke);
    expect(s.accounts[target.xuid]?.invite_grant).toBeNull();
    await expect(fetchMyInvites("http://m/v1", HOST, player, f)).rejects.toMatchObject({ status: 403 });
    expect(calls.map((r) => r.method)).toEqual(["POST", "DELETE"]);
    for (const r of calls) {
      expect(r.url).toBe(`http://m/v1/accounts/${encodeURIComponent(target.gamertag)}/invite-grant`);
      const token = r.headers.get("authorization")!.slice(7);
      expect(parseJws(token)?.payload).toMatchObject({ iss: account.id, aud: HOST, lxm: "invite-grants.write" });
      expect(await verifies(token)).toBe(true);
    }
  });

  test("an admin keeps both open codes after removing a one-invite grant", async () => {
    const { account, f } = await setup(true);
    await writeInviteGrant("http://m/v1", HOST, account, account.gamertag, { open_cap: 1 }, f);
    const first = await createInvite("http://m/v1", HOST, account, 7, f);
    const second = await createInvite("http://m/v1", HOST, account, 7, f);
    await writeInviteGrant("http://m/v1", HOST, account, account.gamertag, null, f);
    const open = await fetchMyInvites("http://m/v1", HOST, account, f);
    expect(open.map((i) => i.jti)).toEqual([second.jti, first.jti]);
  });

  test("a custom code: made, listed as custom without the code, matched loosely, unique while open", async () => {
    const { admin, account, f } = await setup(true);
    const made = await createInvite("http://m/v1", HOST, account, 7, f, Date.now(), "Pancake-Night");
    expect(made).toMatchObject({ invite: "Pancake-Night", custom: true });
    const list = await fetchInvites("http://m/v1", HOST, account, f);
    expect(list[0]).toMatchObject({ state: "open", custom: true, invite: null });
    expect(admin.inviteFor("pancake night")?.jti).toBe(made.jti);
    expect(inviteKey("bcdfg-hjkmp qrtvw")).toBe("BCDFGHJKMPQRTVW");
    await expect(createInvite("http://m/v1", HOST, account, 7, f, Date.now(), "PANCAKE NIGHT")).rejects.toMatchObject({ status: 409 });
  });
});
