// The mock's friends and teams: the docs/live-api.md "Friends" and "Teams"
// reads, `POST /v1/events` for signed friend and team events (checked
// against the author's key and token, kept as the author's own JWS), a
// few seeded teams and friendships, and seeded players who answer a new
// web account (a friend request and a team invitation arrive a moment
// after sign-up, and requests to them are accepted) so the live updates
// can be seen.

import type { WireEvent } from "../src/api/types";
import { emptyFriends, normalizeTeam, OFFICER, MEMBER, LEADER, PRIV, can, roleOf, teamWrites, friendWrites, type FriendsDoc, type Team, type EventWrite } from "../src/api/social";
import { applyFriendEvent, applyTeamEvent, friendCounterpart, teamIdOf } from "../src/state/social-model";
import { authorXuid, b64urlDecode, canonicalJson, parseJws } from "../src/state/verify";
import { chainHashSync } from "./sign";
import { HOST, type World } from "./world";

type Account = World["accounts"] extends Map<string, infer A> ? A : never;
type Reply = { status: number; body: unknown };

const json = (body: unknown, status = 200): Reply => ({ status, body });
const fail = (status: number, error: string, message: string, team_error?: string): Reply => ({ status, body: { error, message, ...(team_error ? { team_error } : {}) } });

export class Social {
  private friends = new Map<string, FriendsDoc>();
  readonly teams = new Map<string, Team>();

  constructor(private world: World) {}

  private tagOf = (x: string) => this.world.accounts.get(x)?.gamertag ?? x;

  /** Folds one appended event into the projections. */
  fold(ev: WireEvent) {
    if (ev.type.startsWith("Friend")) {
      const d = ev.data as Record<string, string>;
      for (const x of new Set([d.from, d.to, d.by, d.friend].filter(Boolean))) {
        if (!friendCounterpart(x!, ev)) continue;
        this.friends.set(x!, applyFriendEvent(this.friendsOf(x!), x!, ev, this.tagOf));
      }
    } else if (ev.type.startsWith("Team") && ev.stream.startsWith("team-")) {
      const id = teamIdOf(ev)!;
      const t = applyTeamEvent(this.teams.get(id) ?? null, ev, this.tagOf);
      if (t) this.teams.set(id, t);
    }
  }

  friendsOf(x: string): FriendsDoc {
    return this.friends.get(x) ?? emptyFriends();
  }

  /** An event by a seeded account, signed by its own key. */
  private bySeeded(a: Account, w: EventWrite) {
    const ev = this.world.append(w.stream, [{ type: w.type, data: w.data }], a)[0]!;
    this.fold(ev);
    if (w.type === "TeamMemberRecruited") this.recruitNote(w, a);
  }

  private recruitNote(w: EventWrite, by: Account | string) {
    const d = w.data as { id: string; target: { xuid: string } };
    const from = typeof by === "string" ? by : by.xuid;
    this.world.append(`account-${d.target.xuid}`, [{ type: "TeamRecruitReceived", data: { team_id: d.id, to: d.target.xuid, from } }], null);
  }

  /** Appends a client's signed event as is: its JWS, its `at`. */
  private appendSigned(body: { stream: string; type: string; data: unknown; author: string; kid: string; at: string }, jws: string): WireEvent {
    const w = this.world;
    const st = w.streams.get(body.stream) ?? { version: 0, hash: null };
    st.version += 1;
    st.hash = chainHashSync(st.hash, jws);
    w.streams.set(body.stream, st);
    const ev: WireEvent = {
      position: w.log.length + 1, stream: body.stream, version: st.version, type: body.type, at: body.at,
      data: body.data, author: body.author, kid: body.kid, jws, hash: st.hash,
    };
    w.log.push(ev);
    if (w.live) for (const l of w.listeners) l.onEvent(ev);
    return ev;
  }

  // ---------- seed ----------

