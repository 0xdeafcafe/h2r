import { describe, expect, test } from "bun:test";
import { issueToken, tokenClaims, TOKEN_LIFETIME_S } from "../src/admin/token";
import {
  buildPatch,
  hopperErrors,
  motdError,
  parseImportEntry,
  toEditable,
  type HopperView,
  type MotdDoc,
} from "../src/admin/validate";
import { importSeed } from "../src/state/account";
import type { LocalAccount } from "../src/state/account";
import { b64urlDecode, canonicalJson, parseJws } from "../src/state/verify";

const RFC = {
  d: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A",
  x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
  kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k",
};

function me(): LocalAccount {
  return {
    xuid: "0009000000000042",
    gamertag: "afr",
    id: "acct:0009000000000042@localhost",
    server: "http://127.0.0.1:8074",
    server_id: "localhost",
    kid: RFC.kid,
    key: RFC.d,
    x: RFC.x,
  };
}

describe("admin request tokens", () => {
  test("claims match live-proto's issue_token", () => {
    const now = 1_800_000_123_456;
    const c = tokenClaims(me(), "localhost", "titles.write", now);
    expect(c.iss).toBe("acct:0009000000000042@localhost");
    expect(c.aud).toBe("localhost");
    expect(c.lxm).toBe("titles.write");
    expect(c.iat).toBe(1_800_000_123);
    expect(c.exp).toBe(c.iat + TOKEN_LIFETIME_S);
    expect(c.jti).toMatch(/^[A-Za-z0-9_-]{22}$/); // 16 bytes, base64url
    // The signed payload is canonical JSON with the keys sorted.
    const { jti, ...rest } = c;
    expect(canonicalJson(rest)).toBe(
      `{"aud":"localhost","exp":${c.exp},"iat":${c.iat},"iss":"acct:0009000000000042@localhost","lxm":"titles.write"}`,
    );
    expect(jti).not.toBe(tokenClaims(me(), "localhost", "titles.write", now).jti);
  });

  test("the token verifies under its own key, header typ JWT, kid named", async () => {
    const token = await issueToken(me(), "localhost", "titles.write", 1_800_000_000_000);
    const p = parseJws(token)!;
    expect(p.header.alg).toBe("EdDSA");
    expect((p.header as { typ?: string }).typ).toBe("JWT");
    expect(p.header.kid).toBe(RFC.kid);
    const raw = new TextDecoder().decode(b64urlDecode(token.split(".")[1]!));
    expect(canonicalJson(JSON.parse(raw))).toBe(raw);
    const claims = p.payload as Record<string, unknown>;
    expect(claims.iss).toBe("acct:0009000000000042@localhost");
    expect(claims.exp).toBe((claims.iat as number) + TOKEN_LIFETIME_S);
    // The seed in the account entry is the key that signed.
    const key = await importSeed(RFC.d, RFC.x);
    const publicOnly = await crypto.subtle.importKey(
      "jwk", { kty: "OKP", crv: "Ed25519", x: RFC.x }, { name: "Ed25519" }, false, ["verify"],
    );
    expect(await crypto.subtle.verify({ name: "Ed25519" }, publicOnly, p.signature, p.signingInput)).toBe(true);
    expect(key.publicJwk.kid).toBe(RFC.kid);
  });
});

describe("motd validation", () => {
  const doc = (messages: string[], header = "Hi"): MotdDoc => ({ header, messages });
  test("accepts the game's limits", () => {
    expect(motdError(doc(["Welcome back."]))).toBeNull();
    expect(motdError(doc(["x".repeat(255)], "y".repeat(255)))).toBeNull();
    expect(motdError(doc(Array(32).fill("m")))).toBeNull();
  });
  test("rejects what the build would refuse", () => {
    expect(motdError(doc([]))).toContain("at least one");
    expect(motdError(doc(Array(33).fill("m")))).toContain("at most 32");
    expect(motdError(doc(["m"], "x".repeat(256)))).toContain("header");
    expect(motdError(doc(["m".repeat(256)]))).toContain("message 1");
    // 128 surrogate pairs are 256 UTF-16 units, like the game's fields.
    expect(motdError(doc(["𝄞".repeat(128)]))).toContain("message 1");
    expect(motdError(doc(["a\0b"]))).toContain("NUL");
  });
});

