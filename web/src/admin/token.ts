// Self-issued request tokens (docs/live-security.md §3), the browser's
// first use of them: the admin area's saves sign a 60-second JWT with the
// signed-in account's key, exactly as live-proto's `issue_token` does
// (header {alg, kid, typ: "JWT"} over the canonical JSON of
// {aud, exp, iat, iss, jti, lxm}).

import { importSeed, signCompact, type LocalAccount } from "../state/account";
import { b64urlEncode, canonicalJson } from "../state/verify";

export const TYP_TOKEN = "JWT";
export const TOKEN_LIFETIME_S = 60;

/** 128 random bits, base64url: the single-use `jti`. */
export function randomId(): string {
  const b = new Uint8Array(16);
  crypto.getRandomValues(b);
  return b64urlEncode(b);
}

export interface TokenClaims {
  iss: string;
  aud: string;
  iat: number;
  exp: number;
  jti: string;
  lxm: string;
}

/** The claims a token for one request carries, at `nowMs` (milliseconds). */
export function tokenClaims(me: LocalAccount, aud: string, method: string, nowMs: number): TokenClaims {
  const iat = Math.floor(nowMs / 1000);
  return { iss: me.id, aud, iat, exp: iat + TOKEN_LIFETIME_S, jti: randomId(), lxm: method };
}

/** A signed token for one request. The server takes each `jti` once. */
export async function issueToken(
  me: LocalAccount,
  aud: string,
  method: string,
  nowMs: number = Date.now(),
): Promise<string> {
  const key = await importSeed(me.key, me.x);
  return signCompact(
    key.privateKey,
    { alg: "EdDSA", kid: key.publicJwk.kid, typ: TYP_TOKEN },
    canonicalJson(tokenClaims(me, aud, method, nowMs)),
  );
}