  seed() {
    const acc = [...this.world.accounts.values()].filter((a) => !a.external);
    const by = (tag: string) => acc.find((a) => a.gamertag === tag)!;
    const pairs: Array<[string, string]> = [["Sneaky Pete", "Mochi"], ["Sneaky Pete", "Noodle"], ["Mochi", "Biscuit"], ["Pixel Pop", "Captain Waffles"], ["Noodle", "Zoomer"], ["Tofu Tank", "Sneaky Pete"]];
    for (const [a, b] of pairs) {
      this.bySeeded(by(a), friendWrites.request(by(a).xuid, by(b).xuid));
      this.bySeeded(by(b), friendWrites.accept(by(b).xuid, by(a).xuid));
    }
    this.bySeeded(by("Lemon Drop"), friendWrites.request(by("Lemon Drop").xuid, by("Sneaky Pete").xuid));
    const teams: Array<{ name: string; motto: string; description: string; url?: string; lead: string; officers: string[]; members: string[]; invited?: string[] }> = [
      { name: "Breakfast Club", motto: "First on the field, last to leave.", description: "Pancakes before every match. We run team slayer on weekend evenings and are always after a sixth.", url: "example.net/breakfast", lead: "Pancake Pal", officers: ["Biscuit"], members: ["Lemon Drop", "Sprinkles", "Mochi"], invited: ["Bean Machine"] },
      { name: "Moss Collective", motto: "Slow and steady holds the hill.", description: "Objective players. King of the hill and territories.", lead: "Moss", officers: [], members: ["Grumbles", "Tofu Tank"] },
      { name: "Sneaky Squad", motto: "You never heard us.", description: "", lead: "Sneaky Pete", officers: ["Noodle"], members: ["Zoomer", "Captain Waffles", "Pixel Pop"] },
    ];
    let n = 1;
    for (const t of teams) {
      const id = "000a" + (0x5eed00000000 + n++).toString(16).padStart(12, "0");
      const lead = by(t.lead);
      this.bySeeded(lead, teamWrites.create(id, { name: t.name, motto: t.motto, description: t.description, url: t.url ?? "" }));
      for (const [list, privileges] of [[t.officers, OFFICER], [t.members, MEMBER], [t.invited ?? [], MEMBER]] as const) {
        for (const tag of list) {
          const who = by(tag);
          this.bySeeded(lead, teamWrites.recruit(id, { xuid: who.xuid, gamertag: who.gamertag }, privileges));
          if (list !== t.invited) this.bySeeded(who, teamWrites.answer(id, true));
        }
      }
    }
  }

  /** A web sign-up gets a friend request and a team invitation shortly after. */
  welcome(xuid: string) {
    const a = this.world.accounts.get(xuid);
    if (!a?.external) return;
    const seeded = [...this.world.accounts.values()].filter((x) => !x.external);
    const mochi = seeded.find((x) => x.gamertag === "Mochi");
    setTimeout(() => {
      if (mochi && !this.related(mochi.xuid, xuid)) this.bySeeded(mochi, friendWrites.request(mochi.xuid, xuid));
    }, 2500);
    setTimeout(() => {
      const team = [...this.teams.values()].find((t) => !t.deleted && t.properties.name === "Breakfast Club");
      const lead = team ? this.world.accounts.get(team.creator) : undefined;
      if (team && lead && !roleOf(team, xuid)) this.bySeeded(lead, teamWrites.recruit(team.id, { xuid, gamertag: a.gamertag }, MEMBER));
    }, 4000);
  }

  private related(a: string, b: string) {
    const d = this.friendsOf(a);
    return d.friends.some((f) => f.xuid === b) || d.requests_in.some((r) => r.xuid === b) || d.requests_out.some((r) => r.xuid === b);
  }

