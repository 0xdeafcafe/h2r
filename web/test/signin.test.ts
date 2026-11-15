import { describe, expect, test } from "bun:test";
import { GONE, clearOnce, redeem, takeOnce } from "../src/signin/redeem";

const RFC = {
  d: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A",
  x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
  kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k",
};
const TOKEN = "q3Jm2oZ0kX8vYb1sT4uW6g" + "Hn9cR5eLp2dF7aQ0zK1mXw";
const SERVER = { base: "http://127.0.0.1:50123", api: "http://127.0.0.1:50123/v1", serverId: "localhost" };

// What `h2 play --live` offers (live_admin::export_entry).
const ENTRY = {
  xuid: "0009000000000042",
  gamertag: "afr",
  id: "acct:0009000000000042@localhost",
  server: SERVER.base,
  server_id: "localhost",
  kid: RFC.kid,
  key: RFC.d,
  x: RFC.x,
};

describe("the link's fragment", () => {
  test("takes the token from #once=", () => {
    expect(takeOnce(`#once=${TOKEN}`)).toBe(TOKEN);
    expect(takeOnce(`once=${TOKEN}`)).toBe(TOKEN);
    expect(takeOnce("")).toBeNull();
    expect(takeOnce("#other=1")).toBeNull();
    expect(takeOnce("#once=")).toBeNull();
    expect(takeOnce("#once=not%20base64url!")).toBeNull();
  });

  test("is cleared from the address bar and history, path and query kept", () => {
    const calls: Array<[unknown, string, string]> = [];
    const history = {
      state: { k: 1 },
      replaceState(state: unknown, title: string, url: string) { calls.push([state, title, url]); },
    } as unknown as History;
    const once = clearOnce({ hash: `#once=${TOKEN}`, pathname: "/signin", search: "?server=x" }, history);
    expect(once).toBe(TOKEN);
    expect(calls).toEqual([[{ k: 1 }, "", "/signin?server=x"]]);
    // A bad fragment is cleared too; nothing to redeem.
    calls.length = 0;
    expect(clearOnce({ hash: "#once=", pathname: "/signin", search: "" }, history)).toBeNull();
    expect(calls.length).toBe(1);
    // No fragment: the address is left alone.
    calls.length = 0;
    expect(clearOnce({ hash: "", pathname: "/signin", search: "" }, history)).toBeNull();
    expect(calls.length).toBe(0);
  });
});

type Route = (init?: RequestInit) => Response;

function server(routes: Record<string, Route>) {
  const seen: Array<{ url: string; init?: RequestInit }> = [];
  const fetchFn = (async (input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    seen.push({ url, init });
    const route = routes[url];
    return route ? route(init) : new Response("{}", { status: 404 });
  }) as typeof fetch;
  return { fetchFn, seen };
}

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

function liveServer(opts: { admin?: boolean; keys?: unknown[] } = {}) {
  let offered = true;
  return server({
    [`${SERVER.api}/local/signin`]: (init) => {
      const body = JSON.parse(String(init?.body));
      if (!offered || body.once !== TOKEN) return json({ error: "not_found" }, 404);
      offered = false;
      return json(ENTRY);
    },
    [`${SERVER.api}/accounts/by-xuid/${ENTRY.xuid}/keys`]: () =>
      json({ keys: opts.keys ?? [{ kid: RFC.kid, jwk: { x: RFC.x }, valid_to: null }] }),
    [`${SERVER.api}/accounts/afr`]: () => json({ gamertag: "afr", admin: opts.admin ?? false }),
  });
}

describe("redeeming the link", () => {
  test("posts the token as JSON and returns the entry, admin known", async () => {
    const s = liveServer({ admin: true });
    const r = await redeem(TOKEN, SERVER, s.fetchFn);
    expect(r).toEqual({ ok: true, admin: true, account: ENTRY });
    const post = s.seen[0]!;
    expect(post.url).toBe(`${SERVER.api}/local/signin`);
    expect(post.init?.method).toBe("POST");
    expect((post.init?.headers as Record<string, string>)["content-type"]).toBe("application/json");
    expect(JSON.parse(String(post.init?.body))).toEqual({ once: TOKEN });
    expect(post.init?.cache).toBe("no-store");
  });

  test("a second use, or a wrong token, says to run h2 play --live again", async () => {
    const s = liveServer();
    expect((await redeem(TOKEN, SERVER, s.fetchFn)).ok).toBe(true);
    const again = await redeem(TOKEN, SERVER, s.fetchFn);
    expect(again).toEqual({ ok: false, kind: "gone", error: GONE });
    expect(GONE).toContain("h2 play --live again");
    expect((await redeem("x".repeat(44), SERVER, liveServer().fetchFn)).ok).toBe(false);
  });

  test("a non-admin goes to the dashboard; a stale key is refused", async () => {
    const r = await redeem(TOKEN, SERVER, liveServer().fetchFn);
    expect(r.ok && r.admin).toBe(false);
    const stale = await redeem(TOKEN, SERVER, liveServer({ keys: [{ kid: RFC.kid, jwk: { x: RFC.x }, valid_to: 1 }] }).fetchFn);
    expect(stale).toEqual({ ok: false, kind: "failed", error: "afr's key is not current on this server" });
  });

  test("another server's account is refused", async () => {
    const r = await redeem(TOKEN, { ...SERVER, serverId: "elsewhere" }, liveServer().fetchFn);
    expect(r.ok).toBe(false);
    expect(!r.ok && r.error).toContain("not this one");
  });

  test("no server answering is a plain error", async () => {
    const down = (async () => { throw new Error("connection refused"); }) as unknown as typeof fetch;
    const r = await redeem(TOKEN, SERVER, down);
    expect(r).toEqual({ ok: false, kind: "failed", error: "the live server did not answer (connection refused)" });
  });
});
