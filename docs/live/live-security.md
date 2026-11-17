# Live service: identity, signing and federation

Status: built except §5 (federation). Applies to [live.md](live.md) and the
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
  first start and kept in its data directory (mode 0600), never in the database.
  The public half is published (§4). It signs events the server writes
  itself (expiry, process managers) and the tokens it issues.
- **Account key.** Each account has its own key, generated on the user's
  machine by `h2 live account create` and stored with the local account
  (`live/accounts.json`, mode 0600). Only the public key goes to the server.
  The server never sees an account's private key, so a stolen database
  cannot forge a user's events.
- Key rotation: `KeyAdded` and `KeyRevoked` events on the account stream;
  a key is valid for events appended between those two positions.

### Recovery

A key lives on one device. To use the account on another (a second game
install, a browser) or after losing every device, the account has a
**recovery code**: a TOTP (RFC 6238: HMAC-SHA1, 30 s steps, six digits,
shown `123-456`), the same scheme authenticator apps use.

- **The secret.** Made by the server at account creation, 160 random
  bits. The server keeps it in the log, sealed (below): in `AccountCreated`
  as `recovery`, and in a server-written `RecoverySet {sealed}` when it is
  set up on an older account or reset. The creation reply carries it once,
  with the current code and an `otpauth://` URI
  (`RecoveryInfo {secret, otpauth, code, period, expires_at}`).
- **Showing a code.** Any signed-in device of the account reads the secret
  and current code with `GET /v1/accounts/by-xuid/{xuid}/recovery` and a
  `recovery.read` token (the web dashboard's recovery screen, `h2 live
  account code`, and the game's sign-in prompt when it makes an account). Enrolling an authenticator
  app (the secret as base32, or the URI as a QR code) means a code is at
  hand with no device left. `POST` on the same path (`recovery.write`)
  makes a new secret: set up on an account from before recovery, or reset
  (the old one stops working at once).
- **Recovering.** The new device makes a key and sends
  `POST /v1/accounts/{gamertag}/recover` with
  `{"request": <JWS>, "code": "123456"}`. The JWS (`typ: h2live-recover`)
  is signed by the new key over `{gamertag, jwk, aud, iat, nonce}`, proving
  it holds the key, for this server, recently. The code goes beside it, so
  the logged proof holds no code. The server accepts the code for the
  current step or one either side, each step once per account; appends a
  server-signed `KeyAdded {kid, jwk, proof, via: "totp"}`; and answers
  `{xuid, gamertag, id}`. No token and no invite: the code is the
  credential.
- **Guessing.** Wrong codes count per gamertag: after 5, the gamertag is
  locked for a minute, doubling with every further wrong code up to a day
  (429 with the time left); a right code clears it; a gamertag quiet for a
  day starts over. A wrong code's 403 says how many are left before the
  lock (`wrong code; N tries left`), which the in-game prompt shows. Sign-up and recovery also share the server-wide
  creation rate limit. With three codes valid at once, the first five
  guesses succeed with odds of about 1 in 67,000; after that each guess
  costs exponentially more time.
- **Sealing.** Secrets are sealed with XChaCha20-Poly1305 (a random
  24-byte nonce per value, stored as base64url of nonce and ciphertext with
  its tag) under a key derived from the server key with HKDF-SHA256 (info
  `h2live-recovery-seal-v1`). The log (SQLite, Postgres or files) holds only that.
  The server key never leaves the data directory. Invite codes use another
  key from the same derivation (info `h2live-invite-code-v1`).

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

- **Self-issued token.** For each request (single use: a `jti` is accepted once), the
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
- **Account creation** and **recovery** (§1) are the writes without a
  token. The creation request carries the new public key and is signed by it (proof of possession), with `aud`
  (the server id) and `iat` so it can't be replayed to another server. Servers can
  require an invite code (`--invite-only`).

Server-issued tokens (signed by the server key) are used between servers
(§5) and by the operator: publishing title files and creating invites need a
token signed by the server key, so they run on the server's machine, or a
token from an admin account ([live-admin.md](live-admin.md)).

**Invites from an admin account.** An invite code is always a JWS signed
by the server key (`{aud, jti, exp}`, typ `h2live-invite`), so account
creation checks it the same way whoever asked for it. An admin account
(the browser's dashboard) asks with an `invites.write` token *and* an
`InviteCreated` event `{jti, exp}` on the `invites` stream signed by its
own key. The server checks that the author is an admin now, rate-limits
it (per author: a burst of 10, then one a minute), checks the event like
any signed write (its key, `at` within 5 minutes, a fresh nonce), that
`exp` is at most 30 days away and the `jti` new, appends the admin's JWS
as it is, and only then signs the code. So:

- the log holds, for every admin invite, who asked for it, signed by them;
  the code itself is not in the log (it is a server signature over the
  `jti` and `exp`, which are);
- the server key never leaves the server, and an account key cannot mint
  a code on its own: the server signs only after the audit event is
  appended;
- `InviteRevoked` `{jti}` (also admin-signed) ends an open one, and an
  invite from an account that is no longer an admin is refused at sign-up,
  so revoking an admin also kills the invites it made. The operator's own
  invites (`h2 live invite create`) are not logged and work as before.

What it adds to an admin's power: inviting people, which on an
`--invite-only` server is who may sign up. It does not let an admin make
admins (grant and revoke stay operator-only), and a stolen admin key
could mint at most ten invites, then one a minute, each visible in the
log and revocable.

`lxm` values: `events.write`, `games.write` (game events and ingest),
`titles.write` (operator or admin), `invites.write` (operator or admin),
`admin.read` (an admin's status and invite reads), and `read` (every
read, including the stream, on a `--private` server).

The server id is fixed on first start and stored with the server key,
because every author id contains it.

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
- Request bodies are capped (64 KiB normally, title files 2 MiB because the
  playlist file inflates to about 1.46 MB, ingest
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
- Recovery is the server's to grant. It holds every account's TOTP secret
  (sealed with a key derived from its own), so whoever has the server key,
  or runs the server, can add a key to any account. That was already
  true, since the server signs the `KeyAdded` of every new account, and a
  log-only reader (a stolen database dump) still cannot: the secrets in it
  are sealed. The trade is deliberate: a six-digit code a person can type
  on a gamepad, checked online with a lockout, instead of a long secret
  only the user holds. A recovery code also lets anyone who sees it add a
  device within its 90-second window; it is used once, then refused.
