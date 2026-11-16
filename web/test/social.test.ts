import { describe, expect, test } from "bun:test";
import {
  LEADER, MEMBER, OFFICER, PRIV, can, emptyFriends, friendStatus, friendWrites, normalizeFriends, normalizeTeam, randomTeamId,
  roleOf, sortedMembers, teamErrorText, teamPropsError, teamWrites, type Team,
} from "../src/api/social";
import { applyFriendEvent, applyTeamEvent, friendCounterpart, teamRecord } from "../src/state/social-model";
import { eventBody, postEvent, signEvent, TYP_EVENT, WriteError } from "../src/state/sign";
import { applyEvents, initialState } from "../src/state/reducer";
import type { LocalAccount } from "../src/state/account";
import { b64urlDecode, canonicalJson, parseJws } from "../src/state/verify";
import { ev, resetPos } from "./helpers";

const A = "0009000000000001", B = "0009000000000002", C = "0009000000000003";
const TAGS: Record<string, string> = { [A]: "Alpha", [B]: "Bravo", [C]: "Charlie" };
const tagOf = (x: string) => TAGS[x] ?? x;
const at = (n: number) => `2026-11-15T20:00:0${n}.000Z`;

describe("the friends document", () => {
  test("maps the server's document, dropping junk and sorting by gamertag", () => {
    const d = normalizeFriends({
      friends: [
        { xuid: B, gamertag: "bravo", friends_since: at(1), presence: { state: "playing", game_id: "g1", since: at(2) } },
        { xuid: A, gamertag: "Alpha", friends_since: at(1) },
        { gamertag: "no xuid" },
        "junk",
      ],
      requests_in: [{ xuid: C, gamertag: "Charlie", at: at(3) }],
    });
    expect(d.friends.map((f) => f.gamertag)).toEqual(["Alpha", "bravo"]);
    expect(d.friends[0]!.presence).toBeNull();
    expect(d.friends[1]!.presence).toEqual({ state: "playing", game_id: "g1", since: at(2) });
    expect(d.requests_in).toEqual([{ xuid: C, gamertag: "Charlie", at: at(3) }]);
    expect(d.requests_out).toEqual([]);
    expect(normalizeFriends(null)).toEqual(emptyFriends());
  });

  test("a friend status for each relation", () => {
    const d = normalizeFriends({ friends: [{ xuid: B, gamertag: "Bravo" }], requests_in: [{ xuid: C, gamertag: "Charlie" }] });
    expect(friendStatus(d, A, A)).toBe("self");
    expect(friendStatus(d, A, B)).toBe("friend");
    expect(friendStatus(d, A, C)).toBe("incoming");
    expect(friendStatus(normalizeFriends({ requests_out: [{ xuid: C }] }), A, C)).toBe("outgoing");
    expect(friendStatus(d, A, "0009000000000009")).toBe("none");
    expect(friendStatus(null, A, B)).toBe("none");
  });
});

describe("friend events fold into each side's document", () => {
  const fold = (me: string, evs: Array<[string, unknown]>) =>
    evs.reduce((d, [type, data], i) => applyFriendEvent(d, me, { type, data, at: at(i) }, tagOf), emptyFriends());

  test("request then accept: pending on both sides, then friends on both", () => {
    const req: [string, unknown] = ["FriendRequested", friendWrites.request(A, B).data];
    expect(fold(A, [req]).requests_out).toEqual([{ xuid: B, gamertag: "Bravo", at: at(0) }]);
    expect(fold(B, [req]).requests_in).toEqual([{ xuid: A, gamertag: "Alpha", at: at(0) }]);
    const acc: [string, unknown] = ["FriendAccepted", friendWrites.accept(B, A).data];
    for (const [me, other] of [[A, B], [B, A]] as const) {
      const d = fold(me, [req, acc]);
      expect(d.friends.map((f) => [f.xuid, f.friends_since])).toEqual([[other, at(1)]]);
      expect(d.requests_in.length + d.requests_out.length).toBe(0);
    }
  });

  test("decline and cancel clear the request; remove ends a friendship", () => {
    const req: [string, unknown] = ["FriendRequested", { from: A, to: B }];
    expect(fold(A, [req, ["FriendDeclined", friendWrites.decline(B, A).data]])).toEqual(emptyFriends());
    expect(fold(B, [req, ["FriendRemoved", friendWrites.remove(A, B).data]])).toEqual(emptyFriends());
    const friends = fold(B, [req, ["FriendAccepted", { from: A, to: B }]]);
    expect(applyFriendEvent(friends, B, { type: "FriendRemoved", data: { by: B, friend: A }, at: at(5) }, tagOf)).toEqual(emptyFriends());
  });

  test("an event between two others leaves the document alone", () => {
    const d = fold(C, [["FriendRequested", { from: A, to: B }]]);
    expect(d).toEqual(emptyFriends());
    expect(friendCounterpart(C, { type: "FriendRemoved", data: { by: A, friend: B } })).toBeNull();
  });
});

