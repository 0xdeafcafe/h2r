// Account creation in the browser, matching live-proto's `AccountRequest`
// (crates/live/live-proto/src/sign.rs): a fresh Ed25519 key signs a compact JWS
// with header {alg: "EdDSA", kid, typ: "h2live-account"} over the RFC 8785
// canonical JSON of {gamertag, jwk, invite?, aud, iat}, POSTed as
// application/jose. `aud` is the server id from its well-known document
// and `iat` the time in seconds; the server wants it within 5 minutes of
// its own clock. `account_request_vector` in sign.rs is the fixed vector.
//
// The private key has to be exportable so it can be downloaded for `h2`,
// so it is kept as a JWK (its `d`) in IndexedDB, not as a non-extractable
// CryptoKey. Anyone with access to this browser profile can read it.

import { b64urlEncode, canonicalJson, jwkThumbprint } from "./verify";
import { parseRecovery, type Recovery } from "./recovery";

export const TYP_ACCOUNT = "h2live-account";

export interface PublicJwk { kty: "OKP"; crv: "Ed25519"; x: string; kid: string }

export interface NewKey { publicJwk: PublicJwk; d: string; privateKey: CryptoKey }

/** Gamertag rules from live-proto: 1..=15 ASCII letters, digits and single
 *  spaces, starting with a letter. Returns an error message or null. */
export function gamertagError(s: string): string | null {
  if (s.length < 1 || s.length > 15) return "1 to 15 characters";
  if (!/^[A-Za-z]/.test(s)) return "must start with a letter";
  if (!/^[A-Za-z][A-Za-z0-9]*( [A-Za-z0-9]+)*$/.test(s)) return "letters, digits and single spaces only";
  return null;
}

export async function generateKey(): Promise<NewKey> {
  const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
  const jwk = (await crypto.subtle.exportKey("jwk", pair.privateKey)) as JsonWebKey;
  return importSeed(jwk.d!, jwk.x!);
}

/** Rebuilds a key from its JWK `d` (and `x`). */
export async function importSeed(d: string, x: string): Promise<NewKey> {
  const privateKey = await crypto.subtle.importKey(
    "jwk", { kty: "OKP", crv: "Ed25519", d, x }, { name: "Ed25519" }, true, ["sign"],
  );
  const kid = await jwkThumbprint({ kty: "OKP", crv: "Ed25519", x });
  return { publicJwk: { kty: "OKP", crv: "Ed25519", x, kid }, d, privateKey };
}

const enc = new TextEncoder();

/** Compact JWS over raw payload bytes with header {alg, kid?, typ?}. */
export async function signCompact(key: CryptoKey, header: { alg: "EdDSA"; kid?: string; typ?: string }, payload: string): Promise<string> {
  const input = b64urlEncode(enc.encode(canonicalJson(header))) + "." + b64urlEncode(enc.encode(payload));
  const sig = new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, key, enc.encode(input)));
  return input + "." + b64urlEncode(sig);
}

export interface AccountRequestBody { gamertag: string; jwk: PublicJwk; invite?: string; aud: string; iat: number }

/** The signed `POST /v1/accounts` body, for the server whose id is `aud`,
 *  made at `iat` (seconds since the epoch). */
export async function signAccountRequest(key: NewKey, gamertag: string, aud: string, iat: number, invite?: string): Promise<string> {
  const body: AccountRequestBody = { gamertag, jwk: key.publicJwk, aud, iat };
  if (invite) body.invite = invite;
  return signCompact(key.privateKey, { alg: "EdDSA", kid: key.publicJwk.kid, typ: TYP_ACCOUNT }, canonicalJson(body));
}

/** What `POST /v1/accounts` answers; `recovery` from servers that set it up. */
export interface CreatedAccount { xuid: string; gamertag: string; id: string; recovery?: unknown }

/** One entry of `accounts.json`, the same fields `h2 live account create`
 *  writes, so `h2 live account import <file>` can take it. */
