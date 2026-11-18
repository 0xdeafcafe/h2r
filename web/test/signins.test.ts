import { describe, expect, test } from "bun:test";
import {
  confirmText, fetchSignIns, OTHERS_KEY, parseSignIns, SIGNINS_READ, SIGNINS_WRITE, signInLabel, signInMeta, signInRows,
  signInSummary, signOutOne, signOutOthers, type SignIn,
} from "../src/state/signins";
import { MASKED_CODE, REVEAL_MS, shownCode } from "../src/state/recovery";
import { generateKey, type LocalAccount } from "../src/state/account";
import { parseJws } from "../src/state/verify";

const NOW = Date.parse("2026-11-18T12:00:00Z");
const at = (s: number) => new Date(NOW - s * 1000).toISOString();

const here: SignIn = { kid: "k-here", kind: "browser", name: "Chrome on macOS", via: "signup", created_at: at(86400 * 30), last_seen: at(0), current: true };
const game: SignIn = { kid: "k-game", kind: "game", name: "Halo 2", via: "code", created_at: at(86400 * 3), last_seen: at(2 * 3600), current: false };
const phone: SignIn = { kid: "k-phone", kind: "browser", name: "Safari on iPhone", via: "code", created_at: at(86400), last_seen: null, current: false };
const old: SignIn = { kid: "k-old", kind: "unknown", name: null, via: "signup", created_at: at(86400 * 400), last_seen: null, current: false };

describe("the sign-in code is hidden until asked for", () => {
  test("masked before and after the reveal window, itself inside it", () => {
    expect(shownCode("123-456", 0, NOW)).toBe(MASKED_CODE);
    const until = NOW + REVEAL_MS;
    expect(shownCode("123-456", until, NOW)).toBe("123-456");
    expect(shownCode("123-456", until, until - 1)).toBe("123-456");
    expect(shownCode("123-456", until, until)).toBe(MASKED_CODE);
    expect(REVEAL_MS).toBe(30_000);
    expect(MASKED_CODE).not.toMatch(/\d/);
  });
});

describe("sign-ins", () => {
  test("parse the server's list, dropping malformed items and unknown kinds", () => {
    const list = parseSignIns({
      items: [
        here, { ...game, kind: "toaster" }, { kid: "", created_at: "x" }, null, { kid: "k" },
        { kid: "k-min", created_at: at(0) },
      ],
      position: 9,
    });
    expect(list.map((s) => s.kid)).toEqual(["k-here", "k-game", "k-min"]);
    expect(list[1]!.kind).toBe("unknown");
    expect(list[2]).toEqual({ kid: "k-min", kind: "unknown", name: null, via: "", created_at: at(0), last_seen: null, current: false });
    expect(parseSignIns(null)).toEqual([]);
    expect(parseSignIns({ items: "no" })).toEqual([]);
  });

  test("rows name each sign-in and end with signing out the others", () => {
    const rows = signInRows([here, game, phone, old], NOW);
    expect(rows.map((r) => r.label)).toEqual(["this browser", "Halo 2", "Safari on iPhone", "a device", "sign out others"]);
    expect(rows.map((r) => r.meta)).toEqual(["signed in here", "used 2 h ago", expect.stringMatching(/^since /), expect.stringMatching(/^since /), "all 3 others"]);
    expect(rows.at(-1)!.key).toBe(OTHERS_KEY);
    // Alone: no "others" row.
    expect(signInRows([here], NOW).map((r) => r.key)).toEqual(["k-here"]);
    expect(signInRows([here, game], NOW).at(-1)!.meta).toBe("1 other");
  });

  test("labels fall back on the kind", () => {
    expect(signInLabel({ ...game, name: null })).toBe("Halo 2");
    expect(signInLabel({ ...phone, name: null })).toBe("a browser");
    expect(signInLabel(old)).toBe("a device");
    expect(signInMeta({ ...game, last_seen: at(30) }, NOW)).toBe("used now");
    expect(signInMeta({ ...game, last_seen: at(600) }, NOW)).toBe("used 10 min ago");
    expect(signInMeta({ ...game, last_seen: at(86400 * 5) }, NOW)).toBe("used 5 days ago");
  });

  test("the account plate sums them up", () => {
    expect(signInSummary([here])).toBe("only this browser");
    expect(signInSummary([here, game])).toBe("this browser, Halo 2");
    expect(signInSummary([here, game, phone, old])).toBe("this browser, Halo 2, Safari on iPhone and 1 more");
    expect(signInSummary([])).toBe("nowhere");
  });

  test("each sign-out asks first, in words that say what it ends", () => {
    const rows = signInRows([here, game, phone], NOW);
    expect(confirmText(rows[1]!, game)).toBe("Sign out Halo 2? It will need your gamertag and sign-in code to sign in again.");
    expect(confirmText(rows[0]!, here)).toContain("Sign out of this browser?");
    expect(confirmText(rows.at(-1)!, undefined)).toContain("everywhere but this browser (all 2 others)");
  });
});

describe("the sign-ins API", () => {
  async function account(): Promise<LocalAccount> {
    const k = await generateKey();
    return { xuid: "0009000000000001", gamertag: "Roamer", id: "acct:0009000000000001@live.test", server: "http://live.test", server_id: "live.test", kid: k.publicJwk.kid, key: k.d, x: k.publicJwk.x };
  }
  function recorder(reply: unknown, status = 200) {
    const calls: { url: string; method: string; lxm: string }[] = [];
    const fetchFn = (async (url: string, init?: RequestInit) => {
      const auth = (init?.headers as Record<string, string>).authorization!;
      const claims = parseJws(auth.slice(7))!.payload as { lxm: string };
      calls.push({ url, method: init?.method ?? "GET", lxm: claims.lxm });
      return new Response(JSON.stringify(reply), { status });
    }) as unknown as typeof fetch;
    return { calls, fetchFn };
  }

  test("lists, signs one out and signs the others out, each with its own token scope", async () => {
    const me = await account();
    const api = "http://live.test/v1";
    let r = recorder({ items: [here, game], position: 3 });
    expect((await fetchSignIns(api, "live.test", me, r.fetchFn, NOW)).map((s) => s.kid)).toEqual(["k-here", "k-game"]);
    expect(r.calls).toEqual([{ url: `${api}/accounts/by-xuid/0009000000000001/signins`, method: "GET", lxm: SIGNINS_READ }]);

    r = recorder({ revoked: ["k-game"], position: 4 });
    expect(await signOutOne(api, "live.test", me, "k/game", r.fetchFn, NOW)).toEqual(["k-game"]);
    expect(r.calls).toEqual([{ url: `${api}/accounts/by-xuid/0009000000000001/signins/k%2Fgame`, method: "DELETE", lxm: SIGNINS_WRITE }]);

    r = recorder({ revoked: ["a", "b", 3], position: 5 });
    expect(await signOutOthers(api, "live.test", me, r.fetchFn, NOW)).toEqual(["a", "b"]);
    expect(r.calls).toEqual([{ url: `${api}/accounts/by-xuid/0009000000000001/signins/sign-out-others`, method: "POST", lxm: SIGNINS_WRITE }]);
  });

  test("a refusal is an error with the server's message", async () => {
    const me = await account();
    const r = recorder({ error: "conflict", message: "that is the sign-in making this request" }, 409);
    await expect(signOutOne("http://live.test/v1", "live.test", me, "k-here", r.fetchFn, NOW)).rejects.toThrow("that is the sign-in making this request");
  });
});
