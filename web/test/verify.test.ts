import { describe, expect, test } from "bun:test";
import { World } from "../mock/world";
import { newSigningKey, signJws, chainHashSync } from "../mock/sign";
import { KeyCache, Verifier, bodyMatches, canonicalJson, chainHash, jwkThumbprint, verifyEvent, type VerifyResult } from "../src/state/verify";
import type { WireEvent } from "../src/api/types";
import { applyAccountsSnapshot, applyEvents, applyGameSnapshot, applyPresenceSnapshot, initialState } from "../src/state/reducer";

function keysFor(world: World) {
  let accountFetches = 0;
  const cache = new KeyCache({
    serverKeys: async () => [world.serverKey.jwk],
    accountKeys: async (author) => {
      accountFetches++;
      const xuid = /^acct:([0-9a-f]{16})@/.exec(author)?.[1];
      const a = xuid ? world.accounts.get(xuid) : undefined;
      return a ? world.accountKeys(a).keys : null;
    },
  });
  return { cache, fetches: () => accountFetches };
}

function smallWorld() {
  const w = new World();
  w.seed(90_000); // 90 s of simulated history
  return w;
}

describe("canonical JSON and thumbprints", () => {
  test("sorted keys, no whitespace, undefined dropped", () => {
    expect(canonicalJson({ b: 1, a: [true, null, { d: "x", c: undefined }] })).toBe('{"a":[true,null,{"d":"x"}],"b":1}');
  });
  test("kid is the RFC 7638 thumbprint", async () => {
    const k = newSigningKey();
    expect(await jwkThumbprint(k.jwk)).toBe(k.kid);
  });
});

