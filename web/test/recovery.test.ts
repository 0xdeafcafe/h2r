import { describe, expect, test } from "bun:test";
import {
  base32Decode, base32Encode, codeError, fetchRecovery, formatCode, groupSecret, normalizeCode, parseRecovery,
  recoverAccount, RecoverError, secondsLeft, setupRecovery, signRecoverRequest, totp, TYP_RECOVER,
} from "../src/state/recovery";
import { importSeed, signUp, type LocalAccount } from "../src/state/account";
import { b64urlDecode, canonicalJson, jwkThumbprint, parseJws } from "../src/state/verify";
import { MockRecovery } from "../mock/recovery";
import { HOST, World } from "../mock/world";

// RFC 8037 appendix A's Ed25519 key.
const RFC = {
  d: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A",
  x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
  kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k",
};
const REC = { secret: "JBSWY3DPEHPK3PXP", otpauth: "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP", code: "123-456", period: 30, expires_at: 1_800_000_010 };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function recorder(reply: (url: string, init: RequestInit) => Response | Promise<Response>) {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const f = (async (url: string, init: RequestInit = {}) => { calls.push({ url, init }); return reply(url, init); }) as unknown as typeof fetch;
  return { f, calls };
}

const tokenOf = (init: RequestInit) => parseJws(new Headers(init.headers).get("authorization")!.slice(7))!;

describe("codes", () => {
  test("formatting and normalising", () => {
    expect(formatCode("123456")).toBe("123-456");
    expect(formatCode("123 456")).toBe("123-456");
    expect(formatCode("123-456")).toBe("123-456");
    expect(formatCode("12345")).toBe("12345");
    expect(normalizeCode(" 123-456 ")).toBe("123456");
    expect(codeError("123 456")).toBeNull();
    expect(codeError("123-45")).not.toBeNull();
    expect(codeError("abcdef")).not.toBeNull();
  });

  test("the secret reads in groups of four", () => {
    expect(groupSecret("JBSWY3DPEHPK3PXP")).toBe("JBSW Y3DP EHPK 3PXP");
    expect(groupSecret("ABCDEFGHIJ")).toBe("ABCD EFGH IJ");
  });

  test("seconds left stay within one period", () => {
    expect(secondsLeft({ expires_at: 1000, period: 30 }, 990_000)).toBe(10);
    expect(secondsLeft({ expires_at: 1000, period: 30 }, 989_500)).toBe(11);
    expect(secondsLeft({ expires_at: 1000, period: 30 }, 1_005_000)).toBe(0);
    expect(secondsLeft({ expires_at: 1000, period: 30 }, 0)).toBe(30);
  });

  test("base32, RFC 4648 section 10", () => {
    const enc = new TextEncoder();
    const v: Array<[string, string]> = [["", ""], ["f", "MY"], ["fo", "MZXQ"], ["foo", "MZXW6"], ["foob", "MZXW6YQ"], ["fooba", "MZXW6YTB"], ["foobar", "MZXW6YTBOI"]];
    for (const [plain, b32] of v) {
      expect(base32Encode(enc.encode(plain))).toBe(b32);
      expect(new TextDecoder().decode(base32Decode(b32))).toBe(plain);
    }
    expect(new TextDecoder().decode(base32Decode("mzxw 6ytb oi======"))).toBe("foobar");
  });

  test("TOTP, RFC 6238 appendix B (SHA-1)", async () => {
    const secret = new TextEncoder().encode("12345678901234567890");
    const v: Array<[number, string]> = [[59, "94287082"], [1111111109, "07081804"], [1111111111, "14050471"], [1234567890, "89005924"], [2000000000, "69279037"], [20000000000, "65353130"]];
    for (const [t, code] of v) expect(await totp(secret, t, 30, 8)).toBe(code);
    expect(await totp(secret, 59)).toBe("287082");
  });
});