describe("team events fold into a team", () => {
  const ID = "000a5eed00000001";
  const by = (x: string) => `acct:${x}@h`;
  const step = (t: Team | null, author: string, w: { stream: string; type: string; data: unknown }, n: number) =>
    applyTeamEvent(t, { ...w, author: by(author), at: at(n) }, tagOf);

  function built(): Team {
    let t = step(null, A, teamWrites.create(ID, { name: "Clan", motto: "hi" }), 0);
    t = step(t, A, teamWrites.recruit(ID, { xuid: B, gamertag: "Bravo" }, OFFICER), 1);
    t = step(t, B, teamWrites.answer(ID, true), 2);
    t = step(t, B, teamWrites.recruit(ID, { xuid: C, gamertag: "Charlie" }, MEMBER), 3);
    return t!;
  }

  test("create, recruit, accept: a leader, an officer and a pending recruit", () => {
    const t = built();
    expect(t.creator).toBe(A);
    expect(t.created_at).toBe(at(0));
    expect(t.properties).toEqual({ name: "Clan", motto: "hi", description: "", url: "", data: [] });
    expect(Object.keys(t.members).sort()).toEqual([A, B]);
    expect(t.members[B]!.joined_at).toBe(at(2));
    expect(Object.keys(t.recruits)).toEqual([C]);
    expect([roleOf(t, A), roleOf(t, B), roleOf(t, C), roleOf(t, "0009000000000009")]).toEqual(["leader", "officer", "recruit", null]);
    expect(sortedMembers(t).map((m) => m.identity.gamertag)).toEqual(["Alpha", "Bravo"]);
    expect(can(t, A, PRIV.delete)).toBe(true);
    expect(can(t, B, PRIV.recruit)).toBe(true);
    expect(can(t, B, PRIV.delete)).toBe(false);
    expect(can(t, C, PRIV.recruit)).toBe(false);
  });

  test("decline, member changes, leaving and deletion", () => {
    const t = built();
    const declined = step(t, C, teamWrites.answer(ID, false), 4)!;
    expect(declined.recruits).toEqual({});
    const demoted = step(t, A, teamWrites.member(ID, B, { privileges: MEMBER, data: [] }), 5)!;
    expect(roleOf(demoted, B)).toBe("member");
    const left = step(demoted, B, teamWrites.remove(ID, B), 6)!;
    expect(Object.keys(left.members)).toEqual([A]);
    const gone = step(left, A, teamWrites.delete(ID), 7)!;
    expect(gone.deleted).toBe(true);
    // Folds never touch the team they were given.
    expect(Object.keys(t.recruits)).toEqual([C]);
    expect(roleOf(t, B)).toBe("officer");
  });

  test("events that don't apply change nothing", () => {
    const t = built();
    expect(step(t, A, teamWrites.answer(ID, true), 8)).toBe(t);
    expect(step(t, A, teamWrites.member(ID, C, { privileges: 1, data: [] }), 8)).toBe(t);
    expect(step(null, A, teamWrites.delete(ID), 8)).toBeNull();
  });

  test("a team read off the wire", () => {
    const t = normalizeTeam({
      id: ID.toUpperCase(), creator: A, properties: { name: "Clan" }, max_members: 100, created_at: at(0), deleted: false,
      members: { [A]: { identity: { xuid: A, gamertag: "Alpha" }, properties: { privileges: LEADER }, joined_at: at(0) } }, recruits: {},
    })!;
    expect(t.id).toBe(ID);
    expect(t.properties.description).toBe("");
    expect(t.members[A]!.properties).toEqual({ privileges: LEADER, data: [] });
    expect(normalizeTeam({ name: "no id" })).toBeNull();
  });
});

