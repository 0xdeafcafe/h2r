import { describe, expect, test } from "bun:test";
import { accountsFile, createAccount, gamertagError, generateKey, importSeed, signAccountRequest, signCompact, TYP_ACCOUNT } from "../src/state/account";
import { b64urlDecode, canonicalJson, jwkThumbprint, parseJws } from "../src/state/verify";

// RFC 8037 appendix A: the Ed25519 test key, its thumbprint and a signature.
const RFC = {
  d: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A",
  x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo",
  kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k",
  jws: "eyJhbGciOiJFZERTQSJ9.RXhhbXBsZSBvZiBFZDI1NTE5IHNpZ25pbmc.hgyY0il_MGCjP0JzlnLWG1PPOt7-09PGcvMg3AIbQR6dWbhijcNR4ki4iylGjg5BhVsPt9g7sVvpAr_MuM0KAg",
};

/** live-proto's AccountRequest::open, reimplemented: typ, canonical
 *  payload, kid = thumbprint of the carried key, signature by that key. */
async function openAccountRequest(jws: string) {
  const p = parseJws(jws)!;
  if (p.header.alg !== "EdDSA") throw new Error("alg");
  if ((p.header as { typ?: string }).typ !== TYP_ACCOUNT) throw new Error("typ");
  const raw = new TextDecoder().decode(b64urlDecode(jws.split(".")[1]!));
  if (canonicalJson(JSON.parse(raw)) !== raw) throw new Error("not canonical");
  const body = p.payload as { gamertag: string; jwk: { kty: string; crv: string; x: string; kid?: string }; invite?: string };
  const kid = await jwkThumbprint(body.jwk);
  if (body.jwk.kid && body.jwk.kid !== kid) throw new Error("jwk kid");
  if (p.header.kid !== kid) throw new Error("header kid");
  const key = await crypto.subtle.importKey("jwk", { kty: "OKP", crv: "Ed25519", x: body.jwk.x }, { name: "Ed25519" }, false, ["verify"]);
  if (!(await crypto.subtle.verify({ name: "Ed25519" }, key, p.signature, p.signingInput))) throw new Error("signature");
  return body;
}

describe("sign-up crypto", () => {
  test("RFC 8037 vector: thumbprint and deterministic signature", async () => {
    const k = await importSeed(RFC.d, RFC.x);
    expect(k.publicJwk.kid).toBe(RFC.kid);
    const jws = await signCompact(k.privateKey, { alg: "EdDSA" }, "Example of Ed25519 signing");
    expect(jws).toBe(RFC.jws);
  });

  test("account request matches live-proto's rules, byte for byte", async () => {
    const k = await importSeed(RFC.d, RFC.x);
    const jws = await signAccountRequest(k, "Sneaky Pete", "inv");
    const [h, p] = jws.split(".");
    expect(new TextDecoder().decode(b64urlDecode(h!))).toBe(`{"alg":"EdDSA","kid":"${RFC.kid}","typ":"h2live-account"}`);
    expect(new TextDecoder().decode(b64urlDecode(p!))).toBe(
      `{"gamertag":"Sneaky Pete","invite":"inv","jwk":{"crv":"Ed25519","kid":"${RFC.kid}","kty":"OKP","x":"${RFC.x}"}}`);
    const body = await openAccountRequest(jws);
    expect(body.gamertag).toBe("Sneaky Pete");
    // No invite: the field is left out, not null.
    const plain = await signAccountRequest(k, "Mochi");
    expect(new TextDecoder().decode(b64urlDecode(plain.split(".")[1]!))).not.toContain("invite");
  });

  test("a tampered request fails the check", async () => {
    const k = await generateKey();
    const jws = await signAccountRequest(k, "Mochi");
    const [h, , s] = jws.split(".");
    const forged = btoa(canonicalJson({ gamertag: "Not Mochi", jwk: k.publicJwk })).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    await expect(openAccountRequest(`${h}.${forged}.${s}`)).rejects.toThrow("signature");
  });

  test("gamertag rules", () => {
    expect(gamertagError("Sneaky Pete")).toBeNull();
    expect(gamertagError("")).not.toBeNull();
    expect(gamertagError("1abc")).not.toBeNull();
    expect(gamertagError("a  b")).not.toBeNull();
    expect(gamertagError("abcdefghijklmnop")).not.toBeNull();
  });

  test("createAccount posts application/jose and builds an accounts.json entry", async () => {
    let seen: { url: string; type: string | null; body: string } | null = null;
    const fakeFetch = (async (url: string, init: RequestInit) => {
      seen = { url, type: new Headers(init.headers).get("content-type"), body: String(init.body) };
      const body = await openAccountRequest(String(init.body));
      return new Response(JSON.stringify({ xuid: "0009aaaaaaaaaaaa", gamertag: body.gamertag, id: "acct:0009aaaaaaaaaaaa@h" }), { status: 201 });
    }) as unknown as typeof fetch;
    const a = await createAccount("http://h/v1", "http://h", "h", "Mochi", undefined, fakeFetch);
    expect(seen!.url).toBe("http://h/v1/accounts");
    expect(seen!.type).toBe("application/jose");
    const file = JSON.parse(accountsFile(a));
    expect(file).toEqual([{ xuid: "0009aaaaaaaaaaaa", gamertag: "Mochi", id: "acct:0009aaaaaaaaaaaa@h", server: "http://h", server_id: "h", kid: a.kid, key: a.key }]);
    // The stored seed really is the key that signed.
    expect((await importSeed(a.key, a.x)).publicJwk.kid).toBe(a.kid);
  });

  test("a taken gamertag reads well", async () => {
    const fakeFetch = (async () => new Response(JSON.stringify({ error: "conflict", message: "taken" }), { status: 409 })) as unknown as typeof fetch;
    await expect(createAccount("http://h/v1", "http://h", "h", "Mochi", undefined, fakeFetch)).rejects.toThrow("already has");
  });
});
