// The mock's friends read matches the server's: friendships are public,
// pending requests show only to the player themselves.
import { describe, expect, test } from "bun:test";
import { World, HOST } from "../mock/world";
import { Social } from "../mock/social";
import { b64urlEncode } from "../src/state/verify";
import type { FriendsDoc } from "../src/api/social";

/** A token-shaped bearer for `xuid` (the mock parses, does not verify). */
function bearer(xuid: string): string {
  const part = (v: unknown) => b64urlEncode(new TextEncoder().encode(JSON.stringify(v)));
  return `Bearer ${part({ alg: "EdDSA" })}.${part({ iss: `acct:${xuid}@${HOST}`, lxm: "read" })}.sig`;
}

async function read(s: Social, tag: string, auth?: string): Promise<FriendsDoc> {
  const u = new URL(`http://mock/v1/accounts/${encodeURIComponent(tag)}/friends`);
  const r = await s.handle(new Request(u, { headers: auth ? { authorization: auth } : {} }), u);
  expect(r?.status).toBe(200);
  return r!.body as FriendsDoc;
}

describe("pending friend requests in the mock", () => {
  const w = new World();
  w.seed(10_000);
  const s = new Social(w);
  s.seed();
  const pete = w.accountByTag("Sneaky Pete")!;
  const lemon = w.accountByTag("Lemon Drop")!;

  test("anonymous and another player see friends but no requests", async () => {
    for (const auth of [undefined, bearer(lemon.xuid)]) {
      const doc = await read(s, "Sneaky Pete", auth);
      expect(doc.friends.length).toBeGreaterThan(0);
      expect(doc.requests_in).toEqual([]);
      expect(doc.requests_out).toEqual([]);
    }
  });

  test("the player sees their own requests", async () => {
    const doc = await read(s, "Sneaky Pete", bearer(pete.xuid));
    expect(doc.requests_in.map((r) => r.gamertag)).toContain("Lemon Drop");
  });
});
