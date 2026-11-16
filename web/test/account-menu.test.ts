import { describe, expect, test } from "bun:test";
import { lockedHint, menuFor } from "../src/dash/menu";
import { fetchDevices, parseKeys, shortKid } from "../src/state/devices";
import { INVITE_DAY_CHOICES, INVITE_MAX_DAYS, nextInviteDays } from "../src/admin/server";

describe("the main menu", () => {
  test("signed out: the ways in first, then what works without an account, then the greyed ones", () => {
    const items = menuFor(false);
    expect(items.slice(0, 3)).toEqual(["signin", "signup", "recover"]);
    expect(items.slice(3, 5)).toEqual(["live", "history"]);
    const locked = items.filter((i) => lockedHint(i, false));
    expect(locked).toEqual(["sessions", "players", "settings"]);
    for (const i of locked) expect(lockedHint(i, false)).toStartWith("sign in");
    expect(items).not.toContain("account");
    expect(items).not.toContain("friends");
  });

  test("signed in: the usual items, the account, nothing greyed", () => {
    const items = menuFor(true);
    expect(items).toEqual(["live", "sessions", "friends", "players", "history", "account", "settings"]);
    expect(items.filter((i) => lockedHint(i, true))).toEqual([]);
    expect(items).not.toContain("signin");
  });
});

describe("devices", () => {
  const json = (b: unknown, status = 200) => new Response(JSON.stringify(b), { status });

  test("keys with the time each was added, current first, newest first", async () => {
    const urls: string[] = [];
    const f = (async (url: string) => {
      urls.push(url);
      if (url.endsWith("/accounts/Sneaky%20Pete/keys")) {
        return json({ keys: [
          { kid: "oldkey-aaaaaaaa", jwk: {}, valid_from: 5, valid_to: 40 },
          { kid: "newkey-bbbbbbbb", jwk: {}, valid_from: 40, valid_to: null },
          { nope: 1 },
        ] });
      }
      const from = Number(new URL(url).searchParams.get("from"));
      return json({ items: [{ position: from + 1, at: `2026-11-${String(from + 1).padStart(2, "0").slice(-2)}T00:00:00Z` }] });
    }) as unknown as typeof fetch;
    const d = await fetchDevices("http://h/v1", "Sneaky Pete", f);
    expect(d.map((x) => [x.kid, x.current, x.from])).toEqual([["newkey-bbbbbbbb", true, 40], ["oldkey-aaaaaaaa", false, 5]]);
    expect(d[1]!.since).toBe("2026-11-05T00:00:00Z");
    expect(urls).toContain("http://h/v1/events?from=39&limit=1");
    expect(shortKid("newkey-bbbbbbbb")).toBe("newkey-b");
  });

  test("an event log that won't say leaves the date out; a bad answer is no keys", async () => {
    const f = (async (url: string) => (url.includes("/keys") ? json({ keys: [{ kid: "k", valid_from: 3, valid_to: null }] }) : json({}, 403))) as unknown as typeof fetch;
    expect((await fetchDevices("http://h/v1", "a", f))[0]).toMatchObject({ kid: "k", since: null, from: 3, current: true });
    expect(parseKeys({})).toEqual([]);
    expect(parseKeys(null)).toEqual([]);
  });
});

describe("invite length", () => {
  test("cycles 1, 7, 30 days: the server caps an admin's invite at 30, so there's no never", () => {
    expect([...INVITE_DAY_CHOICES]).toEqual([1, 7, 30]);
    expect(Math.max(...INVITE_DAY_CHOICES)).toBe(INVITE_MAX_DAYS);
    expect([nextInviteDays(1), nextInviteDays(7), nextInviteDays(30)]).toEqual([7, 30, 1]);
    expect([nextInviteDays(1, -1), nextInviteDays(30, -1)]).toEqual([30, 7]);
  });
});
