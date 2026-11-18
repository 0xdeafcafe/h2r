// Account recovery for the mock (src/state/recovery.ts is the client):
//   GET  /v1/accounts/by-xuid/{xuid}/recovery   token lxm recovery.read
//   POST /v1/accounts/by-xuid/{xuid}/recovery   token lxm recovery.write
//   POST /v1/accounts/{gamertag}/recover        {request, code}, no token
// Codes are real RFC 6238 TOTP (HMAC-SHA1, 30 s, 6 digits), so an
// authenticator app scanning the dashboard's QR code agrees with the mock.
// Tokens here are checked against the account's key; five wrong codes in a
// row lock recovery for that gamertag for a minute, doubling each time.

import { HOST, type World } from "./world";
import { b64urlDecode, canonicalJson, jwkThumbprint, parseJws } from "../src/state/verify";
import { base32Encode, formatCode, normalizeCode, totp, TYP_RECOVER, type Recovery } from "../src/state/recovery";
import { MAX_SKEW_S } from "./account";

export interface Reply { status: number; body: unknown }
const ok = (body: unknown, status = 200): Reply => ({ status, body });
const fail = (status: number, error: string, message: string): Reply => ({ status, body: { error, message } });

const PERIOD = 30;
const MAX_WRONG = 5;

interface Lock { wrong: number; until: number; backoffS: number }

export class MockRecovery {
  private secrets = new Map<string, Uint8Array>();
  private locks = new Map<string, Lock>();
  private nonces = new Set<string>();
  constructor(private world: World, private nowMs: () => number = () => Date.now()) {}

  /** A new secret for `xuid` (the old one stops working); its recovery object. */
  async setup(xuid: string, gamertag: string): Promise<Recovery> {
    const secret = crypto.getRandomValues(new Uint8Array(20));
    this.secrets.set(xuid, secret);
    return this.view(secret, gamertag);
  }

  has(xuid: string): boolean { return this.secrets.has(xuid); }

  private async view(secret: Uint8Array, gamertag: string): Promise<Recovery> {
    const now = this.nowMs() / 1000;
    const b32 = base32Encode(secret);
    const label = encodeURIComponent(`h2 live (${HOST}):${gamertag}`);
    return {
      secret: b32,
      otpauth: `otpauth://totp/${label}?secret=${b32}&issuer=${encodeURIComponent("h2 live")}&algorithm=SHA1&digits=6&period=${PERIOD}`,
      code: formatCode(await totp(secret, now, PERIOD)),
      period: PERIOD,
      expires_at: (Math.floor(now / PERIOD) + 1) * PERIOD,
    };
  }

  /** The account a bearer token speaks for, checked against its key. */
  async caller(auth: string | null, xuid: string, lxm: string): Promise<{ ok: true } | { ok: false; reply: Reply }> {
    const raw = auth?.startsWith("Bearer ") ? auth.slice(7).trim() : "";
    const t = raw ? parseJws(raw) : null;
    const claims = t?.payload as { iss?: string; lxm?: string; aud?: string; exp?: number } | undefined;
    if (!t || !claims?.iss) return { ok: false, reply: fail(401, "unauthorized", "this needs Authorization: Bearer <token>") };
    if (claims.lxm !== lxm) return { ok: false, reply: fail(401, "unauthorized", `token lxm is not ${lxm}`) };
    if (claims.aud !== HOST) return { ok: false, reply: fail(401, "unauthorized", "token is for another server") };
    if (typeof claims.exp !== "number" || claims.exp < this.nowMs() / 1000) return { ok: false, reply: fail(401, "unauthorized", "token expired") };
    const a = this.world.accounts.get(xuid);
    if (!a) return { ok: false, reply: fail(404, "not_found", "no such account") };
    if (claims.iss !== `acct:${xuid}@${HOST}`) return { ok: false, reply: fail(403, "forbidden", "a token for another account") };
    if ((t.header as { kid?: string }).kid !== a.key.jwk.kid) return { ok: false, reply: fail(401, "unauthorized", "unknown key") };
    const key = await crypto.subtle.importKey("jwk", { kty: "OKP", crv: "Ed25519", x: a.key.jwk.x }, { name: "Ed25519" }, false, ["verify"]);
    if (!(await crypto.subtle.verify({ name: "Ed25519" }, key, t.signature, t.signingInput))) return { ok: false, reply: fail(401, "unauthorized", "token signature does not verify") };
    return { ok: true };
  }

