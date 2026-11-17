import { describe, expect, test } from "bun:test";
import { campaignLine, coopPlayers, difficultyName, gametype, isCoop, kind, levelName, parseSession, parseSessions, pingText, roomText, sessionRow } from "../src/lib/sessions";
import { coopDifficulty, coopLevel, coopMinutes, coopNames, newestCoop, parseCoopGame, parseCoopGames } from "../src/lib/coop";
import { normalizeFriends } from "../src/api/social";
import { applyEvent, applyPresenceSnapshot, initialState } from "../src/state/reducer";
import { ev } from "./helpers";
import { Sessions } from "../mock/sessions";
import { World } from "../mock/world";

const HOST = "0009000000000001", GUEST = "0009000000000002";

const wire = (campaign: unknown = { level: "03a_oldmombasa", name: "Outskirts", difficulty: 2 }, over: Record<string, unknown> = {}) => ({
  session_id: "00112233445566aa", host_xuid: HOST, host_gamertag: "Sneaky Pete",
  advertisement: { slots: { public_filled: 2, public_open: 0 }, flags: "coop", campaign, visibility: "friends" },
  since: "2026-11-16T10:00:00.000Z", state: "open", players: { [GUEST]: "public" }, ...over,
});

describe("co-op sessions", () => {
  test("a co-op session reads its campaign and says so", () => {
    const s = parseSession(wire())!;
    expect(s.flags).toBe("coop");
    expect(s.campaign).toEqual({ level: "03a_oldmombasa", name: "Outskirts", difficulty: 2 });
    expect(kind(s)).toBe("co-op campaign");
    expect(gametype(s)).toBe("Co-op campaign");
    expect(campaignLine(s)).toBe("Outskirts, heroic");
    expect(sessionRow(s).meta).toBe("co-op Outskirts 2/2");
    expect(roomText(s)).toBe("full");
  });

  test("the level falls back to its stem without the ordering prefix", () => {
    expect(levelName({ level: "03a_oldmombasa", name: null })).toBe("oldmombasa");
    expect(levelName({ level: "01_the_heretic", name: null })).toBe("the heretic");
    expect(levelName({ level: "mombasa", name: null })).toBe("mombasa");
    expect(parseSession(wire({ level: "03a_oldmombasa", difficulty: 0 }))!.campaign).toEqual({ level: "03a_oldmombasa", name: null, difficulty: 0 });
  });

  test("no campaign, junk campaign, or another kind of game reads as none", () => {
    expect(parseSession(wire(undefined, {}))!.campaign).not.toBeNull();
    expect(parseSession(wire(null))!.campaign).toBeNull();
    expect(parseSession(wire({ level: " " }))!.campaign).toBeNull();
    const custom = parseSession({ ...wire(), advertisement: { slots: {}, flags: "custom" } })!;
    expect(campaignLine(custom)).toBeNull();
    expect(kind(custom)).toBe("custom game");
  });

  test("difficulties by number", () => {
    expect(["easy", "normal", "heroic", "legendary"].map((_, i) => difficultyName(i))).toEqual(["easy", "normal", "heroic", "legendary"]);
    expect(difficultyName(9)).toBe("unknown");
  });

  test("the host's ping shows in the row and as text, and nothing when absent", () => {
    const s = parseSession(wire(undefined, { advertisement: { ...wire().advertisement, ping_ms: 42 } }))!;
    expect(s.ping_ms).toBe(42);
    expect(pingText(s)).toBe("42 ms");
    expect(sessionRow(s).meta).toBe("co-op Outskirts 2/2 42 ms");
    const none = parseSession(wire())!;
    expect(none.ping_ms).toBeNull();
    expect(pingText(none)).toBeNull();
    expect(sessionRow(none).meta).toBe("co-op Outskirts 2/2");
    // 0 clears it on the server; junk reads as none.
    for (const v of [0, -5, "42", null]) expect(parseSession(wire(undefined, { advertisement: { ...wire().advertisement, ping_ms: v } }))!.ping_ms).toBeNull();
  });

  test("players are the host, then who joined, by gamertag", () => {
    const s = parseSession(wire(undefined, { players: { [GUEST]: "public", [HOST]: "public", "0009000000000003": "" } }))!;
    expect(coopPlayers(s, (x) => (x === GUEST ? "Moss" : undefined))).toEqual(["Sneaky Pete", "Moss", "guest"]);
  });
});