describe("event verification (real Ed25519 from the mock)", () => {
  test("every event of a simulated world verifies, account and server authored", async () => {
    const w = smallWorld();
    const { cache, fetches } = keysFor(w);
    const results = await Promise.all(w.log.map((e) => verifyEvent(e, cache)));
    const bad = results.filter((r) => r.status !== "verified");
    expect(bad).toEqual([]);
    expect(w.log.some((e) => e.author?.startsWith("server:"))).toBe(true);
    expect(w.log.some((e) => e.author?.startsWith("acct:"))).toBe(true);
    // Keys are fetched once per author, then cached by kid.
    expect(fetches()).toBeLessThanOrEqual(w.accounts.size);
  });

  test("a tampered data copy is caught", async () => {
    const w = smallWorld();
    const { cache } = keysFor(w);
    const kill = w.log.find((e) => e.type === "PlayerKilled") ?? w.log.find((e) => e.type === "SignedIn")!;
    const forged: WireEvent = { ...kill, data: { ...(kill.data as object), victim: 99 } };
    const r = await verifyEvent(forged, cache);
    expect(r.status).toBe("bad");
    expect(r.reason).toContain("differs");
    expect((await verifyEvent(kill, cache)).status).toBe("verified");
  });

  test("a tampered JWS payload fails the signature", async () => {
    const w = smallWorld();
    const { cache } = keysFor(w);
    const e = w.log.find((x) => x.type === "SignedIn")!;
    const [h, p, s] = e.jws!.split(".");
    const body = JSON.parse(atob(p!.replace(/-/g, "+").replace(/_/g, "/")));
    body.data.state = "away";
    const p2 = btoa(canonicalJson(body)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    const r = await verifyEvent({ ...e, data: body.data, jws: `${h}.${p2}.${s}` }, cache);
    expect(r.status).toBe("bad");
    expect(r.reason).toContain("signature");
  });

  test("signed by someone else's key is caught", async () => {
    const w = smallWorld();
    const { cache } = keysFor(w);
    const e = w.log.find((x) => x.type === "SignedIn")!;
    const mallory = newSigningKey();
    const body = JSON.parse(atob(e.jws!.split(".")[1]!.replace(/-/g, "+").replace(/_/g, "/")));
    // Mallory signs with her own key but claims the account's kid.
    const jws = signJws({ ...mallory, kid: e.kid! }, body);
    expect((await verifyEvent({ ...e, jws }, cache)).status).toBe("bad");
  });

  test("unknown kid and unsigned events", async () => {
    const w = smallWorld();
    const { cache } = keysFor(w);
    const e = w.log[0]!;
    const stranger = newSigningKey();
    const jws = signJws(stranger, { stream: e.stream, type: e.type, data: e.data });
    expect((await verifyEvent({ ...e, kid: stranger.kid, jws, author: "acct:0009ffffffffffff@x" }, cache)).status).toBe("unknown");
    expect((await verifyEvent({ ...e, jws: undefined }, cache)).status).toBe("unsigned");
  });

  test("batch JWS bodies match by jws_index", () => {
    const e: WireEvent = { position: 1, stream: "game-g", version: 1, type: "B", at: "t", data: { y: 2 }, jws_index: 1 };
    const batch = [{ type: "A", stream: "game-g", at: "t", data: { x: 1 } }, { type: "B", stream: "game-g", at: "t", data: { y: 2 } }];
    expect(bodyMatches(batch, e)).toBe(true);
    expect(bodyMatches(batch, { ...e, jws_index: 0 })).toBe(false);
    expect(bodyMatches(batch, { ...e, jws_index: undefined })).toBe(false);
    // A single body with a jws_index is not a batch member.
    expect(bodyMatches(batch[1], e)).toBe(false);
    // The wire `at` must equal the signed one.
    expect(bodyMatches(batch, { ...e, at: "other" })).toBe(false);
  });

  test("mock batches carry jws_index and verify; a wrong index is caught", async () => {
    const w = smallWorld();
    const { cache } = keysFor(w);
    const kill = w.log.find((e) => e.type === "PlayerKilled")!;
    const score = w.log.find((e) => e.position === kill.position + 1)!;
    expect(kill.jws_index).toBe(0);
    expect(score.jws_index).toBe(1);
    expect(score.jws).toBe(kill.jws);
    expect((await verifyEvent(score, cache)).status).toBe("verified");
    expect((await verifyEvent({ ...score, jws_index: 0 }, cache)).status).toBe("bad");
  });

  test("an account key outside its validity range is rejected", async () => {
    const w = smallWorld();
    const e = w.log.find((x) => x.type === "SignedIn")!;
    const a = w.accounts.get((e.data as { xuid: string }).xuid)!;
    const cache = new KeyCache({
      serverKeys: async () => [],
      accountKeys: async () => [{ ...w.accountKeys(a).keys[0]!, valid_to: e.position }],
    });
    const r = await verifyEvent(e, cache);
    expect(r.status).toBe("bad");
    expect(r.reason).toContain("not valid");
  });

  test("hash chain matches the mock's", async () => {
    const jws = "a.b.c";
    const h1 = chainHashSync(null, jws);
    expect(await chainHash(null, jws)).toBe(h1);
    expect(await chainHash(h1, "d.e.f")).toBe(chainHashSync(h1, "d.e.f"));
  });

  test("Verifier batches, flags a mock-tampered event and a broken chain", async () => {
    const w = new World();
    w.tamperEvery = 25;
    w.seed(30_000);
    const { cache } = keysFor(w);
    const out: VerifyResult[] = [];
    let batches = 0;
    const v = new Verifier(cache, (rs) => { batches++; out.push(...rs); }, 32, () => Promise.resolve());
    for (const e of w.log) v.enqueue(e);
    await v.idle();
    expect(out).toHaveLength(w.log.length);
    expect(batches).toBeGreaterThan(1);
    const bad = out.filter((r) => r.status === "bad").map((r) => r.position);
    const tampered = w.log.filter((e) => (e.data as { tampered?: boolean }).tampered).map((e) => e.position);
    expect(tampered.length).toBeGreaterThan(0);
    expect(bad).toEqual(tampered);

    // Drop the hash of one event: the chain check notices.
    const w2 = smallWorld();
    const { cache: c2 } = keysFor(w2);
    const res2: VerifyResult[] = [];
    const v2 = new Verifier(c2, (rs) => res2.push(...rs), 64, () => Promise.resolve());
    const victim = w2.log.find((e) => e.version === 2)!;
    for (const e of w2.log) v2.enqueue(e === victim ? { ...e, hash: "00".repeat(32) } : e);
    await v2.idle();
    expect(res2.find((r) => r.position === victim.position)!.reason).toBe("hash chain broken");
  });
});

describe("mock world through the reducer", () => {
  test("a simulated history builds sensible state", () => {
    const w = new World();
    w.seed(10 * 60_000);
    const s = initialState();
    applyEvents(s, w.log);
    expect(Object.keys(s.accounts).length).toBe(w.accounts.size);
    expect(Object.keys(s.games).length).toBe(w.games.size);
    expect(Object.values(s.games).some((g) => g.status === "ended")).toBe(true);
    for (const g of Object.values(s.games)) {
      const sim = w.games.get(g.game_id)!;
      expect(g.kills.length).toBe(sim.kills.length);
      expect(g.status).toBe(sim.ended_at ? "ended" : "live");
    }
  });
});

describe("snapshot plus stream equals full replay (mock world)", () => {
  test("joining by position gives the same state", () => {
    const w = new World();
    w.seed(6 * 60_000);
    // Snapshot from the mock's read models at position P...
    const P = w.log.length;
    const snap = initialState();
    applyAccountsSnapshot(snap, [...w.accounts.values()].map((a) => w.publicAccount(a)));
    applyPresenceSnapshot(snap, [...w.accounts.values()].filter((a) => a.online).map((a) => ({
      xuid: a.xuid, gamertag: a.gamertag, state: a.state, game_id: a.game_id ?? undefined, since: a.since,
    })));
    for (const g of w.games.values()) applyGameSnapshot(snap, w.gameDetail(g));
    snap.position = P;
    // ...then more happens, and the stream delivers events after P.
    for (let i = 0; i < 15 * 90; i++) { w.now += 1000 / 15; w.step(); }
    applyEvents(snap, w.log.slice(P));
    const full = initialState();
    applyEvents(full, w.log);
    for (const [xuid, a] of Object.entries(full.accounts)) {
      expect(snap.accounts[xuid]!.online).toBe(a.online);
      expect(snap.accounts[xuid]!.presence?.game_id ?? null).toBe(a.presence?.game_id ?? null);
    }
    expect(Object.keys(snap.games).sort()).toEqual(Object.keys(full.games).sort());
    for (const [id, g] of Object.entries(full.games)) {
      const sg = snap.games[id]!;
      expect(sg.status).toBe(g.status);
      expect(sg.winner).toEqual(g.winner);
      expect(sg.scores).toEqual(g.scores);
      expect(sg.teams).toEqual(g.teams);
      expect(sg.team_play).toBe(g.team_play);
    }
    expect(snap.gameOrder).toEqual(full.gameOrder);
  });
});
