# Live service: identity, signing and federation

Status: design, being implemented. Applies to [live.md](live.md) and the
contract in [live-api.md](live-api.md).

Goals:

- Every event says who made it and carries that author's signature, so
  anyone holding the log (another server, an agent, the web app in a
  browser) can check it without asking anyone.
- Requests are authorised with signed tokens checked locally: no session
  lookup, no call to another service per request.
- Any client can point at any server by base URL, and the identity scheme
  works when servers federate later.

## 1. Keys

All signatures are Ed25519 (fast to sign and verify, small keys, available
in Rust through `ed25519-dalek` and in browsers through WebCrypto).

- **Server key.** Each server has a long-lived signing key, generated on
  first start and kept in its data directory (mode 0600), never in Postgres.
  The public half is published (§4). It signs events the server writes
  itself (expiry, process managers) and the tokens it issues.
- **Account key.** Each account has its own key, generated on the user's
  machine by `h2 live account create` and stored with the local account
  (`live/accounts.json`, mode 0600). Only the public key goes to the server.
  The server never sees an account's private key, so a stolen database
  cannot forge a user's events.
- Key rotation: `KeyAdded` and `KeyRevoked` events on the account stream;
  a key is valid for events appended between those two positions.

Public keys travel as JWK (`{"kty":"OKP","crv":"Ed25519","x":"..."}`) and
are named by their RFC 7638 thumbprint (`kid`).

## 2. Signed events

The client signs what it means; the server orders it.

1. The author builds the event body: `{ stream, type, data, author, kid,
   at, nonce }`. `author` is `acct:<xuid>@<server host>` or
   `server:<host>`.
2. It signs the body's canonical JSON (RFC 8785, JCS) and sends it as a
   JWS in compact form with `alg: EdDSA` and `kid`.
3. The server checks the signature against the author's registered key
   (an in-memory projection, so no I/O), checks the author may write to that
   stream (an account writes its own streams and games it is in), checks
   `at` is within 5 minutes and `nonce` is new, then appends.
4. On append the server adds `position`, `version`, and a hash chain:
   `hash = sha256(prev_hash || jws)` per stream, so a missing or altered
   event in a stream is detectable. The stored record is the author's JWS
   untouched plus those fields.

Exact rules:

- **Hash chain.** `hash = hex(sha256(prev || utf8(jws)))`, where `prev` is
  the raw 32 bytes of the previous event's hash in the same stream, and
  empty for the stream's first event.
- **Batches.** A batch is one JWS whose payload is a JSON array of event
  bodies. Each stored event keeps the batch `jws` plus `jws_index`, its
  element in that array; its hash uses the batch JWS string.
- **Time.** The body's `at` is the author's clock; the wire `at` is the same
  value, copied. The server's append time is not part of the event.

The wire event in live-api.md gains `author`, `kid`, `jws`, `jws_index`
(batches only) and `hash`.
Readers that don't care ignore them; readers that do verify the JWS with the
author's public key from §4.

**Telemetry frames** (up to 15 Hz) are not signed one by one. The ingest
WebSocket is authorised by a token (§3) and the frames inherit it. The 1 Hz
`GameFrame` events recorded from them are written by the server and
signed by it, with the ingesting author named in `data.source`. Game events
from the ingest socket (kills, scores) are signed by the console's account
key, in batches: one JWS over an array of events, verified once.

## 3. Request tokens

No passwords. Clients prove they hold an account key by signing a short
JWT themselves, the way AT Protocol service auth works.

- **Self-issued token.** For each request (or each minute, reused), the
  client signs a JWT with its account key: `iss` = author id, `aud` = the
  server's id (§4), `iat`, `exp` at most 60 s later, `jti` random, and
  `lxm` naming the method (`games.write`, `accounts.read`, ...). Sent as
  `Authorization: Bearer <jwt>`.
- **Server check.** Parse header, look up `kid` in memory, verify EdDSA,
  check `aud`, `exp`, `lxm`, and reject a `jti` seen in the last 2 minutes
  (an in-memory set that expires itself). No database call.
- **WebSockets** send the token as the first message (browsers cannot set
  headers on a WebSocket); the server closes the socket when the token's
  `exp` passes unless the client sends a fresh one.
- **Reads are public by default**: viewing games, accounts and the event
  log needs no token, which is what the web app does. A server can switch
  that off (`--private`), and then readers need a token too.
- **Account creation** is the one write without a token. The request carries
  the new public key and is signed by it (proof of possession). Servers can
  require an invite code (`--invite-only`).

Server-issued tokens (signed by the server key) are used only between
servers (§5).

## 4. Base URL and discovery

Every client takes a base URL: `--server`/`H2_LIVE_SERVER` for the CLI and
the game, a server switcher in the web app. Everything about a server is
found from that URL:

`GET /.well-known/h2live.json`

```json
{ "id": "live.example.net", "api": "https://live.example.net/v1",
  "keys": [ { "kty": "OKP", "crv": "Ed25519", "x": "...", "kid": "..." } ],
  "software": "h2recomp live-server 0.1", "federation": false }
```

Account keys are at `GET /v1/accounts/{gamertag}/keys` (current and
revoked, with the positions they were valid between). Clients cache keys by
`kid`; a signature with an unknown `kid` triggers one fetch, then is cached.

An account's home is the server that created it: `Sneaky Pete@live.example.net`.
Gamertags are unique per server, and XUIDs are assigned by the home server.
The game only ever sees the XUID and gamertag.

## 5. Federation (later)

Not built yet; the formats above are chosen so it can be.

- Servers follow each other's logs over `/v1/stream` and verify every event
  with the author's home server keys. A server never re-signs someone else's
  event; it stores the original JWS.
- A user plays on a remote server by presenting a token signed by their
  account key. The remote server fetches their keys from the home server
  once.
- XUID collisions across servers: the game needs a 64-bit XUID unique within
  a session. A remote account is mapped to a local XUID on first contact
  (`RemoteAccountLinked` event), so the game never sees a collision.

## 6. Transport and limits

- The server binds to loopback by default. Binding anywhere else requires TLS
  (`--tls-cert` and `--tls-key`, rustls) or an explicit `--insecure-http`,
  and clients refuse to send tokens over plain HTTP to a non-loopback host.
- Request bodies are capped (64 KiB normally, title files 64 KiB, ingest
  messages 16 KiB), and each author is rate-limited per method in memory.
- CORS: public reads allow any origin, so a web app served elsewhere can
  read any server; writes allow none.
- Postgres credentials come only from the environment or the `--db` URL;
  they are never logged.

## 7. What this does not protect

- A user's own machine. The account key is a file on disk; anyone who can read
  it can play as that user. It is mode 0600, and that is all.
- Cheating in the game. A signed event proves who sent it, not that the game
  played fairly. The recomp has no anti-cheat.
- A malicious server can drop or reorder events, or refuse to serve them. It
  cannot forge another user's events or rewrite one already signed, and
  the per-stream hash chain shows when something is missing.
