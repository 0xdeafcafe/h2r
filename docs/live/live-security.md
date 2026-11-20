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
**sign-in code** (the API and log call it recovery; people see "sign-in
code"): a TOTP (RFC 6238: HMAC-SHA1, 30 s steps, six digits,
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
- **Guessing.** Wrong codes count per gamertag and client IP. After five
  attempts, that pair is locked for a minute (429 with the time left).
  The lock doubles with each further wrong code up to a day. A correct
  code or a day without failures clears it. A wrong code's 403 says how
  many tries remain (`wrong code; N tries left`). Recovery has its own
  bucket of one request per second per IP with a burst of ten across all
  gamertags. An owner recovering from another IP has their own budget and
  backoff. With three codes valid at once, five guesses from one IP have
  odds of about 1 in 67,000 before the backoff begins.
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
  element in that array; its hash uses the batch JWS string. (The
  databases store the batch JWS once, in `batches`, and its events' rows
  name it; a read gives each event the whole JWS back, so nothing a
  verifier sees changes.)
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
- **WebSockets** for stream and ingest send the token as the first
  message when authentication is required. For co-op relay, both homes
  check the bearer token and session membership before upgrading. The
  relay rechecks membership and accepts refreshes from the same account.
  [Relay protocol](live-api.md#co-op-relay)
- **Reads are public by default**: viewing games, accounts and the event
  log needs no token, which is what the web app does. A server can switch
  that off (`--private`), and then readers need a token too.
- **Account creation** and **recovery** (§1) are the writes without a
  token. The creation request carries the new public key and is signed by it (proof of possession), with `aud`
  (the server id) and `iat` so it can't be replayed to another server. Servers can
  require an invite code (`--invite-only`).

The operator signs tokens with the server key for tasks such as
publishing title files and creating invites. Admin accounts can also
perform these tasks with self-issued tokens. The server checks an
explicit per-account grant when a non-admin account asks to create an
invite ([live-admin.md](live-admin.md) §5).

**Server invites.** An account sends an `invites.write` token and a signed
`InviteCreated {jti, exp}` event on the `invites` stream. The server
checks the caller's permission, matches the event author to the token
issuer, verifies the event with a current key, checks its time (within
five minutes) and rejects a replayed nonce. Each invite has a fresh
`jti` and expires within 30 days. Creation uses a separate rate bucket
per author. Non-admin accounts with the grant can make three at once
and admins ten, with one slot replenished per minute. A grant can also
limit the account's open invites. The server checks the cap and appends
the event in the same write sequence.

The server returns a generated code after the append. Only the server
has the key that derives the code from the invite id. A custom code goes
beside the signed event and the log stores its MAC. Operator invites use
server-signed creation events. Long codes signed by the server also
redeem.

An `invite-grants.write` token from an admin or the operator sets or
revokes the permission. The server signs the resulting account event,
including the caller's identity, and replays it at startup. A
revocation closes the account's open invites permanently. Redemption
checks that the invite is open and its maker still has permission. An
`InviteRevoked` event also closes a single invite. Admins can close any
invite and granted players can close their own.

Listing all invites and their open generated codes requires an
`admin.read` token. The `invites.read` scope gives a caller only their own
open invites and still requires invite permission.

**Signing a device out.** Each device's key is its sign-in, so ending one
is revoking its key: `KeyRevoked` (client-signed through `POST
/v1/events`, or server-signed for the account through the sign-ins
endpoints, live-api.md "Sign-ins"). The check above looks the key up in
memory on every request, so the revoked key's next token is refused (401)
with no session table and no extra lookup.

`lxm` values: `events.write`, `games.write` (game events and ingest),
`titles.write` (operator or admin), `invites.write` (operator, admin or
granted account), `invites.read` (own open invites), `invite-grants.write`
(admin or operator), `admin.read` (admin status and all invites), `recovery.read` and
`recovery.write` (the sign-in code), `signins.read` and `signins.write`
(the account's own sign-ins), and `read` (every
read, including the stream, on a `--private` server).

The server id is fixed on first start and stored with the server key,
because every author id contains it.

### Dedicated host identity

Live issues the host process one reusable Ed25519 JWT at boot. The issuer
is the Live server, the audience is its server ID, and the subject is
`host:<lease_id>`. The token expires `max_match_s + 300` seconds after
issuance. Its `lxm` array grants `hosts.register`, `hosts.report`,
`sessions.publish`, and `telemetry.ingest`. Registration and reporting
compare the verified subject’s lease ID to the route. Player and operator
routes keep their existing single-method token checks.

The private JWT also carries `event_key`, the base64url Ed25519 seed for
this lease’s event batches. Live derives it with HKDF from the server key
and the lease subject. The host uses the JWT as its HTTP or ingest bearer
and signs ordinary session and game batch JWSes with the lease key, using
the JWT subject as event author. Live checks the batch signature, scope
and resource owner. Session creation and game start also check the active
lease. The event log retains the signed batches. A hibernating ingest
socket retains only the subject and expiry. Keep the JWT and its event key
private for the lifetime of the lease.

A dedicated session carries the service subject in `host` and zero in the
legacy `host_xuid` field. Host heartbeats refresh its advertisement, and
only player XUIDs populate presence, session membership and Ranked results.
Raw events on the `hosts` stream are visible only to admins and the
operator. Roster members read their lease endpoint through
`GET /v1/hosts/{lease}`.
[Ranked admission](dedicated-host.md#routing-assignment-and-arbitration)

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
  playlist file inflates to about 1.46 MB). Ingest messages have a 16 KiB
  cap before authentication and throughout the session on both deployments.
  Writes have an in-memory budget per author and method. API GET and HEAD
  requests share a budget of 30 requests per second per client IP with a
  burst of 120. Both budgets return 429 when exhausted. The native server
  uses the peer address and trusts `CF-Connecting-IP` from loopback with a
  Cloudflare tunnel configured. The Worker uses Cloudflare's client IP.
  IPv6 addresses share limits within their /64 for reads and recovery.
  Requests with no trusted IP share a fallback bucket.
- **Map outline writes** require an admin or the operator with a
  `games.write` token. That role is required for both the first outline
  and any replacement. [Outline API](live-api.md#map-outline)
- **Content downloads** check read authorization and the blob's moderation
  status before streaming it from storage. The server can stream large
  packages without buffering the full download. [Content API](live-api.md#custom-maps)
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
