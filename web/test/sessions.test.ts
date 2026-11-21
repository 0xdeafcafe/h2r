import { describe, expect, test } from "bun:test";
import { gametype, kind, listed, mapName, parseSession, parseSessions, seats, sortSessions, type Session } from "../src/lib/sessions";
import { readSessionFilters, sessionFiltersParam, type SessionFilters } from "../src/lib/sessionFilters";
import { COOP_LEVELS, Sessions } from "../mock/sessions";
import { World } from "../mock/world";

/** A session as the server sends it (crates/live/live-proto/src/session.rs). */
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

  test("the advertised variant can name the game when the host has no separate title", () => {
    const s = session({}, { name: null, browser: { game_type: 2, variant: "Rockets & Swords" } });
    expect(gametype(s)).toBe("Rockets & Swords");
  });

  test("visibility reads as the server writes it; absent is public", () => {
    expect(session().visibility).toBe("public");
    expect(session({}, { visibility: "friends" }).visibility).toBe("friends");
    expect(session({}, { visibility: "invite" }).visibility).toBe("invite");
    expect(session({}, { visibility: "secret" }).visibility).toBe("public");
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

  test("a dedicated session reads its lease and region without a host gamertag", () => {
    const s = session({ host_gamertag: "", host_lease: "hl-example", dedicated_region: "lhr" });
    expect(s.host_gamertag).toBe("");
    expect(s).toMatchObject({ host_lease: "hl-example", dedicated_region: "lhr" });
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
});

describe("what a row says", () => {
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
});

describe("list order", () => {
  const a = session({ session_id: "a", host_gamertag: "Zed" }, { slots: { public_filled: 2, public_open: 6, private_filled: 0, private_open: 0 } });
  const b = session({ session_id: "b", host_gamertag: "Amy" }, { slots: { public_filled: 8, public_open: 0, private_filled: 0, private_open: 0 } });
  const c = session({ session_id: "c", host_gamertag: "Bo" }, { slots: { public_filled: 5, public_open: 3, private_filled: 0, private_open: 0 } });
  const d = session({ session_id: "d", host_gamertag: "Al" }, { slots: { public_filled: 2, public_open: 2, private_filled: 0, private_open: 0 } });

  test("games with room come first, fuller before emptier, then by host; full ones last", () => {
    expect(sortSessions([a, b, c, d]).map((s) => s.session_id)).toEqual(["c", "d", "a", "b"]);
  });
});

describe("saved session filters", () => {
  test("normalization preserves false and zero, and ignores obsolete values", () => {
    const filters = readSessionFilters({
      kind: "coop", clankers: false, campaign: "halo_ce", difficulty: 0,
      has_space: false, game_type: 99, map_category: "retail", variant: "   ", expired_key: "obsolete",
    });
    expect(JSON.parse(sessionFiltersParam(filters)!)).toEqual({
      kind: "coop", clankers: false, campaign: "halo_ce", difficulty: 0,
    });
    expect(sessionFiltersParam(readSessionFilters(null))).toBeUndefined();
  });

  test("the variant uses the full trimmed title and the UTF-8 size limit", () => {
    expect(JSON.parse(sessionFiltersParam({ variant: "    Rockets & Swords    " })!)).toEqual({ variant: "Rockets & Swords" });
    expect(sessionFiltersParam({ variant: "é".repeat(128) })).toBeDefined();
    expect(sessionFiltersParam({ variant: "é".repeat(129) })).toBeUndefined();
    expect(sessionFiltersParam({ variant: "Team\0Slayer" })).toBeUndefined();
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

  test("filters before paging, retains old unknown metadata, and provides visible variants", () => {
    const s = make(4);
    const ids = [...s.all.keys()].sort();
    const known = (i: number) => s.all.get(ids[i]!)!.advertisement;
    for (const id of ids) {
      const ad = s.all.get(id)!.advertisement;
      ad.slots = { public_open: 2, public_filled: 1, private_open: 0, private_filled: 0 };
      ad.flags = "custom";
      ad.visibility = "public";
      ad.map = null;
      delete ad.campaign;
      delete ad.browser;
    }
    known(0).browser = { clankers: false, game_type: 1, variant: "Other Variant", map_category: "halo2" };
    known(1).browser = { clankers: true, game_type: 2, variant: "Rockets & Swords", map_category: "map_pack" };
    known(3).flags = "coop";
    const [level, name] = COOP_LEVELS[0]!;
    known(3).campaign = { level, name, difficulty: 0 };
    known(3).browser = { clankers: false, map_category: "halo_ce" };
    const list = (filters: SessionFilters, cursor?: string) => {
      const q = new URLSearchParams({ limit: "1", filters: sessionFiltersParam(filters)!, browser_variants: "true" });
      if (cursor) q.set("cursor", cursor);
      return get(s, "/v1/sessions?" + q).body as { items: Array<{ session_id: string }>; next: string | null; variants?: string[] };
    };

    const filters = { kind: "multiplayer", clankers: true, game_type: 2, variant: "ROCKETS & SWORDS", map_category: "map_pack" } as const;
    const first = list(filters);
    expect(first.items.map((item) => item.session_id)).toEqual([ids[1]!]);
    expect(first.next).toBe(ids[1]!);
    expect(first.variants).toEqual(["Other Variant", "Rockets & Swords"]);
    const second = list(filters, first.next!);
    expect(second.items.map((item) => item.session_id)).toEqual([ids[2]!]);
    expect(second.next).toBeNull();

    expect(list({ kind: "coop", campaign: "halo_ce", difficulty: 0, has_space: true }).items.map((item) => item.session_id)).toEqual([ids[3]!]);
    known(3).slots.public_open = 0;
    known(3).slots.private_open = 2;
    expect(list({ kind: "coop", has_space: true }).items).toEqual([]);
    expect(get(s, "/v1/sessions?filters=broken").status).toBe(400);
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
