// Game variants for the mock (docs/live/live-api.md "Game variants"), so
// the variant page, the player page's Game Types and the workbench's
// "Make a game variant" run with `bun run dev` and no server. Kept in
// memory, two seeded for Sneaky Pete (one plain, one scripted):
// - `POST /v1/variants` (a `content.write` token): `{bundle, visibility}`,
//   or the legacy `{name, description, record_hex, script_hash?}`.
// - `POST /v1/variants/from-script` (`content.write`): a default record for
//   the script's carrier, published.
// - `POST /v1/variants/bundle`: a bundle from a saved or built-in script's
//   hash, or from source, unpublished.
// - `GET /v1/variants/{id}`, `GET /v1/variants?author=` (unlisted ones too
//   for the author's `read` token), `GET /v1/variants/{id}/h2variant`, and
//   `DELETE /v1/variants/{id}` by its author.
// The record is invented: the name as UTF-16LE and the engine index at
// 0x40, zeros after. The server builds the real 0x130 bytes.

import { createHash } from "node:crypto";
import { parseJws } from "../src/state/verify";
import { descriptionProblem, engineName, isBundle, nameProblem, variantFileName, type H2Variant, type Visibility } from "../src/lib/variants";
import type { Reply } from "./admin";
import type { World } from "./world";
import { builtinScripts, carrierOfSource, mockCheck, optionsOf, type MockRules } from "./rules";

export type VariantReply = Reply & { headers?: Record<string, string> };

const ok = (body: unknown, status = 200): VariantReply => ({ status, body });
const fail = (status: number, error: string, message: string): VariantReply => ({ status, body: { error, message } });
const RECORD_BYTES = 0x130;
const HASH = /^[0-9a-f]{64}$/;

const sha = (s: string) => createHash("sha256").update(s).digest("hex");

/** JSON with its object keys sorted, as the id is taken over. */
function canonical(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canonical).join(",")}]`;
  if (v && typeof v === "object") {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o).filter((k) => o[k] !== undefined).sort().map((k) => `${JSON.stringify(k)}:${canonical(o[k])}`).join(",")}}`;
  }
  return JSON.stringify(v);
}