describe("hopper validation", () => {
  const hopper = (over: Partial<HopperView> = {}): HopperView => ({
    index: 0,
    type: 4,
    status: 1,
    record_count: 1,
    name: "Rumble Training",
    ranges: [1, 8, 4],
    dwords: Array(18).fill(0),
    records: [{ id: 1, string: "slayer", flags: 0, dwords: [0, 0] }],
    ...over,
  });

  test("accepts a hopper the game would load", () => {
    expect(hopperErrors(hopper())).toEqual([]);
    expect(hopperErrors(hopper({ name: "x".repeat(132) }))).toEqual([]);
    expect(hopperErrors(hopper({ name: "x".repeat(133) } as Partial<HopperView>)).length).toBeGreaterThan(0);
  });

  test("rejects the game's rules and ours", () => {
    expect(hopperErrors(hopper({ type: 0 }))).toEqual(["type must be 1 to 5"]);
    expect(hopperErrors(hopper({ type: 6 } as Partial<HopperView>))).toEqual(["type must be 1 to 5"]);
    expect(hopperErrors(hopper({ status: 3 } as Partial<HopperView>))[0]).toContain("status");
    expect(hopperErrors(hopper({ name: "" }))[0]).toContain("name");
    expect(hopperErrors(hopper({ name: "café" }))[0]).toContain("ASCII");
    const none = hopper({ records: [], record_count: 0 });
    expect(hopperErrors(none).join()).toContain("1 to 100 entries, got 0");
    const many = hopper({ records: Array(101).fill({ id: 1, string: "s", flags: 0, dwords: [0, 0] }), record_count: 101 });
    expect(hopperErrors(many).join()).toContain("got 101");
    const mismatch = hopper({ record_count: 2 });
    expect(hopperErrors(mismatch).join()).toContain("record_count does not match");
    const longString = hopper({ records: [{ id: 1, string: "s".repeat(72), flags: 0, dwords: [0, 0] }] });
    expect(hopperErrors(longString).join()).toContain("longer than 71");
    const badId = hopper({ records: [{ id: -1, string: "s", flags: 0, dwords: [0, 0] }] });
    expect(hopperErrors(badId).join()).toContain("whole number");
    expect(hopperErrors(hopper({ ranges: [-1, 0, 0] as [number, number, number] })).join()).toContain("range");
    expect(hopperErrors(hopper({ dwords: Array(17).fill(0) })).join()).toContain("dword array");
  });
});