export interface LocalAccount {
  xuid: string;
  gamertag: string;
  id: string;
  server: string;
  server_id: string;
  kid: string;
  /** The 32-byte Ed25519 seed, base64url (the JWK `d`). */
  key: string;
  /** The public half, for this browser's convenience. */
  x: string;
}

export class SignUpError extends Error {
  constructor(readonly code: string, message: string) { super(message); }
}

/** Signs up: the account entry to keep, and the recovery setup to show
 *  once (null from a server that sends none). */
export async function signUp(
  apiRoot: string, serverBase: string, serverId: string, gamertag: string, invite?: string,
  fetchFn: typeof fetch = fetch, nowMs: number = Date.now(),
): Promise<{ account: LocalAccount; recovery: Recovery | null }> {
  const bad = gamertagError(gamertag);
  if (bad) throw new SignUpError("invalid", `That gamertag won't work: ${bad}.`);
  const key = await generateKey();
  const jws = await signAccountRequest(key, gamertag, serverId, Math.floor(nowMs / 1000), invite || undefined);
  const r = await fetchFn(apiRoot + "/accounts", { method: "POST", headers: { "content-type": "application/jose" }, body: jws });
  if (!r.ok) {
    let code = "http_" + r.status, message = r.statusText;
    try { const b = await r.json(); if (typeof b?.error === "string") { code = b.error; message = b.message ?? message; } } catch { /* */ }
    if (r.status === 409) message = "Someone already has that gamertag.";
    throw new SignUpError(code, message);
  }
  const created = (await r.json()) as CreatedAccount;
  return {
    account: {
      xuid: created.xuid, gamertag: created.gamertag, id: created.id, server: serverBase, server_id: serverId,
      kid: key.publicJwk.kid, key: key.d, x: key.publicJwk.x,
    },
    recovery: parseRecovery(created.recovery),
  };
}

export async function createAccount(
  apiRoot: string, serverBase: string, serverId: string, gamertag: string, invite?: string,
  fetchFn: typeof fetch = fetch, nowMs: number = Date.now(),
): Promise<LocalAccount> {
  return (await signUp(apiRoot, serverBase, serverId, gamertag, invite, fetchFn, nowMs)).account;
}

/** The file `h2 live account import` reads: an array like accounts.json,
 *  with the key as a private JWK (the form `h2` writes itself). */
export function accountsFile(a: LocalAccount): string {
  const { x, key, ...rest } = a;
  const entry = { ...rest, key: { kty: "OKP", crv: "Ed25519", x, d: key } };
  return JSON.stringify([entry], null, 2) + "\n";
}

// ---------- storage (IndexedDB) ----------

const DB = "h2live";
const STORE = "accounts";

function openDb(): Promise<IDBDatabase> {
  return new Promise((res, rej) => {
    const r = indexedDB.open(DB, 1);
    r.onupgradeneeded = () => r.result.createObjectStore(STORE, { keyPath: ["server", "xuid"] });
    r.onsuccess = () => res(r.result);
    r.onerror = () => rej(r.error);
  });
}

export async function saveAccount(a: LocalAccount): Promise<void> {
  const db = await openDb();
  await new Promise<void>((res, rej) => {
    const t = db.transaction(STORE, "readwrite");
    t.objectStore(STORE).put(a);
    t.oncomplete = () => res();
    t.onerror = () => rej(t.error);
  });
}

export async function loadAccounts(): Promise<LocalAccount[]> {
  if (typeof indexedDB === "undefined") return [];
  const db = await openDb();
  return new Promise((res, rej) => {
    const r = db.transaction(STORE).objectStore(STORE).getAll();
    r.onsuccess = () => res(r.result as LocalAccount[]);
    r.onerror = () => rej(r.error);
  });
}

export async function deleteAccount(a: LocalAccount): Promise<void> {
  const db = await openDb();
  await new Promise<void>((res, rej) => {
    const t = db.transaction(STORE, "readwrite");
    t.objectStore(STORE).delete([a.server, a.xuid]);
    t.oncomplete = () => res();
    t.onerror = () => rej(t.error);
  });
}
