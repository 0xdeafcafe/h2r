// The split between the two admin areas, the dashboard's blades and the
// account import that moved to the dashboard.
import { describe, expect, test } from "bun:test";
import { ADMIN_TABS, IMPORT_ACCOUNT, SERVER_ADMIN, adminTab } from "../src/admin/tabs";
import { importAccounts } from "../src/admin/accountImport";
import { bladeAction, firstBlade, nextBlade, type Blade } from "../src/dash/bladeNav";
import { importSeed, type LocalAccount } from "../src/state/account";
import { sideOf } from "../src/dash/cross";

const RFC = {
  d: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A",
  x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
};

describe("the stats admin keeps only Halo's things", () => {
  test("message of the day, playlists and custom content; no service tabs", () => {
    expect(ADMIN_TABS.map((t) => t.id)).toEqual(["motd", "hopper", "content"]);
    expect(ADMIN_TABS.some((t) => /import|account|invite|server/i.test(t.label))).toBe(false);
  });

  test("an old ?tab=import link falls back to the first tab", () => {
    expect(adminTab("hopper")).toBe("hopper");
    expect(adminTab("content")).toBe("content");
    expect(adminTab("import")).toBe("motd");
    expect(adminTab(null)).toBe("motd");
  });

  test("its links for service work go to the dashboard", () => {
    expect(sideOf(SERVER_ADMIN)).toBe("dash");
    expect(sideOf(IMPORT_ACCOUNT)).toBe("dash");
  });
});

const row = (key: string, extra: Partial<Blade> = {}): Blade => ({ key, label: key, ...extra });

describe("blades", () => {
  const act = () => {};
  const rows: Blade[] = [
    row("fact"),
    row("days", { act, step: () => {} }),
    row("code", { field: { value: "", onInput: () => {} } }),
    row("off", { act, disabled: true }),
    row("open", { act, alt: { label: "revoke", run: act } }),
    row("used", { old: true }),
  ];

  test("up and down skip facts, greyed and disabled rows", () => {
    expect(firstBlade(rows)).toBe(1);
    expect(nextBlade(rows, 1, 1)).toBe(2);
    expect(nextBlade(rows, 2, 1)).toBe(4);
    expect(nextBlade(rows, 4, 1)).toBe(4);
    expect(nextBlade(rows, 1, -1)).toBe(1);
    expect(bladeAction(rows, 2, "down")).toEqual({ kind: "move", to: 4 });
    expect(bladeAction(rows, 4, "down")).toBeNull();
    expect(firstBlade([row("a"), row("b")])).toBe(0);
  });

  test("A acts, X only where the row has a second action, left and right only on a choice", () => {
    expect(bladeAction(rows, 1, "select")).toEqual({ kind: "act" });
    expect(bladeAction(rows, 2, "select")).toEqual({ kind: "act" });
    expect(bladeAction(rows, 3, "select")).toBeNull();
    expect(bladeAction(rows, 4, "alt")).toEqual({ kind: "alt" });
    expect(bladeAction(rows, 1, "alt")).toBeNull();
    expect(bladeAction(rows, 1, "right")).toEqual({ kind: "step", by: 1 });
    expect(bladeAction(rows, 1, "left")).toEqual({ kind: "step", by: -1 });
    // Left and right elsewhere fall through to the screen (the pods).
    expect(bladeAction(rows, 4, "left")).toBeNull();
    expect(bladeAction(rows, 4, "back")).toBeNull();
  });
});

describe("importing an account on the dashboard", () => {
  const entry = (over: Record<string, unknown> = {}) => JSON.stringify({
    xuid: "0009000000000042", gamertag: "afr", id: "acct:0009000000000042@live.test",
    server: "http://h", server_id: "live.test", key: RFC.d, x: RFC.x, ...over,
  });
  async function target(keys: unknown[] | null) {
    const added: LocalAccount[] = [];
    const urls: string[] = [];
    const f = (async (url: string) => {
      urls.push(url);
      return keys ? new Response(JSON.stringify({ keys }), { status: 200 }) : new Response("{}", { status: 404 });
    }) as unknown as typeof fetch;
    return { added, urls, to: { api: "http://h/v1", base: "http://h", id: "live.test", add: async (a: LocalAccount) => { added.push(a); }, fetch: f } };
  }

  test("keeps an account whose key is current here, and signs in as it", async () => {
    const kid = (await importSeed(RFC.d, RFC.x)).publicJwk.kid;
    const t = await target([{ kid, jwk: { x: RFC.x }, valid_to: null }]);
    const r = await importAccounts(entry(), t.to);
    expect(r).toEqual({ ok: true, gamertags: ["afr"] });
    expect(t.urls).toEqual(["http://h/v1/accounts/by-xuid/0009000000000042/keys"]);
    expect(t.added[0]).toMatchObject({ gamertag: "afr", server: "http://h", server_id: "live.test", kid, key: RFC.d });
  });

  test("refuses a retired key, another server's account and an unknown one", async () => {
    const kid = (await importSeed(RFC.d, RFC.x)).publicJwk.kid;
    const retired = await target([{ kid, jwk: { x: RFC.x }, valid_to: 5 }]);
    expect(await importAccounts(entry(), retired.to)).toEqual({ ok: false, error: "afr's key is not current on this server" });
    expect(retired.added).toEqual([]);
    const other = await target([]);
    const r = await importAccounts(entry({ server_id: "elsewhere" }), other.to);
    expect(r.ok).toBe(false);
    expect(other.urls).toEqual([]);
    const unknown = await target(null);
    expect(await importAccounts(entry(), unknown.to)).toEqual({ ok: false, error: "this server doesn't know afr" });
    expect((await importAccounts("nope", unknown.to)).ok).toBe(false);
  });
});