describe("account transport", () => {
  test("refuses insecure discovery and API addresses before sending credentials", async () => {
    const { calls, f } = recorder(() => json({}));
    for (const [api, base] of [
      ["https://home.test/v1", "http://home.test"],
      ["http://home.test/v1", "https://home.test"],
    ] as const) {
      await expect(signUp(api, base, "home", "Mochi", undefined, f)).rejects.toThrow("HTTPS");
      await expect(recoverAccount(api, base, "home", "Mochi", "123456", f)).rejects.toThrow("HTTPS");
    }
    expect(calls).toHaveLength(0);
    const loop = recorder(() => json({ xuid: "0009aaaaaaaaaaaa", gamertag: "Mochi", id: "local" }, 201));
    await signUp("http://127.0.0.1/v1", "http://127.0.0.1", "home", "Mochi", undefined, loop.f);
    expect(loop.calls[0]!.init.redirect).toBe("error");
  });
});

describe("the recovery object", () => {
  test("parses, formats the code, and is null when absent or wrong", () => {
    expect(parseRecovery({ ...REC, code: "123456" })).toEqual(REC);
    expect(parseRecovery({ ...REC, period: undefined })!.period).toBe(30);
    expect(parseRecovery(undefined)).toBeNull();
    expect(parseRecovery({ secret: "X" })).toBeNull();
  });

  test("sign-up hands back the recovery setup, or null from an older server", async () => {
    const reply = (recovery?: unknown) => recorder(() => json({ xuid: "0009aaaaaaaaaaaa", gamertag: "Mochi", id: "acct:0009aaaaaaaaaaaa@h", ...(recovery ? { recovery } : {}) }, 201)).f;
    const withRec = await signUp("https://h/v1", "https://h", "h", "Mochi", undefined, reply(REC));
    expect(withRec.recovery).toEqual(REC);
    expect(withRec.account.gamertag).toBe("Mochi");
    const old = await signUp("https://h/v1", "https://h", "h", "Mochi", undefined, reply());
    expect(old.recovery).toBeNull();
    expect(old.account.xuid).toBe("0009aaaaaaaaaaaa");
  });
});