  /** Opens a recover `request` (live-proto's shape): typ, canonical payload,
   *  kid = thumbprint of the carried key, signed by it, for this server, recent. */
  async openRequest(compact: string, gamertag: string): Promise<{ jwk: { kty: "OKP"; crv: "Ed25519"; x: string; kid: string } } | { error: string }> {
    const jws = parseJws(compact.trim());
    if (!jws) return { error: "malformed JWS" };
    const h = jws.header as { alg?: string; kid?: string; typ?: string };
    if (h.alg !== "EdDSA") return { error: "alg must be EdDSA" };
    if (h.typ !== TYP_RECOVER) return { error: `typ must be ${TYP_RECOVER}` };
    const raw = new TextDecoder().decode(b64urlDecode(compact.trim().split(".")[1]!));
    if (canonicalJson(JSON.parse(raw)) !== raw) return { error: "payload is not canonical JSON" };
    const b = jws.payload as { aud?: unknown; gamertag?: unknown; iat?: unknown; nonce?: unknown; jwk?: { kty?: string; crv?: string; x?: string } };
    if (b.jwk?.kty !== "OKP" || b.jwk.crv !== "Ed25519" || typeof b.jwk.x !== "string") return { error: "bad jwk" };
    if (typeof b.nonce !== "string" || b.nonce.length < 16) return { error: "bad nonce" };
    if (typeof b.iat !== "number" || !Number.isInteger(b.iat)) return { error: "bad iat" };
    const kid = await jwkThumbprint({ kty: "OKP", crv: "Ed25519", x: b.jwk.x });
    if (h.kid !== kid) return { error: "kid does not match the key" };
    const key = await crypto.subtle.importKey("jwk", { kty: "OKP", crv: "Ed25519", x: b.jwk.x }, { name: "Ed25519" }, false, ["verify"]);
    if (!(await crypto.subtle.verify({ name: "Ed25519" }, key, jws.signature, jws.signingInput))) return { error: "signature does not verify" };
    if (b.aud !== HOST) return { error: "request is for another server" };
    if (b.gamertag !== gamertag) return { error: "request is for another gamertag" };
    if (Math.abs(b.iat - this.nowMs() / 1000) > MAX_SKEW_S) return { error: "request iat is more than 5 minutes off" };
    if (this.nonces.has(b.nonce)) return { error: "request already used" };
    this.nonces.add(b.nonce);
    return { jwk: { kty: "OKP", crv: "Ed25519", x: b.jwk.x, kid } };
  }

  private async codeMatches(secret: Uint8Array, code: string): Promise<boolean> {
    const now = this.nowMs() / 1000;
    for (const d of [-1, 0, 1]) if ((await totp(secret, now + d * PERIOD, PERIOD)) === code) return true;
    return false;
  }

  async handle(req: Request, u: URL): Promise<Reply | null> {
    const path = u.pathname;
    let m = path.match(/^\/v1\/accounts\/by-xuid\/([0-9a-fA-F]{16})\/recovery$/);
    if (m) {
      const xuid = m[1]!.toLowerCase();
      if (req.method === "GET") {
        const c = await this.caller(req.headers.get("authorization"), xuid, "recovery.read");
        if (!c.ok) return c.reply;
        const s = this.secrets.get(xuid);
        return s ? ok(await this.view(s, this.world.accounts.get(xuid)!.gamertag)) : fail(404, "not_found", "no recovery set up");
      }
      if (req.method === "POST") {
        const c = await this.caller(req.headers.get("authorization"), xuid, "recovery.write");
        if (!c.ok) return c.reply;
        return ok(await this.setup(xuid, this.world.accounts.get(xuid)!.gamertag), 201);
      }
      return null;
    }
    m = path.match(/^\/v1\/accounts\/([^/]+)\/recover$/);
    if (m && req.method === "POST") {
      const tag = decodeURIComponent(m[1]!);
      const a = this.world.accountByTag(tag);
      if (!a) return fail(404, "not_found", "no such account");
      const lockKey = a.xuid, now = this.nowMs();
      const lock = this.locks.get(lockKey) ?? { wrong: 0, until: 0, backoffS: 60 };
      if (lock.until > now) return fail(429, "rate_limited", `too many wrong codes; try again in ${Math.ceil((lock.until - now) / 1000)} s`);
      let body: { request?: unknown; code?: unknown };
      try { body = await req.json(); } catch { return fail(400, "invalid", "expected JSON {request, code}"); }
      if (typeof body.request !== "string" || typeof body.code !== "string") return fail(400, "invalid", "expected JSON {request, code}");
      const opened = await this.openRequest(body.request, a.gamertag);
      if ("error" in opened) return fail(400, "invalid", opened.error);
      const secret = this.secrets.get(a.xuid);
      if (!secret || !(await this.codeMatches(secret, normalizeCode(body.code)))) {
        lock.wrong += 1;
        if (lock.wrong >= MAX_WRONG) { lock.until = now + lock.backoffS * 1000; lock.backoffS *= 2; lock.wrong = 0; }
        this.locks.set(lockKey, lock);
        return fail(403, "forbidden", "wrong code, or no recovery set up");
      }
      this.locks.delete(lockKey);
      (a.retired ??= []).push({ kid: a.key.kid, jwk: a.key.jwk, from: a.keyFrom, to: this.world.log.length + 1 });
      a.key = { kid: opened.jwk.kid, jwk: opened.jwk, privateKey: undefined as never };
      a.keyFrom = this.world.log.length + 1;
      // A log event at the new key's position, so it has a date (the real server logs the key change).
      this.world.append(`account-${a.xuid}`, [{ type: "AccountRecovered", data: { xuid: a.xuid, kid: opened.jwk.kid } }], null);
      return ok({ xuid: a.xuid, gamertag: a.gamertag, id: `acct:${a.xuid}@${HOST}` }, 201);
    }
    return null;
  }
}