  /** Seeded players answer what a web account does to them. */
  private respond(ev: WireEvent) {
    const d = ev.data as Record<string, string>;
    if (ev.type === "FriendRequested") {
      const to = this.world.accounts.get(d.to!);
      if (to && !to.external) setTimeout(() => { if (this.friendsOf(to.xuid).requests_in.some((r) => r.xuid === d.from)) this.bySeeded(to, friendWrites.accept(to.xuid, d.from!)); }, 2500);
    }
    if (ev.type === "TeamMemberRecruited") {
      const target = (ev.data as { target?: { xuid?: string } }).target?.xuid;
      const who = target ? this.world.accounts.get(target) : undefined;
      const id = teamIdOf(ev)!;
      if (who && !who.external) setTimeout(() => { const t = this.teams.get(id); if (t?.recruits[who.xuid]) this.bySeeded(who, teamWrites.answer(id, true)); }, 3000);
    }
  }

  // ---------- writes ----------

  private async verify(a: Account, compact: string, typ: string): Promise<{ payload: unknown } | string> {
    const p = parseJws(compact);
    if (!p) return "malformed JWS";
    const h = p.header as { alg?: string; kid?: string; typ?: string };
    if (h.alg !== "EdDSA") return "alg must be EdDSA";
    if (h.typ !== typ) return `typ must be ${typ}`;
    if (h.kid !== a.key.jwk.kid) return "unknown key";
    const key = await crypto.subtle.importKey("jwk", { kty: "OKP", crv: "Ed25519", x: a.key.jwk.x }, { name: "Ed25519" }, false, ["verify"]);
    if (!(await crypto.subtle.verify({ name: "Ed25519" }, key, p.signature, p.signingInput))) return "signature does not verify";
    return { payload: p.payload };
  }

  async postEvent(compact: string, authorization: string | null): Promise<Reply> {
    compact = compact.trim();
    const p = parseJws(compact);
    const body = p?.payload as { stream?: string; type?: string; data?: unknown; author?: string; kid?: string; at?: string; nonce?: string } | undefined;
    if (!body || typeof body.stream !== "string" || typeof body.type !== "string" || typeof body.author !== "string" || typeof body.at !== "string") return fail(400, "invalid", "expected a signed event body");
    if (canonicalJson(body) !== new TextDecoder().decode(b64urlDecode(compact.split(".")[1]!))) return fail(400, "invalid", "payload is not canonical JSON");
    const x = authorXuid(body.author);
    const a = x ? this.world.accounts.get(x) : undefined;
    if (!a || body.author !== `acct:${a.xuid}@${HOST}`) return fail(400, "invalid", "unknown author");
    const ok = await this.verify(a, compact, "h2live-event");
    if (typeof ok === "string") return fail(400, "invalid", ok);
    // The token: signed by the same key, for events.write, not expired.
    const tok = authorization?.replace(/^Bearer\s+/i, "") ?? "";
    const t = await this.verify(a, tok, "JWT");
    if (typeof t === "string") return fail(401, "unauthorized", `token: ${t}`);
    const claims = t.payload as { lxm?: string; exp?: number; aud?: string };
    if (claims.lxm !== "events.write" || claims.aud !== HOST || !claims.exp || claims.exp * 1000 < Date.now()) return fail(401, "unauthorized", "token is not for events.write here");
    if (Math.abs(Date.parse(body.at) - Date.now()) > 5 * 60_000) return fail(400, "invalid", "event time is more than 5 minutes off");

    const bad = body.type.startsWith("Friend") ? this.checkFriend(a, body as Required<typeof body>) : body.type.startsWith("Team") ? this.checkTeam(a, body as Required<typeof body>) : fail(400, "invalid", "the mock takes friend and team events here");
    if (bad) return bad;
    const ev = this.appendSigned(body as Required<typeof body>, compact);
    this.fold(ev);
    if (ev.type === "TeamMemberRecruited") this.recruitNote({ stream: ev.stream, type: ev.type, data: ev.data }, a.xuid);
    this.respond(ev);
    return json({ position: ev.position }, 201);
  }

