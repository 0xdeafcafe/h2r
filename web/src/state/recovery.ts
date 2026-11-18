// The sign-in code (the API calls it recovery): a TOTP secret the server
// keeps per account, so a new device (a browser, or the game) can sign in
// with the gamertag and the 6-digit code.
//
//   - sign-up (POST /v1/accounts) answers the first `recovery` object;
//   - GET  /v1/accounts/by-xuid/{xuid}/recovery (token lxm recovery.read)
//     reads it again, 404 when the account has none;
//   - POST /v1/accounts/by-xuid/{xuid}/recovery (token lxm recovery.write)
//     sets one up or replaces it (the old secret stops working);
//   - POST /v1/accounts/{gamertag}/recover with {request, code}: `request`
//     is a compact JWS {alg: "EdDSA", kid, typ: "h2live-recover"} over the
//     canonical JSON of {aud, gamertag, iat, jwk, nonce}, signed by a fresh
//     key that becomes the account's key, exactly as at sign-up.

import { gamertagError, generateKey, signCompact, type LocalAccount, type NewKey } from "./account";
import { b64urlEncode, canonicalJson } from "./verify";
import { issueToken } from "../admin/token";
import { ApiError } from "../api/http";

export const TYP_RECOVER = "h2live-recover";
export const RECOVERY_READ = "recovery.read";
export const RECOVERY_WRITE = "recovery.write";

export interface Recovery {
  /** RFC 4648 base32, no padding. */
  secret: string;
  otpauth: string;
  /** The current code, "123-456". */
  code: string;
  period: number;
  /** Unix seconds when `code` rolls over. */
  expires_at: number;
}

/** A `recovery` object from the server, or null when absent or malformed
 *  (an older server sends none). */
export function parseRecovery(v: unknown): Recovery | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  if (typeof o.secret !== "string" || !o.secret || typeof o.code !== "string" || typeof o.expires_at !== "number") return null;
  return {
    secret: o.secret,
    otpauth: typeof o.otpauth === "string" ? o.otpauth : "",
    code: formatCode(o.code),
    period: typeof o.period === "number" && o.period > 0 ? o.period : 30,
    expires_at: o.expires_at,
  };
}

// ---------- codes ----------

/** The digits of a typed code, spaces and dashes taken out. */
export function normalizeCode(s: string): string {
  return s.replace(/[\s-]/g, "");
}

/** "123456" or "123 456" as "123-456"; anything else as typed. */
export function formatCode(s: string): string {
  const d = normalizeCode(s);
  return /^\d{6}$/.test(d) ? `${d.slice(0, 3)}-${d.slice(3)}` : s;
}

export function codeError(s: string): string | null {
  return /^\d{6}$/.test(normalizeCode(s)) ? null : "the code is 6 digits";
}

/** How the code reads while hidden. */
export const MASKED_CODE = "••• •••";
/** How long a shown code stays shown. */
export const REVEAL_MS = 30_000;

/** The code as the screen shows it: itself while revealed (until
 *  `revealUntil`, a time in ms), masked otherwise. */
export function shownCode(code: string, revealUntil: number, nowMs: number): string {
  return nowMs < revealUntil ? code : MASKED_CODE;
}

/** A base32 secret in groups of four, for reading off a screen. */
export function groupSecret(secret: string): string {
  return secret.replace(/\s/g, "").replace(/(.{4})(?=.)/g, "$1 ");
}

/** Whole seconds until `r.code` rolls over, by this browser's clock,
 *  kept within one period so a skewed clock can't stall or spin it. */
export function secondsLeft(r: Pick<Recovery, "expires_at" | "period">, nowMs: number): number {
  return Math.max(0, Math.min(r.period, Math.ceil(r.expires_at - nowMs / 1000)));
}

const B32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

export function base32Encode(bytes: Uint8Array): string {
  let out = "", bits = 0, acc = 0;
  for (const b of bytes) {
    acc = (acc << 8) | b;
    bits += 8;
    while (bits >= 5) { out += B32[(acc >>> (bits - 5)) & 31]; bits -= 5; }
  }
  if (bits) out += B32[(acc << (5 - bits)) & 31];
  return out;
}

export function base32Decode(s: string): Uint8Array {
  const clean = s.replace(/[\s=]/g, "").toUpperCase();
  const out: number[] = [];
  let bits = 0, acc = 0;
  for (const c of clean) {
    const v = B32.indexOf(c);
    if (v < 0) throw new Error(`not base32: ${c}`);
    acc = ((acc << 5) | v) & 0xffff;
    bits += 5;
    if (bits >= 8) { out.push((acc >>> (bits - 8)) & 0xff); bits -= 8; }
  }
  return new Uint8Array(out);
}

/** RFC 6238 TOTP with HMAC-SHA1 (RFC 4226 truncation), `digits` long, at
 *  `nowS` seconds. The mock server's codes; the browser shows the server's. */
export async function totp(secret: Uint8Array, nowS: number, period = 30, digits = 6): Promise<string> {
  const counter = Math.floor(nowS / period);
  const msg = new Uint8Array(8);
  new DataView(msg.buffer).setBigUint64(0, BigInt(counter));
  const key = await crypto.subtle.importKey("raw", new Uint8Array(secret), { name: "HMAC", hash: "SHA-1" }, false, ["sign"]);
  const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, msg));
  const off = mac[mac.length - 1]! & 15;
  const bin = ((mac[off]! & 0x7f) << 24) | (mac[off + 1]! << 16) | (mac[off + 2]! << 8) | mac[off + 3]!;
  return String(bin % 10 ** digits).padStart(digits, "0");
}

