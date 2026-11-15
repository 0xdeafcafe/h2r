// Ed25519 signing for the mock, following docs/live-security.md §1-2:
// JWK keys named by RFC 7638 thumbprint, compact JWS (alg EdDSA) over the
// canonical JSON of the event body, and a per-stream hash chain.

import { createHash, generateKeyPairSync, sign as edSign, type KeyObject } from "node:crypto";
import { b64urlEncode, canonicalJson } from "../src/state/verify";

export interface SigningKey {
  kid: string;
  jwk: { kty: "OKP"; crv: "Ed25519"; x: string; kid: string };
  privateKey: KeyObject;
}

export function newSigningKey(): SigningKey {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const pub = publicKey.export({ format: "jwk" }) as { x: string };
  const thumb = createHash("sha256").update(canonicalJson({ crv: "Ed25519", kty: "OKP", x: pub.x })).digest();
  const kid = b64urlEncode(new Uint8Array(thumb));
  return { kid, jwk: { kty: "OKP", crv: "Ed25519", x: pub.x, kid }, privateKey };
}

const enc = new TextEncoder();

/** Compact JWS over the canonical JSON of `payload`. */
export function signJws(key: SigningKey, payload: unknown): string {
  const header = b64urlEncode(enc.encode(canonicalJson({ alg: "EdDSA", kid: key.kid })));
  const body = b64urlEncode(enc.encode(canonicalJson(payload)));
  const input = header + "." + body;
  const sig = edSign(null, Buffer.from(input), key.privateKey);
  return input + "." + b64urlEncode(new Uint8Array(sig));
}

/** sha256(prev_hash || jws) as hex; prev_hash is the raw bytes of the previous hex hash. */
export function chainHashSync(prev: string | null, jws: string): string {
  const h = createHash("sha256");
  if (prev) h.update(Buffer.from(prev, "hex"));
  h.update(jws);
  return h.digest("hex");
}