describe("the hopper PUT body", () => {
  const doc = (): HopperView[] => [
    {
      index: 0, type: 4, status: 1, record_count: 2, name: "Big Team Battle",
      ranges: [1, 8, 6], dwords: Array(18).fill(0x753007a4),
      records: [
        { id: 1, string: "one", flags: 0, dwords: [0, 0] },
        { id: 2, string: "two", flags: 0, dwords: [1, 0] },
      ],
    },
    {
      index: 1, type: 2, status: 1, record_count: 1, name: "Rumble Training",
      ranges: [1, 8, 4], dwords: Array(18).fill(0),
      records: [{ id: 9, string: "solo", flags: 0, dwords: [0, 0] }],
    },
  ];

  test("nothing changed is an empty patch, so no PUT", () => {
    const original = toEditable(doc());
    expect(buildPatch(original, toEditable(doc()))).toEqual({ hoppers: [] });
    expect(buildPatch(original, original)).toEqual({ hoppers: [] });
  });

  test("only changed hoppers go in, with derived record_count", () => {
    const original = toEditable(doc());
    const edited = toEditable(doc());
    edited[1]!.name = "Rumble Pit";
    const patch = buildPatch(original, edited);
    expect(patch.hoppers.length).toBe(1);
    expect(patch.hoppers[0]!.index).toBe(1);
    expect(patch.hoppers[0]!.name).toBe("Rumble Pit");
    expect(patch.hoppers[0]!.record_count).toBe(1);
    // Records that stayed in place carry no `from`.
    expect(patch.hoppers[0]!.records[0]!.from).toBeUndefined();
  });

  test("reorders and copies say where their bytes come from", () => {
    const original = toEditable(doc());
    const edited = toEditable(doc());
    // Swap the two entries: each keeps its published slot's bytes.
    edited[0]!.records = [edited[0]!.records[1]!, edited[0]!.records[0]!];
    edited[0]!.records[0]!.string = "two, first now";
    // Then append a copy of the (originally) first entry.
    edited[0]!.records.push({ ...original[0]!.records[0]!, string: "a copy of one", src: 0 });
    const patch = buildPatch(original, edited);
    expect(patch.hoppers.length).toBe(1);
    const records = patch.hoppers[0]!.records;
    expect(records.length).toBe(3);
    expect(patch.hoppers[0]!.record_count).toBe(3);
    expect(records[0]!).toMatchObject({ id: 2, string: "two, first now", from: 1 });
    expect(records[1]!).toMatchObject({ id: 1, from: 0 });
    expect(records[2]!).toMatchObject({ id: 1, string: "a copy of one", from: 0 });
    // Field edits ride along on the moved records.
    expect(records[1]!.string).toBe("one");
  });

  test("removal is a shorter list", () => {
    const original = toEditable(doc());
    const edited = toEditable(doc());
    edited[0]!.records = [edited[0]!.records[0]!];
    const patch = buildPatch(original, edited);
    expect(patch.hoppers[0]!.record_count).toBe(1);
    expect(patch.hoppers[0]!.records.length).toBe(1);
  });
});

describe("importing an exported CLI account", () => {
  test("takes the export's entry, bare seed or JWK key", () => {
    const seedForm = JSON.stringify({
      xuid: "0009000000000042", gamertag: "afr",
      id: "acct:0009000000000042@localhost",
      server: "http://127.0.0.1:8074", server_id: "localhost",
      kid: RFC.kid, key: RFC.d, x: RFC.x,
    });
    const fromSeed = parseImportEntry(seedForm);
    expect(fromSeed.ok).toBe(true);
    if (fromSeed.ok) {
      expect(fromSeed.accounts[0]).toMatchObject({
        xuid: "0009000000000042", gamertag: "afr", key: RFC.d, x: RFC.x,
      });
    }
    const jwkForm = seedForm.replace(
      `"key":"${RFC.d}","x":"${RFC.x}"`,
      `"key":{"kty":"OKP","crv":"Ed25519","x":"${RFC.x}","d":"${RFC.d}"}`,
    );
    const fromJwk = parseImportEntry(jwkForm);
    expect(fromJwk.ok).toBe(true);
    if (fromJwk.ok) expect(fromJwk.accounts[0]!.key).toBe(RFC.d);
    // A list works too (the CLI writes one; be liberal).
    const list = parseImportEntry(`[${seedForm}]`);
    expect(list.ok && list.accounts.length === 1).toBe(true);
  });

  test("refuses what it cannot sign in as, without echoing the key", () => {
    expect(parseImportEntry("not json").ok).toBe(false);
    expect(parseImportEntry("[]").ok).toBe(false);
    expect(parseImportEntry('{"gamertag":"afr"}').ok).toBe(false);
    const noKey = parseImportEntry('{"xuid":"0009000000000042","gamertag":"afr","server":"http://h"}');
    expect(noKey.ok).toBe(false);
    if (!noKey.ok) expect(noKey.error).toContain("key");
  });
});
