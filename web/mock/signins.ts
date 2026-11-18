// Sign-ins for the mock (src/state/signins.ts is the client):
//   GET    /v1/accounts/by-xuid/{xuid}/signins                  lxm signins.read
//   DELETE /v1/accounts/by-xuid/{xuid}/signins/{kid}            lxm signins.write
//   POST   /v1/accounts/by-xuid/{xuid}/signins/sign-out-others  lxm signins.write
// The mock keeps one real key per account (the caller's); each account also
// gets two made-up sign-ins elsewhere (a copy of Halo 2 and a phone) so the
// screen has something to sign out.

import type { World } from "./world";
import type { MockRecovery, Reply } from "./recovery";
import type { SignIn } from "../src/state/signins";

const ok = (body: unknown, status = 200): Reply => ({ status, body });
const fail = (status: number, error: string, message: string): Reply => ({ status, body: { error, message } });

export class MockSignIns {
  private others = new Map<string, SignIn[]>();
  constructor(private world: World, private recovery: MockRecovery, private nowMs: () => number = () => Date.now()) {}

  private elsewhere(xuid: string): SignIn[] {
    let l = this.others.get(xuid);
    if (!l) {
      const at = (s: number) => new Date(this.nowMs() - s * 1000).toISOString();
      l = [
        { kid: `mock-game-${xuid}`, kind: "game", name: "Halo 2", via: "code", created_at: at(3 * 86400), last_seen: at(2 * 3600), current: false },
        { kid: `mock-phone-${xuid}`, kind: "browser", name: "Safari on iPhone", via: "code", created_at: at(86400), last_seen: at(300), current: false },
      ];
      this.others.set(xuid, l);
    }
    return l;
  }

  async handle(req: Request, u: URL): Promise<Reply | null> {
    const m = u.pathname.match(/^\/v1\/accounts\/by-xuid\/([0-9a-fA-F]{16})\/signins(?:\/([^/]+))?$/);
    if (!m) return null;
    const xuid = m[1]!.toLowerCase(), rest = m[2] ? decodeURIComponent(m[2]) : null;
    const a = this.world.accounts.get(xuid);
    if (!a) return fail(404, "not_found", "no such account");
    const write = req.method !== "GET";
    const c = await this.recovery.caller(req.headers.get("authorization"), xuid, write ? "signins.write" : "signins.read");
    if (!c.ok) return c.reply;
    const list = this.elsewhere(xuid);
    if (req.method === "GET" && rest === null) {
      const mine: SignIn = { kid: a.key.kid, kind: "browser", name: browserName(req.headers.get("user-agent")), via: "signup", created_at: new Date(this.nowMs() - 600_000).toISOString(), last_seen: new Date(this.nowMs()).toISOString(), current: true };
      return ok({ items: [mine, ...list], position: this.world.log.length });
    }
    if (req.method === "POST" && rest === "sign-out-others") {
      const revoked = list.map((s) => s.kid);
      this.others.set(xuid, []);
      return ok({ revoked, position: this.world.log.length });
    }
    if (req.method === "DELETE" && rest) {
      if (rest === a.key.kid) return fail(409, "conflict", "that is the sign-in making this request; sign out on this device instead");
      const i = list.findIndex((s) => s.kid === rest);
      if (i < 0) return fail(404, "not_found", "no such sign-in on this account");
      list.splice(i, 1);
      return ok({ revoked: [rest], position: this.world.log.length });
    }
    return null;
  }
}

/** A short name from a User-Agent, as the server makes one. */
function browserName(ua: string | null): string | null {
  if (!ua?.startsWith("Mozilla/")) return null;
  const b = ua.includes("Edg/") ? "Edge" : ua.includes("Firefox/") ? "Firefox" : ua.includes("Chrome/") ? "Chrome" : ua.includes("Safari/") ? "Safari" : "A browser";
  const os = ua.includes("iPhone") ? "iPhone" : ua.includes("Android") ? "Android" : ua.includes("Mac OS X") ? "macOS" : ua.includes("Windows") ? "Windows" : ua.includes("Linux") ? "Linux" : "";
  return os ? `${b} on ${os}` : b;
}
