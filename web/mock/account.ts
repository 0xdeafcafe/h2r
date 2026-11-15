// Opening a `POST /v1/accounts` body the way the real server does
// (live-proto AccountRequest::open): a JWS signed by the new key over
// canonical {gamertag, jwk, invite?, aud, iat}, for this server and recent.

import { b64urlDecode, canonicalJson, jwkThumbprint, parseJws } from "../src/state/verify";

export type OpenedAccount = { gamertag: string; jwk: { kty: "OKP"; crv: "Ed25519"; x: string; kid: string } } | { error: string };

/** How far `iat` may be from the server's clock, in seconds. */
export const MAX_SKEW_S = 5 * 60;

export async function openAccountRequest(compact: string, aud: string, nowSeconds: number): Promise<OpenedAccount> {
  const jws = parseJws(compact.trim());
  if (!jws) return { error: "malformed JWS" };
  const h = jws.header as { alg?: string; kid?: string; typ?: string };
  if (h.alg !== "EdDSA") return { error: "alg must be EdDSA" };
  if (h.typ !== "h2live-account") return { error: "typ must be h2live-account" };
  const raw = new TextDecoder().decode(b64urlDecode(compact.trim().split(".")[1]!));
  if (canonicalJson(JSON.parse(raw)) !== raw) return { error: "payload is not canonical JSON" };
  const body = jws.payload as { gamertag?: unknown; aud?: unknown; iat?: unknown; jwk?: { kty?: string; crv?: string; x?: string; kid?: string } };
  if (typeof body.gamertag !== "string" || !body.jwk?.x || body.jwk.kty !== "OKP" || body.jwk.crv !== "Ed25519") return { error: "bad body" };
  if (typeof body.aud !== "string" || typeof body.iat !== "number" || !Number.isInteger(body.iat)) return { error: "bad body: aud and iat are required" };
  const kid = await jwkThumbprint({ kty: "OKP", crv: "Ed25519", x: body.jwk.x });
  if ((body.jwk.kid && body.jwk.kid !== kid) || h.kid !== kid) return { error: "kid does not match the key" };
  const key = await crypto.subtle.importKey("jwk", { kty: "OKP", crv: "Ed25519", x: body.jwk.x }, { name: "Ed25519" }, false, ["verify"]);
  if (!(await crypto.subtle.verify({ name: "Ed25519" }, key, jws.signature, jws.signingInput))) return { error: "signature does not verify" };
  if (body.aud !== aud) return { error: "account request is for another server" };
  if (Math.abs(body.iat - nowSeconds) > MAX_SKEW_S) return { error: "account request iat is more than 5 minutes off" };
  return { gamertag: body.gamertag, jwk: { kty: "OKP", crv: "Ed25519", x: body.jwk.x, kid } };
}
