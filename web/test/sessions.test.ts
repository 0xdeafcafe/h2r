import { describe, expect, test } from "bun:test";
import { gametype, keepSelection, kind, listed, mapName, openFor, parseSession, parseSessions, roomText, seats, sessionRow, sortSessions, visibilityText, type Session } from "../src/lib/sessions";
import { Sessions } from "../mock/sessions";
import { World } from "../mock/world";

/** A session as the server sends it (crates/live-proto/src/session.rs). */
const wire = (over: Record<string, unknown> = {}, ad: Record<string, unknown> = {}) => ({
  session_id: "00112233445566aa",
  host: "acct:0009000000000001@live.example",
  host_xuid: "0009000000000001",
  host_gamertag: "Sneaky Pete",
  xnkid: "00112233445566aa",
  endpoint: { addr: "203.0.113.7", port: 3074, source: "observed" },
  advertisement: {
    addr: null, port: 3074, host_address: [], attributes: {},
    slots: { public_filled: 3, public_open: 5, private_filled: 1, private_open: 1 },
    flags: "custom", hopper_id: null, map: "teacup", name: "Team Slayer", language: "en", ...ad,
  },
  since: "2026-11-16T10:00:00.000Z",
  last_activity: 1794823200,
  state: "open",
  players: { "0009000000000001": "public", "0009000000000002": "" },
  ...over,
});
const session = (over?: Record<string, unknown>, ad?: Record<string, unknown>): Session => parseSession(wire(over, ad))!;

describe("reading sessions", () => {
  test("takes the host, map, game and slots out of the server's shape", () => {
    const s = session();
    expect(s).toMatchObject({ session_id: "00112233445566aa", host_gamertag: "Sneaky Pete", host_xuid: "0009000000000001", state: "open", flags: "custom", map: "teacup", name: "Team Slayer" });
    expect(s.slots).toEqual({ public_filled: 3, public_open: 5, private_filled: 1, private_open: 1 });
    expect(Object.keys(s.players)).toEqual(["0009000000000001", "0009000000000002"]);
  });

  test("visibility reads as the server writes it; absent is public", () => {
    expect(session().visibility).toBe("public");
    expect(session({}, { visibility: "friends" }).visibility).toBe("friends");
    expect(session({}, { visibility: "invite" }).visibility).toBe("invite");
    expect(session({}, { visibility: "secret" }).visibility).toBe("public");
    expect(visibilityText(session({}, { visibility: "friends" }))).toBe("friends only");
    expect(visibilityText(session())).toBe("everyone");
    expect([session(), session({}, { visibility: "friends" }), session({}, { visibility: "invite" })].filter(listed).map((s) => s.visibility)).toEqual(["public", "friends"]);
  });

  test("an advertisement with nothing optional still reads", () => {
    const s = parseSession({ session_id: "a", host_gamertag: "Moss", advertisement: { slots: { public_filled: 1, public_open: 3 } } })!;
    expect(s).toMatchObject({ map: null, name: null, flags: "custom", hopper_id: null, since: null, state: "open", players: {} });
    expect(seats(s)).toEqual({ filled: 1, max: 4, open: 3 });
  });

  test("something without an id or a host is dropped, and the rest of the list kept", () => {
    expect(parseSession(null)).toBeNull();
    expect(parseSession({ session_id: "a" })).toBeNull();
    expect(parseSession({ host_gamertag: "Moss" })).toBeNull();
    expect(parseSessions([wire(), { nope: true }, "x"]).length).toBe(1);
    expect(parseSessions(undefined)).toEqual([]);
  });

  test("a dedicated session has a region label without a host gamertag", () => {
    const s = session({ host_gamertag: "", host_lease: "hl-example", dedicated_region: "lhr" });
    expect(s.host_gamertag).toBe("");
    expect(sessionRow(s).label).toBe("Dedicated (lhr)");
  });

  test("bad slot numbers count as none", () => {
    const s = session({}, { slots: { public_filled: -2, public_open: "4", private_filled: 2.9, private_open: NaN } });
    expect(s.slots).toEqual({ public_filled: 0, public_open: 0, private_filled: 2, private_open: 0 });
  });
});