describe("team writes", () => {
  test("payloads match live-proto's team Action (operation tag, snake case)", () => {
    const id = "000a000000000001";
    expect(teamWrites.create(id, { name: "Clan" })).toEqual({
      stream: `team-${id}`, type: "TeamCreated",
      data: { operation: "create", id, properties: { name: "Clan", description: "", motto: "", url: "", data: [] }, member: { privileges: 0xffffffff, data: [] }, max_members: 100 },
    });
    expect(teamWrites.answer(id, false)).toEqual({ stream: `team-${id}`, type: "TeamRecruitDeclined", data: { operation: "answer", id, accept: false } });
    expect(teamWrites.member(id, B, { privileges: 0x1c, data: [] }).data).toEqual({ operation: "member_properties", id, user: B, properties: { privileges: 0x1c, data: [] } });
    expect(teamWrites.remove(id, B).type).toBe("TeamMemberRemoved");
    expect(friendWrites.accept(B, A)).toEqual({ stream: `account-${B}`, type: "FriendAccepted", data: { from: A, to: B } });
    expect(friendWrites.remove(A, B)).toEqual({ stream: `account-${A}`, type: "FriendRemoved", data: { by: A, friend: B } });
  });

  test("ids are sixteen hex digits under 000a", () => {
    expect(randomTeamId(() => new Uint8Array([0xde, 0xad, 0xbe, 0xef, 0x00, 0x01]))).toBe("000adeadbeef0001");
    expect(randomTeamId()).toMatch(/^000a[0-9a-f]{12}$/);
  });

  test("name and text limits", () => {
    const p = { name: "Clan", description: "", motto: "", url: "" };
    expect(teamPropsError(p)).toBeNull();
    expect(teamPropsError({ ...p, name: "  " })).toMatch(/name/);
    expect(teamPropsError({ ...p, name: "x".repeat(16) })).toMatch(/15/);
    expect(teamPropsError({ ...p, motto: "x".repeat(256) })).toMatch(/motto/);
    expect(teamErrorText("NoAdmin", "x")).toMatch(/leader/);
    expect(teamErrorText(undefined, "fallback")).toBe("fallback");
  });
});