describe("the recover request", () => {
  test("header and canonical payload, byte for byte, signed by the new key", async () => {
    const k = await importSeed(RFC.d, RFC.x);
    const jws = await signRecoverRequest(k, "Sneaky Pete", "live.example", 1_800_000_000, "AAAAAAAAAAAAAAAAAAAAAA");
    const [h, p] = jws.split(".");
    expect(new TextDecoder().decode(b64urlDecode(h!))).toBe(`{"alg":"EdDSA","kid":"${RFC.kid}","typ":"h2live-recover"}`);
    const raw = new TextDecoder().decode(b64urlDecode(p!));
    expect(raw).toBe(`{"aud":"live.example","gamertag":"Sneaky Pete","iat":1800000000,"jwk":{"crv":"Ed25519","kty":"OKP","x":"${RFC.x}"},"nonce":"AAAAAAAAAAAAAAAAAAAAAA"}`);
    expect(canonicalJson(JSON.parse(raw))).toBe(raw);
    const parsed = parseJws(jws)!;
    expect(parsed.header.kid).toBe(await jwkThumbprint({ kty: "OKP", crv: "Ed25519", x: RFC.x }));
    const pub = await crypto.subtle.importKey("jwk", { kty: "OKP", crv: "Ed25519", x: RFC.x }, { name: "Ed25519" }, false, ["verify"]);
    expect(await crypto.subtle.verify({ name: "Ed25519" }, pub, parsed.signature, parsed.signingInput)).toBe(true);
  });

  test("a fresh nonce each time: 16 random bytes, base64url", async () => {
    const k = await importSeed(RFC.d, RFC.x);
    const a = parseJws(await signRecoverRequest(k, "A", "h", 1))!.payload as { nonce: string };
    const b = parseJws(await signRecoverRequest(k, "A", "h", 1))!.payload as { nonce: string };
    expect(a.nonce).toMatch(/^[A-Za-z0-9_-]{22}$/);
    expect(a.nonce).not.toBe(b.nonce);
  });

  test("recoverAccount posts {request, code} and keeps the new key", async () => {
    const { f, calls } = recorder(() => json({ xuid: "0009aaaaaaaaaaaa", gamertag: "Sneaky Pete", id: "acct:0009aaaaaaaaaaaa@h" }, 201));
    const a = await recoverAccount("https://h/v1", "https://h", "h", "Sneaky Pete", "123 456", f, 1_800_000_123_456);
    const c = calls[0]!;
    expect(c.url).toBe("https://h/v1/accounts/Sneaky%20Pete/recover");
    expect(c.init.method).toBe("POST");
    expect(new Headers(c.init.headers).get("content-type")).toBe("application/json");
    const body = JSON.parse(String(c.init.body)) as { request: string; code: string };
    expect(body.code).toBe("123456");
    const req = parseJws(body.request)!;
    expect((req.header as { typ?: string }).typ).toBe(TYP_RECOVER);
    const payload = req.payload as { aud: string; gamertag: string; iat: number; jwk: { x: string } };
    expect([payload.aud, payload.gamertag, payload.iat]).toEqual(["h", "Sneaky Pete", 1_800_000_123]);
    // The stored key is the one that signed.
    expect(payload.jwk.x).toBe(a.x);
    expect(req.header.kid).toBe(a.kid);
    expect((await importSeed(a.key, a.x)).publicJwk.kid).toBe(a.kid);
    expect(a).toMatchObject({ xuid: "0009aaaaaaaaaaaa", gamertag: "Sneaky Pete", id: "acct:0009aaaaaaaaaaaa@h", server: "https://h", server_id: "h" });
  });

  test("refusals read well", async () => {
    const tries: Array<[number, string, RegExp]> = [
      [403, "forbidden", /didn't work/],
      [429, "rate_limited", /Too many wrong codes/],
      [404, "not_found", /no Mochi on this server/],
      [400, "invalid", /refused the request: bad nonce/],
    ];
    for (const [status, error, msg] of tries) {
      const { f } = recorder(() => json({ error, message: "bad nonce" }, status));
      const e = await recoverAccount("https://h/v1", "https://h", "h", "Mochi", "123456", f).catch((x) => x);
      expect(e).toBeInstanceOf(RecoverError);
      expect(e.status).toBe(status);
      expect(e.code).toBe(error);
      expect(e.message).toMatch(msg);
    }
    const { f, calls } = recorder(() => json({}, 201));
    await expect(recoverAccount("https://h/v1", "https://h", "h", "Mochi", "12345", f)).rejects.toThrow("6 digits");
    await expect(recoverAccount("https://h/v1", "https://h", "h", "1bad", "123456", f)).rejects.toThrow("gamertag");
    expect(calls).toHaveLength(0);
  });
});

function me(xuid: string, server = HOST): LocalAccount {
  return { xuid, gamertag: "afr", id: `acct:${xuid}@${server}`, server: "https://m", server_id: server, kid: RFC.kid, key: RFC.d, x: RFC.x };
}

describe("reading and setting up", () => {
  test("GET with a recovery.read token; 404 is no recovery", async () => {
    const { f, calls } = recorder((url) => (url.includes("0009000000000001") ? json(REC) : json({ error: "not_found" }, 404)));
    expect(await fetchRecovery("https://h/v1", "aud", me("0009000000000001"), f)).toEqual(REC);
    expect(calls[0]!.url).toBe("https://h/v1/accounts/by-xuid/0009000000000001/recovery");
    const t = tokenOf(calls[0]!.init);
    expect((t.payload as { lxm: string; aud: string }).lxm).toBe("recovery.read");
    expect((t.payload as { aud: string }).aud).toBe("aud");
    expect(await fetchRecovery("https://h/v1", "aud", me("0009000000000002"), f)).toBeNull();
  });

  test("POST with a recovery.write token and no body", async () => {
    const { f, calls } = recorder(() => json(REC, 201));
    expect(await setupRecovery("https://h/v1", "aud", me("0009000000000001"), f)).toEqual(REC);
    expect(calls[0]!.init.method).toBe("POST");
    expect(calls[0]!.init.body).toBeUndefined();
    expect((tokenOf(calls[0]!.init).payload as { lxm: string }).lxm).toBe("recovery.write");
  });
});

describe("the mock's recovery endpoints", () => {
  function setup() {
    const world = new World();
    let t = Date.now();
    const rec = new MockRecovery(world, () => t);
    const a = world.createExternalAccount("afr", { kty: "OKP", crv: "Ed25519", x: RFC.x, kid: RFC.kid });
    const f = (async (url: string, init: RequestInit = {}) => {
      const r = await rec.handle(new Request(url, init), new URL(url));
      return r ? json(r.body, r.status) : json({ error: "not_found" }, 404);
    }) as unknown as typeof fetch;
    return { world, rec, a, f, advance: (ms: number) => { t += ms; }, now: () => t };
  }

  test("none until set up; the code is the secret's TOTP; a reset changes the secret", async () => {
    const { a, f, now } = setup();
    const account = me(a.xuid);
    expect(await fetchRecovery("https://m/v1", HOST, account, f, now())).toBeNull();
    const first = await setupRecovery("https://m/v1", HOST, account, f, now());
    expect(first.secret).toMatch(/^[A-Z2-7]{32}$/);
    expect(first.otpauth).toStartWith("otpauth://totp/");
    expect(first.code).toBe(formatCode(await totp(base32Decode(first.secret), now() / 1000)));
    expect(first.expires_at % 30).toBe(0);
    expect((await fetchRecovery("https://m/v1", HOST, account, f, now()))!.secret).toBe(first.secret);
    const second = await setupRecovery("https://m/v1", HOST, account, f, now());
    expect(second.secret).not.toBe(first.secret);
    // A token from another account is refused.
    await expect(fetchRecovery("https://m/v1", HOST, { ...account, id: `acct:0009000000000099@${HOST}` }, f, now())).rejects.toMatchObject({ status: 403 });
  });

  test("recover: wrong codes, the lockout, then the right code moves the account to the new key", async () => {
    const { a, f, rec, advance, now } = setup();
    const r = await rec.setup(a.xuid, a.gamertag);
    const good = await totp(base32Decode(r.secret), now() / 1000);
    const bad = String((Number(good) + 500_000) % 1_000_000).padStart(6, "0");
    for (let i = 0; i < 5; i++) {
      await expect(recoverAccount("https://m/v1", "https://m", HOST, "afr", bad, f, now())).rejects.toMatchObject({ status: 403 });
    }
    await expect(recoverAccount("https://m/v1", "https://m", HOST, "afr", good, f, now())).rejects.toMatchObject({ status: 429 });
    advance(61_000);
    const code = await totp(base32Decode(r.secret), now() / 1000);
    const back = await recoverAccount("https://m/v1", "https://m", HOST, "afr", formatCode(code), f, now());
    expect(back.xuid).toBe(a.xuid);
    expect(a.key.kid).toBe(back.kid);
    expect(a.key.jwk.x).toBe(back.x);
    // The new key reads recovery; the old one no longer does.
    expect((await fetchRecovery("https://m/v1", HOST, back, f, now()))!.secret).toBe(r.secret);
    await expect(fetchRecovery("https://m/v1", HOST, me(a.xuid), f, now())).rejects.toMatchObject({ status: 401 });
    await expect(recoverAccount("https://m/v1", "https://m", HOST, "nobody", "123456", f, now())).rejects.toMatchObject({ status: 404 });
  });

  test("an account without recovery can't be taken over", async () => {
    const { f, now } = setup();
    await expect(recoverAccount("https://m/v1", "https://m", HOST, "afr", "123456", f, now())).rejects.toMatchObject({ status: 403 });
  });
});