// ---------- the recover request ----------

export interface RecoverBody {
  aud: string;
  gamertag: string;
  iat: number;
  jwk: { kty: "OKP"; crv: "Ed25519"; x: string };
  nonce: string;
}

function nonce16(): string {
  const b = new Uint8Array(16);
  crypto.getRandomValues(b);
  return b64urlEncode(b);
}

/** The signed `request` of a recover call: `key` (the new device key)
 *  asks to become `gamertag`'s key on the server `aud`, at `iat`. */
export async function signRecoverRequest(key: NewKey, gamertag: string, aud: string, iat: number, nonce: string = nonce16()): Promise<string> {
  const body: RecoverBody = { aud, gamertag, iat, jwk: { kty: "OKP", crv: "Ed25519", x: key.publicJwk.x }, nonce };
  return signCompact(key.privateKey, { alg: "EdDSA", kid: key.publicJwk.kid, typ: TYP_RECOVER }, canonicalJson(body));
}

export class RecoverError extends Error {
  constructor(readonly status: number, readonly code: string, message: string) { super(message); }
}

/** What to tell someone whose recover call was refused. */
export function recoverMessage(status: number, gamertag: string, serverMessage?: string): string {
  switch (status) {
    case 403: return `That code didn't work. Check it, or ${gamertag} may not have a sign-in code set up.`;
    case 429: return "Too many wrong codes. Wait a few minutes, then try again.";
    case 404: return `There's no ${gamertag} on this server.`;
    case 400: return `The server refused the request${serverMessage ? `: ${serverMessage}` : ""}.`;
    default: return `Couldn't sign in (HTTP ${status})${serverMessage ? `: ${serverMessage}` : ""}.`;
  }
}

/** Takes `gamertag` over on this browser: a new key, the code, and the
 *  account entry to keep, as after sign-up. */
export async function recoverAccount(
  apiRoot: string, serverBase: string, serverId: string, gamertag: string, code: string,
  fetchFn: typeof fetch = fetch, nowMs: number = Date.now(),
): Promise<LocalAccount> {
  const bad = gamertagError(gamertag);
  if (bad) throw new RecoverError(0, "invalid", `That gamertag won't work: ${bad}.`);
  const badCode = codeError(code);
  if (badCode) throw new RecoverError(0, "invalid", `Check the code: ${badCode}.`);
  const key = await generateKey();
  const request = await signRecoverRequest(key, gamertag, serverId, Math.floor(nowMs / 1000));
  const r = await fetchFn(`${apiRoot}/accounts/${encodeURIComponent(gamertag)}/recover`, {
    method: "POST",
    headers: { "content-type": "application/json", accept: "application/json" },
    body: JSON.stringify({ request, code: normalizeCode(code) }),
  });
  if (!r.ok) {
    let err = "http_" + r.status, msg: string | undefined;
    try { const b = await r.json(); if (typeof b?.error === "string") { err = b.error; msg = b.message; } } catch { /* not JSON */ }
    throw new RecoverError(r.status, err, recoverMessage(r.status, gamertag, msg));
  }
  const b = (await r.json()) as { xuid: string; gamertag: string; id: string };
  return {
    xuid: b.xuid, gamertag: b.gamertag, id: b.id, server: serverBase, server_id: serverId,
    kid: key.publicJwk.kid, key: key.d, x: key.publicJwk.x,
  };
}

// ---------- reading and setting up (signed in) ----------

async function fail(r: Response): Promise<never> {
  let code = "http_" + r.status, message = r.statusText;
  try { const b = await r.json(); if (typeof b?.error === "string") { code = b.error; message = b.message ?? message; } } catch { /* not JSON */ }
  throw new ApiError(r.status, code, message);
}

const recoveryUrl = (api: string, me: LocalAccount) => `${api}/accounts/by-xuid/${encodeURIComponent(me.xuid)}/recovery`;

/** The account's recovery now; null when it has none set up. */
export async function fetchRecovery(api: string, aud: string, me: LocalAccount, fetchFn: typeof fetch = fetch, nowMs: number = Date.now()): Promise<Recovery | null> {
  const token = await issueToken(me, aud, RECOVERY_READ, nowMs);
  const r = await fetchFn(recoveryUrl(api, me), { headers: { accept: "application/json", authorization: `Bearer ${token}` }, cache: "no-store" });
  if (r.status === 404) return null;
  if (!r.ok) return fail(r);
  const rec = parseRecovery(await r.json());
  if (!rec) throw new ApiError(500, "invalid", "the server's answer is not a recovery setup");
  return rec;
}

/** Sets recovery up, or replaces it: the old secret stops working. */
export async function setupRecovery(api: string, aud: string, me: LocalAccount, fetchFn: typeof fetch = fetch, nowMs: number = Date.now()): Promise<Recovery> {
  const token = await issueToken(me, aud, RECOVERY_WRITE, nowMs);
  const r = await fetchFn(recoveryUrl(api, me), { method: "POST", headers: { accept: "application/json", authorization: `Bearer ${token}` } });
  if (!r.ok) return fail(r);
  const rec = parseRecovery(await r.json());
  if (!rec) throw new ApiError(500, "invalid", "the server's answer is not a recovery setup");
  return rec;
}