describe("signed event writes", () => {
  // RFC 8037's Ed25519 test key.
  const me: LocalAccount = {
    xuid: A, gamertag: "Alpha", id: `acct:${A}@localhost`, server: "http://localhost", server_id: "localhost",
    kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k",
    key: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A", x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
  };
  const verifyWith = async (jws: string) => {
    const p = parseJws(jws)!;
    const key = await crypto.subtle.importKey("jwk", { kty: "OKP", crv: "Ed25519", x: me.x }, { name: "Ed25519" }, false, ["verify"]);
    return { p, ok: await crypto.subtle.verify({ name: "Ed25519" }, key, p.signature, p.signingInput) };
  };

  test("the body is live-proto's EventBody, signed as h2live-event over canonical JSON", async () => {
    const body = eventBody(me, friendWrites.request(A, B), Date.UTC(2026, 10, 15, 20, 0, 0, 120), "n0nce");
    expect(body).toEqual({ stream: `account-${A}`, type: "FriendRequested", data: { from: A, to: B }, author: me.id, kid: me.kid, at: "2026-11-15T20:00:00.120Z", nonce: "n0nce" });
    const jws = await signEvent(me, body);
    const { p, ok } = await verifyWith(jws);
    expect(ok).toBe(true);
    expect(p.header as unknown).toEqual({ alg: "EdDSA", kid: me.kid, typ: TYP_EVENT });
    expect(new TextDecoder().decode(b64urlDecode(jws.split(".")[1]!))).toBe(canonicalJson(body));
  });

  test("posts the JWS as application/jose with an events.write token", async () => {
    let seen: { url: string; init: RequestInit } | null = null;
    const fetchFn = (async (url: string, init: RequestInit) => {
      seen = { url, init };
      return new Response(JSON.stringify({ position: 42 }), { status: 201 });
    }) as unknown as typeof fetch;
    const pos = await postEvent("http://localhost/v1", "localhost", me, teamWrites.answer("000a000000000001", true), fetchFn, Date.now());
    expect(pos).toBe(42);
    const s = seen as unknown as { url: string; init: RequestInit };
    expect(s.url).toBe("http://localhost/v1/events");
    const h = s.init.headers as Record<string, string>;
    expect(h["content-type"]).toBe("application/jose");
    const token = await verifyWith(h.authorization!.replace("Bearer ", ""));
    expect(token.ok).toBe(true);
    expect(token.p.payload).toMatchObject({ iss: me.id, aud: "localhost", lxm: "events.write" });
    const body = await verifyWith(String(s.init.body));
    expect(body.ok).toBe(true);
    expect(body.p.payload).toMatchObject({ stream: "team-000a000000000001", type: "TeamRecruitAccepted", author: me.id });
  });

  test("a refused write carries the team error", async () => {
    const fetchFn = (async () => new Response(JSON.stringify({ error: "conflict", message: "name taken", team_error: "AlreadyExists" }), { status: 409 })) as unknown as typeof fetch;
    const e = await postEvent("http://localhost/v1", "localhost", me, teamWrites.create("000a000000000001", { name: "Clan" }), fetchFn).catch((x) => x);
    expect(e).toBeInstanceOf(WriteError);
    expect((e as WriteError).status).toBe(409);
    expect((e as WriteError).teamError).toBe("AlreadyExists");
  });
});

describe("a team's combined record", () => {
  function game(id: string, teamPlay: boolean, players: Array<[number, string, number]>, scores: Array<[number, number, number]>, winner: unknown) {
    return [
      ev("GameStarted", `game-${id}`, { game_id: id, map: "lockout", mode: "multiplayer", team_play: teamPlay, players: players.map(([index, xuid, team]) => ({ index, name: tagOf(xuid), xuid, team })) }),
      ev("GameEnded", `game-${id}`, { tick: 100, reason: "finished", winner, players: scores.map(([index, kills, deaths]) => ({ index, score: kills, kills, deaths, assists: 0 })) }),
    ];
  }

  test("counts games two members played on one side, with their numbers and wins", () => {
    resetPos();
    const s = initialState();
    applyEvents(s, [
      // Together on team 0, which wins.
      ...game("g1", true, [[0, A, 0], [1, B, 0], [2, C, 1]], [[0, 5, 1], [1, 3, 2], [2, 2, 8]], { team: 0 }),
      // On opposite teams: not together.
      ...game("g2", true, [[0, A, 0], [1, B, 1]], [[0, 1, 1], [1, 1, 1]], { team: 1 }),
      // Free for all with both in it: together, B wins.
      ...game("g3", false, [[0, A, 0], [1, B, 0]], [[0, 2, 3], [1, 3, 2]], { player: 1 }),
      // Only one member.
      ...game("g4", false, [[0, A, 0], [1, C, 0]], [[0, 9, 0], [1, 0, 9]], { player: 0 }),
    ]);
    const r = teamRecord(s, [A, B]);
    expect(r.games.map((g) => g.game.game_id)).toEqual(["g3", "g1"]);
    expect(r.together).toBe(2);
    expect(r.wins).toBe(2);
    expect(r.kills).toBe(5 + 3 + 2 + 3);
    expect(r.deaths).toBe(1 + 2 + 3 + 2);
    expect(r.games[1]!.slots).toEqual([0, 1]);
  });
});