/** The mock's record: the name as UTF-16LE, the engine at 0x40. */
export function mockRecord(name: string, engine: number): string {
  const b = new Uint8Array(RECORD_BYTES);
  for (let i = 0; i < Math.min(name.length, 31); i++) {
    const c = name.charCodeAt(i);
    b[i * 2] = c & 0xff;
    b[i * 2 + 1] = c >> 8;
  }
  b[0x40] = engine & 0xff;
  return [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
}

interface ScriptInfo { name: string; hash: string; source: string; engine: number }

interface Stored {
  bundle: H2Variant;
  id: string;
  author: { xuid: string; gamertag: string };
  visibility: Visibility;
  created: string;
  options: Record<string, unknown>;
  deleted: boolean;
}

const optionWords = (v: unknown): string => (typeof v === "boolean" ? (v ? "On" : "Off") : String(v));

export class MockVariants {
  readonly stored = new Map<string, Stored>();
  private seeded = false;

  constructor(private world: World, private rules: MockRules) {}

  private script(hash: string): ScriptInfo | null {
    const saved = this.rules.saved.get(hash);
    if (saved) return { name: saved.name, hash, source: saved.source, engine: carrierOfSource(saved.source)?.engine ?? 2 };
    const b = builtinScripts().find((x) => x.hash === hash);
    return b ? { name: b.name, hash, source: b.source, engine: b.engine } : null;
  }

  private bundleOf(name: string, description: string, engine: number, script: ScriptInfo | null, author?: { xuid: string; gamertag: string }): H2Variant {
    return {
      format: "h2recomp-game-variant",
      version: 2,
      name,
      description,
      ...(author ? { author } : {}),
      game_type: engineName(engine),
      engine,
      record_hex: mockRecord(name, engine),
      ...(script ? { script: { name: script.name, hash: script.hash, ir_b64: Buffer.from(script.source).toString("base64"), source: script.source } } : {}),
    };
  }

  private view(s: Stored) {
    const script = s.bundle.script ?? null;
    const settings = [
      { label: "Game type", value: s.bundle.game_type },
      ...(script ? [{ label: "Script", value: script.name }] : []),
      ...(script ? optionsOf(script.source ?? "").map((o) => ({ label: o.label, value: optionWords(s.options[o.name] ?? o.default) })) : []),
      { label: "Score to win", value: "25" },
      { label: "Time limit", value: "10 minutes" },
    ];
    return {
      id: s.id,
      name: s.bundle.name,
      description: s.bundle.description,
      author: s.author,
      engine: s.bundle.engine,
      game_type: s.bundle.game_type,
      record_hex: s.bundle.record_hex,
      script_hash: script?.hash ?? null,
      script_name: script?.name ?? null,
      visibility: s.visibility,
      created: s.created,
      bundle_url: `/v1/variants/${s.id}/h2variant`,
      settings,
      // The mock runs no policy: every kind of game.
      eligible: ["custom", "hopper", "ranked"],
    };
  }

  /** Publishes a bundle as `author`: 200 when it was already there. */
  private publish(bundle: H2Variant, author: { xuid: string; gamertag: string }, visibility: Visibility, options: Record<string, unknown> = {}, created = new Date().toISOString()): VariantReply {
    const problem = nameProblem(bundle.name) ?? descriptionProblem(bundle.description);
    if (problem) return fail(400, "invalid", problem);
    if (!/^[0-9a-f]+$/.test(bundle.record_hex) || bundle.record_hex.length !== RECORD_BYTES * 2) return fail(400, "invalid", "record_hex is 608 lowercase hex characters");
    const signed: H2Variant = { ...bundle, author };
    const id = sha(canonical(signed));
    const had = this.stored.get(id);
    if (had && !had.deleted) return ok(this.view(had));
    const s: Stored = { bundle: signed, id, author, visibility, created, options, deleted: false };
    this.stored.set(id, s);
    return ok(this.view(s), 201);
  }

  private seed() {
    if (this.seeded) return;
    const a = this.world.accountByTag("Sneaky Pete");
    if (!a) return;
    this.seeded = true;
    const who = { xuid: a.xuid, gamertag: a.gamertag };
    const day = 86_400_000;
    this.publish(this.bundleOf("Pillow Fight", "Swords only, low gravity, first to 15.", 2, null), who, "public", {}, new Date(Date.now() - 3 * day).toISOString());
    const b = builtinScripts()[0];
    if (b) {
      const info: ScriptInfo = { name: b.name, hash: b.hash, source: b.source, engine: b.engine };
      this.publish(this.bundleOf("Double Points", "Two points a kill, suicides cost one.", b.engine, info), who, "public", { kill_points: 2, suicide_costs: true }, new Date(Date.now() - day).toISOString());
    }
  }

  private caller(req: Request, method: string): string | null {
    const claims = parseJws((req.headers.get("authorization") ?? "").replace(/^Bearer /, "").trim())?.payload as { iss?: string; lxm?: string } | undefined;
    const xuid = /^acct:([0-9a-f]{16})@/.exec(claims?.iss ?? "")?.[1];
    return xuid && claims?.lxm === method ? xuid : null;
  }

  private who(xuid: string) {
    return { xuid, gamertag: this.world.accounts.get(xuid)?.gamertag ?? "" };
  }

  /** The bundle a script and its options make, or a refusal. */
  private fromScript(b: Record<string, unknown>, author?: { xuid: string; gamertag: string }): H2Variant | VariantReply {
    const name = typeof b.name === "string" ? b.name : "";
    const description = typeof b.description === "string" ? b.description : "";
    const problem = nameProblem(name) ?? descriptionProblem(description);
    if (problem) return fail(400, "invalid", problem);
    let script: ScriptInfo | null = null;
    if (typeof b.script_hash === "string") {
      script = this.script(b.script_hash);
      if (!script) return fail(404, "not_found", "no such script");
    } else if (typeof b.source === "string") {
      const c = mockCheck(b.source);
      if (!c.ok || !c.hash) return fail(400, "invalid", `the script does not verify: ${c.errors[0]?.message ?? "it does not verify"}`);
      script = { name: c.name ?? "Script", hash: c.hash, source: b.source, engine: carrierOfSource(b.source)?.engine ?? 2 };
    } else {
      return fail(400, "invalid", "send script_hash or source");
    }
    return this.bundleOf(name, description, script.engine, script, author);
  }

  async handle(req: Request, u: URL): Promise<VariantReply | null> {
    const path = u.pathname;
    if (path !== "/v1/variants" && !path.startsWith("/v1/variants/")) return null;
    this.seed();
    const body = async (): Promise<Record<string, unknown> | null> => {
      try { const b = await req.json(); return b && typeof b === "object" ? b as Record<string, unknown> : null; } catch { return null; }
    };
    const visibility = (v: unknown): Visibility => (v === "unlisted" ? "unlisted" : "public");

    if (path === "/v1/variants" && req.method === "GET") {
      const author = u.searchParams.get("author")?.trim();
      if (!author) return fail(400, "invalid", "?author= (a gamertag or xuid) is required");
      const a = this.world.accounts.get(author) ?? this.world.accountByTag(author);
      if (!a) return fail(404, "not_found", "no such account");
      const reader = this.caller(req, "read");
      const items = [...this.stored.values()]
        .filter((s) => !s.deleted && s.author.xuid === a.xuid && (s.visibility === "public" || reader === a.xuid))
        .sort((x, y) => y.created.localeCompare(x.created))
        .map((s) => this.view(s));
      return ok({ items });
    }
    if (path === "/v1/variants" && req.method === "POST") {
      const caller = this.caller(req, "content.write");
      if (!caller) return fail(401, "unauthorized", "this needs a content.write token");
      const b = await body();
      if (!b) return fail(400, "invalid", "body: expected JSON");
      if (b.bundle !== undefined) {
        if (!isBundle(b.bundle)) return fail(400, "invalid", "bundle: not a format 2 .h2variant");
        return this.publish(b.bundle, this.who(caller), visibility(b.visibility), {});
      }
      if (typeof b.name !== "string" || typeof b.record_hex !== "string") return fail(400, "invalid", "body: send bundle, or name, description and record_hex");
      const script = typeof b.script_hash === "string" ? this.script(b.script_hash) : null;
      if (typeof b.script_hash === "string" && !script) return fail(404, "not_found", "no such script");
      const engine = parseInt(b.record_hex.slice(0x80, 0x82), 16) || 2;
      const bundle = { ...this.bundleOf(b.name, typeof b.description === "string" ? b.description : "", engine, script), record_hex: b.record_hex };
      return this.publish(bundle, this.who(caller), visibility(b.visibility), {});
    }
    if (path === "/v1/variants/from-script" && req.method === "POST") {
      const caller = this.caller(req, "content.write");
      if (!caller) return fail(401, "unauthorized", "this needs a content.write token");
      const b = await body();
      if (!b || typeof b.script_hash !== "string") return fail(400, "invalid", "body: missing field `script_hash`");
      const made = this.fromScript(b);
      if (!isBundle(made)) return made;
      const options = b.options && typeof b.options === "object" ? b.options as Record<string, unknown> : {};
      return this.publish(made, this.who(caller), visibility(b.visibility), options);
    }
    if (path === "/v1/variants/bundle" && req.method === "POST") {
      const b = await body();
      if (!b) return fail(400, "invalid", "body: expected JSON");
      const made = this.fromScript(b);
      if (!isBundle(made)) return made;
      return ok({ id: sha(canonical(made)), file_name: variantFileName(made.name), bundle: made });
    }
    const m = path.match(/^\/v1\/variants\/([0-9a-f]{64})(\/h2variant)?$/);
    if (!m || !HASH.test(m[1]!)) return fail(404, "not_found", "no such variant");
    const s = this.stored.get(m[1]!);
    if (!s || s.deleted) return fail(404, "not_found", "no such variant");
    if (req.method === "GET" && m[2]) {
      return { status: 200, body: s.bundle, headers: { "content-disposition": `attachment; filename="${variantFileName(s.bundle.name)}"` } };
    }
    if (req.method === "GET") return ok(this.view(s));
    if (req.method === "DELETE" && !m[2]) {
      const caller = this.caller(req, "content.write");
      if (!caller) return fail(401, "unauthorized", "this needs a content.write token");
      if (caller !== s.author.xuid) return fail(403, "forbidden", "only its author can delete it");
      s.deleted = true;
      return ok({ ok: true, id: s.id });
    }
    return fail(405, "method_not_allowed", "not on this route");
  }
}