  private checkFriend(a: Account, b: { stream: string; type: string; data: unknown }): Reply | null {
    if (b.stream !== `account-${a.xuid}`) return fail(403, "invalid", "friend events go on your own account stream");
    const d = (b.data ?? {}) as Record<string, string>;
    const me = a.xuid;
    const other = friendCounterpart(me, { type: b.type, data: d });
    if (!other || !this.world.accounts.has(other)) return fail(404, "not_found", "no such account");
    if (other === me) return fail(400, "invalid", "that's you");
    const doc = this.friendsOf(me);
    const friends = doc.friends.some((f) => f.xuid === other);
    const inbound = doc.requests_in.some((r) => r.xuid === other);
    const outbound = doc.requests_out.some((r) => r.xuid === other);
    switch (b.type) {
      case "FriendRequested": return d.from !== me ? fail(400, "invalid", "from must be you") : friends || inbound || outbound ? fail(409, "conflict", "already friends or a request is pending") : null;
      case "FriendAccepted": case "FriendDeclined": return d.to !== me ? fail(400, "invalid", "only the recipient answers") : inbound ? null : fail(409, "conflict", "no pending request");
      case "FriendRemoved": return d.by !== me ? fail(400, "invalid", "by must be you") : friends || inbound || outbound ? null : fail(409, "conflict", "nothing to remove");
      default: return fail(400, "invalid", "unknown friend event");
    }
  }

  private checkTeam(a: Account, b: { stream: string; type: string; data: unknown }): Reply | null {
    const d = (b.data ?? {}) as Record<string, unknown>;
    const id = String(d.id ?? "").toLowerCase();
    if (b.stream !== `team-${id}`) return fail(400, "invalid", "stream and id disagree", "Invalid");
    const me = a.xuid;
    const t = this.teams.get(id);
    const err = (status: number, te: string, m: string) => fail(status, status === 409 ? "conflict" : status === 404 ? "not_found" : status === 403 ? "forbidden" : "invalid", m, te);
    const mine = [...this.teams.values()].filter((x) => !x.deleted && (x.members[me] || x.recruits[me])).length;
    const nameTaken = (name: string) => [...this.teams.values()].some((x) => !x.deleted && x.id !== id && x.properties.name.toLowerCase() === name.toLowerCase());
    const props = normalizeTeam({ id, properties: d.properties })?.properties;
    if (b.type === "TeamCreated") {
      if (t) return err(409, "AlreadyExists", "that id is taken");
      if (!props?.name.trim() || props.name.length > 15) return err(400, "Invalid", "bad name");
      if (nameTaken(props.name)) return err(409, "AlreadyExists", "name taken");
      if (mine >= 8) return err(409, "UserTeamsFull", "eight teams already");
      if ((d.member as { privileges?: number })?.privileges !== LEADER) return err(400, "Invalid", "the creator leads");
      return null;
    }
    if (!t || t.deleted) return err(404, "NotFound", "no such team");
    const user = typeof d.user === "string" ? d.user.toLowerCase() : "";
    switch (b.type) {
      case "TeamPropertiesChanged":
        if (!can(t, me, PRIV.modifyTeam)) return err(403, "Privileges", "no");
        if (!props?.name.trim() || props.name.length > 15) return err(400, "Invalid", "bad name");
        return nameTaken(props.name) ? err(409, "AlreadyExists", "name taken") : null;
      case "TeamDeleted": return me === t.creator ? null : err(403, "Privileges", "only the leader");
      case "TeamMemberRecruited": {
        const target = (d.target as { xuid?: string } | undefined)?.xuid?.toLowerCase() ?? "";
        if (!can(t, me, PRIV.recruit)) return err(403, "Privileges", "no");
        if (!this.world.accounts.has(target)) return err(404, "UserNotFound", "no such account");
        if (t.members[target]) return err(409, "AlreadyMember", "already a member");
        if (t.recruits[target]) return err(409, "Pending", "already invited");
        const theirs = [...this.teams.values()].filter((x) => !x.deleted && (x.members[target] || x.recruits[target])).length;
        if (theirs >= 8) return err(409, "UserTeamsFull", "they're on eight teams");
        const priv = (d.properties as { privileges?: number } | undefined)?.privileges ?? 0;
        const mineP = t.members[me]!.properties.privileges;
        return me !== t.creator && (priv & ~mineP) !== 0 ? err(403, "Privileges", "can't grant that") : null;
      }
      case "TeamRecruitAccepted": case "TeamRecruitDeclined": return t.recruits[me] ? null : err(403, "NotMember", "no invitation");
      case "TeamMemberPropertiesChanged":
        if (!t.members[user]) return err(404, "UserNotFound", "not a member");
        if (user === t.creator) return err(403, "Privileges", "the leader's rank is fixed");
        return can(t, me, PRIV.modifyMember) ? null : err(403, "Privileges", "no");
      case "TeamMemberRemoved":
        if (!t.members[user] && !t.recruits[user]) return err(404, "UserNotFound", "not on the team");
        if (user === t.creator) return err(403, "NoAdmin", "the leader can't leave");
        if (user === me) return null;
        return can(t, me, PRIV.removeMember) ? null : err(403, "Privileges", "no");
      default: return err(400, "Invalid", "unknown team event");
    }
  }