const game = (over: Record<string, unknown> = {}) => ({
  session_id: "00112233445566aa",
  host: { xuid: HOST, gamertag: "Sneaky Pete" },
  players: [{ xuid: HOST, gamertag: "Sneaky Pete" }, { xuid: GUEST, gamertag: "Moss" }],
  campaign: { level: "03a_oldmombasa", name: "Outskirts", difficulty: 3 },
  started: "2026-11-16T10:00:00Z", ended: "2026-11-16T10:42:10Z", end_reason: "closed", ...over,
});

describe("finished co-op games", () => {
  test("reads the server's item", () => {
    const g = parseCoopGame(game())!;
    expect(coopLevel(g)).toBe("Outskirts");
    expect(coopDifficulty(g)).toBe("legendary");
    expect(coopNames(g)).toEqual(["Sneaky Pete", "Moss"]);
    expect(coopMinutes(g)).toBe(42);
    expect(g.end_reason).toBe("closed");
  });

  test("the session's visibility is read, public when a server doesn't say", () => {
    expect(parseCoopGame(game({ visibility: "friends" }))!.visibility).toBe("friends");
    expect(parseCoopGame(game({ visibility: "invite" }))!.visibility).toBe("invite");
    expect(parseCoopGame(game())!.visibility).toBe("public");
    expect(parseCoopGame(game({ visibility: "weird" }))!.visibility).toBe("public");
  });

  test("an account the server forgot reads as a guest; no roster is just the host", () => {
    expect(coopNames(parseCoopGame(game({ players: [{ xuid: HOST, gamertag: null }, { xuid: GUEST, gamertag: null }] }))!)).toEqual(["guest", "guest"]);
    expect(parseCoopGame(game({ players: undefined }))!.players.map((p) => p.xuid)).toEqual([HOST]);
  });

  test("junk items are dropped, and the rest kept", () => {
    expect(parseCoopGame(null)).toBeNull();
    expect(parseCoopGame(game({ campaign: null }))).toBeNull();
    expect(parseCoopGame(game({ ended: undefined }))).toBeNull();
    expect(parseCoopGames([game(), { nope: 1 }, "x"]).length).toBe(1);
    expect(parseCoopGames(undefined)).toEqual([]);
  });

  test("length is at least a minute, and unknown for backwards times", () => {
    expect(coopMinutes({ started: "2026-11-16T10:00:00Z", ended: "2026-11-16T10:00:05Z" })).toBe(1);
    expect(coopMinutes({ started: "2026-11-16T10:10:00Z", ended: "2026-11-16T10:00:00Z" })).toBeNull();
    expect(coopMinutes({ started: "x", ended: "y" })).toBeNull();
  });

  test("newest first by the end time", () => {
    const a = parseCoopGame(game({ session_id: "aa", ended: "2026-11-16T09:00:00Z" }))!;
    const b = parseCoopGame(game({ session_id: "bb", ended: "2026-11-16T11:00:00Z" }))!;
    expect(newestCoop([a, b]).map((g) => g.session_id)).toEqual(["bb", "aa"]);
  });
});