describe("players against the most", () => {
  test("filled is public and private taken; max is every slot", () => {
    expect(seats(session())).toEqual({ filled: 4, max: 10, open: 6 });
  });

  test("a longer reported roster wins over the host's count, but not past the room", () => {
    const players = Object.fromEntries(Array.from({ length: 6 }, (_, i) => [`x${i}`, ""]));
    expect(seats(session({ players }, { slots: { public_filled: 1, public_open: 3, private_filled: 0, private_open: 0 } }))).toEqual({ filled: 4, max: 4, open: 0 });
    expect(seats(session({ players }))).toEqual({ filled: 6, max: 10, open: 4 });
  });

  test("says how much room there is, or that it is full or gone", () => {
    expect(roomText(session())).toBe("6 open");
    expect(roomText(session({}, { slots: { public_filled: 8, public_open: 0, private_filled: 0, private_open: 0 } }))).toBe("full");
    expect(roomText(session({ state: "deleted" }))).toBe("closed");
    expect(roomText(session({ state: "expired" }))).toBe("expired");
  });
});

describe("what a row says", () => {
  test("the host, then the map and players against the most", () => {
    expect(sessionRow(session())).toEqual({ key: "00112233445566aa", label: "Sneaky Pete", meta: "teacup 4/10" });
  });

  test("the game is the host's name for it, else the kind of session", () => {
    expect(gametype(session())).toBe("Team Slayer");
    expect(gametype(session({}, { name: null }))).toBe("custom game");
    expect(gametype(session({}, { name: "  ", flags: "hopper" }))).toBe("matchmaking");
    expect(kind(session({}, { flags: "ranked" }))).toBe("ranked match");
  });

  test("a map is named by its file, and an absent one says so", () => {
    expect(mapName(session({}, { map: "scenarios\\multi\\beaver_creek.map" }))).toBe("beaver creek");
    expect(mapName(session({}, { map: null }))).toBe("unknown map");
  });

  test("how long it has been open", () => {
    const t = Date.parse("2026-11-16T10:00:00.000Z");
    expect(openFor("2026-11-16T10:00:00.000Z", t + 20_000)).toBe("just opened");
    expect(openFor("2026-11-16T10:00:00.000Z", t + 4.5 * 60_000)).toBe("4 min");
    expect(openFor("2026-11-16T10:00:00.000Z", t + 2.2 * 3_600_000)).toBe("2 h");
    expect(openFor(null, t)).toBe("unknown");
    expect(openFor("not a time", t)).toBe("unknown");
  });
});

describe("list order and selection", () => {
  const a = session({ session_id: "a", host_gamertag: "Zed" }, { slots: { public_filled: 2, public_open: 6, private_filled: 0, private_open: 0 } });
  const b = session({ session_id: "b", host_gamertag: "Amy" }, { slots: { public_filled: 8, public_open: 0, private_filled: 0, private_open: 0 } });
  const c = session({ session_id: "c", host_gamertag: "Bo" }, { slots: { public_filled: 5, public_open: 3, private_filled: 0, private_open: 0 } });
  const d = session({ session_id: "d", host_gamertag: "Al" }, { slots: { public_filled: 2, public_open: 2, private_filled: 0, private_open: 0 } });

  test("games with room come first, fuller before emptier, then by host; full ones last", () => {
    expect(sortSessions([a, b, c, d]).map((s) => s.session_id)).toEqual(["c", "d", "a", "b"]);
  });

  test("a refresh keeps the selection on the same session, wherever it moved", () => {
    const rows = [{ key: "c" }, { key: "d" }, { key: "a" }];
    expect(keepSelection(rows, "a", 0)).toBe(2);
    // The selected session closed: stay on the same row, or the last one left.
    expect(keepSelection(rows, "gone", 1)).toBe(1);
    expect(keepSelection(rows, "gone", 9)).toBe(2);
    expect(keepSelection([], "gone", 3)).toBe(0);
    expect(keepSelection(rows, null, 1)).toBe(1);
  });
});