  // ---------- reads ----------

  private presenceDoc(doc: FriendsDoc): FriendsDoc {
    return {
      ...doc,
      friends: doc.friends.map((f) => {
        const a = this.world.accounts.get(f.xuid);
        return { ...f, presence: a?.online ? { state: a.state, game_id: a.game_id, since: a.since } : null };
      }),
    };
  }

  async handle(req: Request, u: URL): Promise<Reply | null> {
    const path = u.pathname;
    let m: RegExpMatchArray | null;
    if (path === "/v1/events" && req.method === "POST") return this.postEvent(await req.text(), req.headers.get("authorization"));
    if (req.method !== "GET") return null;
    const live = () => [...this.teams.values()].filter((t) => !t.deleted).sort((a, b) => a.id.localeCompare(b.id));
    const page = (items: unknown[], next: string | null = null) => json({ items, next, position: this.world.log.length });
    if ((m = path.match(/^\/v1\/accounts\/by-xuid\/([0-9a-fA-F]{16})(\/teams)?$/))) {
      const a = this.world.accounts.get(m[1]!.toLowerCase());
      if (!a) return fail(404, "not_found", "no such account");
      return m[2] ? page(live().filter((t) => t.members[a.xuid] || t.recruits[a.xuid])) : json({ xuid: a.xuid, gamertag: a.gamertag });
    }
    if ((m = path.match(/^\/v1\/accounts\/([^/]+)\/(friends|teams)$/))) {
      const a = this.world.accountByTag(decodeURIComponent(m[1]!));
      if (!a) return fail(404, "not_found", "no such account");
      return m[2] === "friends" ? json(this.presenceDoc(this.friendsOf(a.xuid))) : page(live().filter((t) => t.members[a.xuid] || t.recruits[a.xuid]));
    }
    if (path === "/v1/teams") {
      const limit = Math.min(100, Math.max(1, Number(u.searchParams.get("limit") ?? 50) || 50));
      const after = u.searchParams.get("cursor") ?? "";
      const rest = live().filter((t) => t.id > after);
      const items = rest.slice(0, limit);
      return page(items, rest.length > limit ? items[items.length - 1]!.id : null);
    }
    if ((m = path.match(/^\/v1\/teams\/([0-9a-fA-F]{16})$/))) {
      const t = this.teams.get(m[1]!.toLowerCase());
      return t && !t.deleted ? json({ team: t, position: this.world.log.length }) : fail(404, "not_found", "no such team", "NotFound");
    }
    return null;
  }
}