describe("presence activity", () => {
  test("friends carry it, and an empty one is none", () => {
    const d = normalizeFriends({ friends: [{ xuid: HOST, gamertag: "Pete", friends_since: "x", presence: { state: "online", activity: "Co-op: Outskirts" } }, { xuid: GUEST, gamertag: "Moss", friends_since: "x", presence: { state: "online", activity: "" } }] });
    const [moss, pete] = d.friends; // sorted by gamertag
    expect(moss!.presence!.activity).toBeUndefined();
    expect(pete!.presence!.activity).toBe("Co-op: Outskirts");
  });

  test("the reducer keeps it and a change without it clears it", () => {
    const s = initialState();
    applyPresenceSnapshot(s, [{ xuid: HOST, gamertag: "Pete", state: "online", activity: "Co-op: Outskirts", since: "2026-11-16T10:00:00Z" }]);
    expect(s.accounts[HOST]!.presence!.activity).toBe("Co-op: Outskirts");
    applyEvent(s, ev("PresenceChanged", `presence-${HOST}`, { xuid: HOST, state: "online" }, "2026-11-16T10:05:00Z"));
    expect(s.accounts[HOST]!.presence!.activity).toBeNull();
    applyEvent(s, ev("PresenceChanged", `presence-${HOST}`, { xuid: HOST, state: "online", activity: "Co-op: Outskirts" }, "2026-11-16T10:06:00Z"));
    expect(s.accounts[HOST]!.presence!.activity).toBe("Co-op: Outskirts");
  });
});

describe("mock co-op", () => {
  const get = (s: Sessions, path: string) => s.handle(new Request("http://mock" + path), new URL("http://mock" + path))!;
  const make = () => {
    const world = new World();
    world.seed(5 * 60_000);
    const s = new Sessions(world, 5);
    s.seed();
    return s;
  };

  test("lists open co-op sessions with a campaign, friends only, and a ping once a guest is in", () => {
    const s = make();
    const coop = parseSessions((get(s, "/v1/sessions?flags=coop").body as { items: unknown[] }).items);
    expect(coop.length).toBeGreaterThanOrEqual(2);
    for (const c of coop) {
      expect(isCoop(c)).toBe(true);
      expect(c.state).toBe("open");
      expect(c.campaign?.name).toBeTruthy();
      expect(c.visibility).toBe("friends");
      expect(Object.keys(c.players).length > 1).toBe(c.ping_ms !== null);
    }
    expect(coop.some((c) => c.ping_ms !== null && pingText(c)!.endsWith(" ms"))).toBe(true);
    // The plain list carries them too, beside other kinds.
    const all = parseSessions((get(s, "/v1/sessions").body as { items: unknown[] }).items);
    expect(all.filter(isCoop).length).toBe(coop.length);
    expect(all.some((x) => !isCoop(x))).toBe(true);
  });

  test("has finished co-op games, newest first, the web reads every one, and a player's own", () => {
    const s = make();
    const body = get(s, "/v1/coop/games").body as { items: unknown[]; next: string | null };
    const games = parseCoopGames(body.items);
    expect(games.length).toBeGreaterThanOrEqual(6);
    expect(games.length).toBe(body.items.length);
    expect(newestCoop(games).map((g) => g.session_id)).toEqual(games.map((g) => g.session_id));
    expect(games.some((g) => g.visibility === "friends")).toBe(true);
    for (const g of games) expect(g.players.length).toBeGreaterThanOrEqual(2);
    const who = games[0]!.players[1]!.gamertag!;
    const mine = parseCoopGames((get(s, `/v1/coop/games?player=${encodeURIComponent(who.toUpperCase())}`).body as { items: unknown[] }).items);
    expect(mine.length).toBeGreaterThanOrEqual(1);
    for (const g of mine) expect(g.players.some((p) => p.gamertag === who)).toBe(true);
    const page = get(s, "/v1/coop/games?limit=2").body as { items: unknown[]; next: string };
    expect(page.items.length).toBe(2);
    const next = get(s, `/v1/coop/games?limit=2&cursor=${page.next}`).body as { items: unknown[] };
    expect(parseCoopGames(next.items)[0]!.session_id).toBe(games[2]!.session_id);
  });

  test("a co-op session a guest joined becomes a finished game when it closes", () => {
    const s = make();
    const before = s.coopGames.length;
    for (let i = 0; i < 400 && s.coopGames.length === before; i++) s.step();
    expect(s.coopGames.length).toBe(before + 1);
    const g = parseCoopGame(s.coopGames[0])!;
    expect(g.players.length).toBeGreaterThanOrEqual(2);
    expect(g.visibility).toBe("friends");
  });
});
