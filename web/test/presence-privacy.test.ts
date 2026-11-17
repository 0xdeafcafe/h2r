// The web's side of presence privacy: friends lists read as the signed-in
// reader, a sign-in or sign-out reloads at once, a redacted presence event
// reads as "In a game", and the mock's stream waits for `?auth=1`'s token.
import { describe, expect, test } from "bun:test";
import { socialApi } from "../src/api/social";
import { signInChanged, type Reader } from "../src/state/signin-watch";
import { applyEvent, applyPresenceSnapshot, initialState } from "../src/state/reducer";
import { streamAuthStep } from "../mock/streamauth";
import { ev } from "./helpers";

const target = { base: "http://live.test", api: "http://live.test/v1" } as Parameters<typeof socialApi>[0];
const HOST = "0009000000000001";

describe("friends as the signed-in reader", () => {
  test("the friends list sends the reader's token, and none signed out", async () => {
    const seen: Array<string | null> = [];
    const real = globalThis.fetch;
    globalThis.fetch = (async (_url: string, init?: RequestInit) => {
      seen.push(((init?.headers ?? {}) as Record<string, string>).authorization ?? null);
      return new Response(JSON.stringify({ friends: [], requests_in: [], requests_out: [] }));
    }) as typeof fetch;
    try {
      await socialApi(target, async () => "tok").friends("Pete");
      await socialApi(target).friends("Pete");
      expect(seen).toEqual(["Bearer tok", null]);
    } finally { globalThis.fetch = real; }
  });
});

describe("sign-in changes", () => {
  test("signing in or out on the same server reloads; a server change, the first reading or nothing new does not", () => {
    const steps: Array<[Reader | undefined, Reader, boolean]> = [
      [undefined, { base: "a", who: HOST }, false],
      [{ base: "a", who: null }, { base: "a", who: HOST }, true],
      [{ base: "a", who: HOST }, { base: "a", who: HOST }, false],
      [{ base: "a", who: HOST }, { base: "a", who: null }, true],
      [{ base: "a", who: HOST }, { base: "b", who: null }, false],
    ];
    for (const [prev, now, want] of steps) expect(signInChanged(prev, now)).toBe(want);
  });
});

describe("redacted presence", () => {
  test("a hidden game's presence reads as playing, in a game, with no game id", () => {
    const s = initialState();
    applyPresenceSnapshot(s, [{ xuid: HOST, gamertag: "Pete", state: "online", since: "2026-11-16T10:00:00Z" }]);
    applyEvent(s, ev("PresenceChanged", `presence-${HOST}`, { xuid: HOST, state: "playing", activity: "In a game", redacted: true }, "2026-11-16T10:05:00Z"));
    const p = s.accounts[HOST]!.presence!;
    expect(p.state).toBe("playing");
    expect(p.game_id).toBeNull();
    expect(p.activity).toBe("In a game");
  });
});

describe("mock stream ?auth=1", () => {
  test("waits for the auth message, then replays; anything else closes", () => {
    expect(streamAuthStep(true, JSON.stringify({ kind: "auth", token: "t" }))).toBe("replay");
    expect(streamAuthStep(true, JSON.stringify({ kind: "auth" }))).toBe("close");
    expect(streamAuthStep(true, JSON.stringify({ kind: "hello" }))).toBe("close");
    expect(streamAuthStep(true, "not json")).toBe("close");
    expect(streamAuthStep(false, JSON.stringify({ kind: "auth", token: "t" }))).toBe("ignore");
  });
});