describe("mock sessions", () => {
  const get = (s: Sessions, path: string) => s.handle(new Request("http://mock" + path), new URL("http://mock" + path))!;
  const make = (n: number) => {
    const world = new World();
    world.seed(5 * 60_000);
    const s = new Sessions(world, n);
    s.seed();
    return s;
  };

  test("lists open sessions in the server's shape, ordered by id, and the web reads every one", () => {
    const s = make(5);
    const r = get(s, "/v1/sessions");
    const body = r.body as { items: Array<{ session_id: string }>; next: string | null; position: number };
    expect(r.status).toBe(200);
    expect(body.items.length).toBe(5);
    expect(body.next).toBeNull();
    expect(body.items.map((i) => i.session_id)).toEqual([...body.items.map((i) => i.session_id)].sort());
    const read = parseSessions(body.items);
    expect(read.length).toBe(5);
    for (const x of read) {
      expect(x.session_id).toMatch(/^[0-9a-f]{16}$/);
      const n = seats(x);
      expect(n.filled).toBeGreaterThanOrEqual(1);
      expect(n.max).toBeLessThanOrEqual(16);
      // The roster names no more players than the slots count, and the host is on it.
      expect(Object.keys(x.players).length).toBeLessThanOrEqual(n.filled);
      expect(x.players[x.host_xuid]).toBeDefined();
    }
  });

  test("pages with limit and cursor, filters by map, and refuses a bad limit", () => {
    const s = make(5);
    const first = get(s, "/v1/sessions?limit=2").body as { items: Array<{ session_id: string }>; next: string };
    expect(first.items.length).toBe(2);
    expect(first.next).toBe(first.items[1]!.session_id);
    const rest = get(s, `/v1/sessions?limit=100&cursor=${first.next}`).body as { items: unknown[]; next: string | null };
    expect(rest.items.length).toBe(3);
    expect(rest.next).toBeNull();
    const all = parseSessions((get(s, "/v1/sessions").body as { items: unknown[] }).items);
    const map = all[0]!.map!;
    const onMap = parseSessions((get(s, `/v1/sessions?map=${map}`).body as { items: unknown[] }).items);
    expect(onMap.length).toBe(all.filter((x) => x.map === map).length);
    expect(get(s, "/v1/sessions?limit=0").status).toBe(400);
  });

  test("one session by id; an unknown id is 404; nothing open is an empty list", () => {
    const s = make(3);
    const id = [...s.all.keys()][0]!;
    expect(parseSession((get(s, `/v1/sessions/${id}`).body as { session: unknown }).session)!.session_id).toBe(id);
    expect(get(s, "/v1/sessions/ffffffffffffffff").status).toBe(404);
    expect((get(make(0), "/v1/sessions").body as { items: unknown[] }).items).toEqual([]);
  });

  test("through many steps the slots stay sane, and a closed session leaves the list but stays readable", () => {
    const s = make(4);
    for (let i = 0; i < 400; i++) s.step();
    const open = parseSessions((get(s, "/v1/sessions").body as { items: unknown[] }).items);
    expect(open.length).toBeGreaterThan(0);
    for (const x of open) {
      expect(x.state).toBe("open");
      expect(seats(x).max).toBeLessThanOrEqual(16);
      expect(Object.keys(x.players).length).toBeLessThanOrEqual(x.slots.public_filled + x.slots.private_filled);
      expect(x.players[x.host_xuid]).toBeDefined();
    }
    // Nobody is in two open sessions at once.
    const everyone = open.flatMap((x) => Object.keys(x.players));
    expect(new Set(everyone).size).toBe(everyone.length);
    const closed = [...s.all.values()].find((x) => x.state !== "open");
    expect(closed).toBeDefined();
    expect(open.some((x) => x.session_id === closed!.session_id)).toBe(false);
    expect(parseSession((get(s, `/v1/sessions/${closed!.session_id}`).body as { session: unknown }).session)!.state).not.toBe("open");
  });
});
