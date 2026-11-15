import type { WireEvent } from "../src/api/types";

let pos = 0;
export function resetPos() { pos = 0; }
export function ev(type: string, stream: string, data: unknown, at = "2026-11-15T20:00:00.000Z"): WireEvent {
  pos += 1;
  return { position: pos, stream, version: pos, type, at, data };
}

// `ACCOUNT_VECTOR` in crates/live-proto/src/sign.rs: the RFC 8037 key signing
// {gamertag: "Sneaky Pete", aud: "localhost", iat: 1800000000}.
export const ACCOUNT_VECTOR =
  "eyJhbGciOiJFZERTQSIsImtpZCI6ImtQcktfcW14VldhWVZBOXd3QkY2SXVvM3ZWeno3VHhIQ1R3WEJ5Z3JTNGsiLCJ0eXAiOiJoMmxpdmUtYWNjb3VudCJ9." +
  "eyJhdWQiOiJsb2NhbGhvc3QiLCJnYW1lcnRhZyI6IlNuZWFreSBQZXRlIiwiaWF0IjoxODAwMDAwMDAwLCJqd2siOnsiY3J2IjoiRWQyNTUxOSIsImtpZCI6ImtQcktfcW14VldhWVZBOXd3QkY2SXVvM3ZWeno3VHhIQ1R3WEJ5Z3JTNGsiLCJrdHkiOiJPS1AiLCJ4IjoiMTFxWUFZS3hDcmZWU183VHlXUUhPZzdoY3ZQYXBpTWxyd0lhYVBjSFVSbyJ9fQ." +
  "CAbhCapHV4aPxyp0SgMosKYqEXCVZvD5fOSx8sUQo9IpOUII91YGE75JFkE2QN0aojf0I5zQH1lZN3ehfQOGBg";
