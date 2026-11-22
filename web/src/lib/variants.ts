// Game variants on Live (docs/live/live-api.md, "Game variants"): what
// players play, a name, description, base game type and the 0x130-byte
// record, with the rules script it uses when it has one. Live keeps each
// as a self-contained `.h2variant` file (a bundle). Here: the answers read
// defensively, the name and description rules the server enforces (for
// instant feedback), the file name a bundle downloads as, option values
// coerced to what the script declares, and the calls with their token
// scope. The server builds records and checks everything again.

import { ApiError, requireOk } from "../api/http";
import type { LocalAccount } from "../state/account";
import { issueToken } from "../admin/token";
import { statsPath } from "../routes";
import { CONTENT_WRITE, type ScriptOption } from "./rules";

export const VARIANT_NAME_MAX = 31;
export const VARIANT_DESCRIPTION_MAX = 150;
/** The bundle's file extension. */
export const VARIANT_EXT = ".h2variant";

export type Visibility = "public" | "unlisted";
export interface VariantAuthor { xuid: string; gamertag: string }
export interface VariantSetting { label: string; value: string }

export interface Variant {
  /** 64 hex: the SHA-256 of the bundle's canonical bytes. */
  id: string;
  name: string;
  description: string;
  author: VariantAuthor;
  /** Retail engine index: 1 CTF, 2 Slayer, 3 Oddball, 4 King, 7 Juggernaut, 8 Territories, 9 Assault. */
  engine: number;
  game_type: string;
  /** 608 lowercase hex characters (0x130 bytes). */
  record_hex: string;
  script_hash: string | null;
  script_name: string | null;
  visibility: Visibility;
  created: string;
  /** "/v1/variants/{id}/h2variant". */
  bundle_url: string;
  /** The server's summary of the record. */
  settings: VariantSetting[];
  /** Where it may play, by the script policy: "custom", "hopper", "ranked". */
  eligible: string[];
}

/** The `.h2variant` file, format 2. */
export interface H2Variant {
  format: "h2recomp-game-variant";
  version: 2;
  name: string;
  description: string;
  author?: VariantAuthor;
  game_type: string;
  engine: number;
  record_hex: string;
  script?: { name: string; hash: string; ir_b64: string; source?: string };
}

/** `POST /v1/variants/bundle`'s answer. */
export interface BundleAnswer { id: string; file_name: string; bundle: H2Variant }

export type OptionValue = number | boolean | string;
export type OptionValues = Record<string, OptionValue>;

const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const num = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v : 0);
const optStr = (v: unknown): string | null => (typeof v === "string" && v ? v : null);
const HASH = /^[0-9a-f]{64}$/;

/** The retail game types by engine index. */
export const ENGINES: Readonly<Record<number, string>> = {
  1: "Capture the Flag", 2: "Slayer", 3: "Oddball", 4: "King of the Hill", 7: "Juggernaut", 8: "Territories", 9: "Assault",
};
export const engineName = (engine: number): string => ENGINES[engine] ?? `Engine ${engine}`;

export const variantHref = (id: string): string => statsPath(`/variants/${encodeURIComponent(id)}`);
/** The workbench with a script open read-only. */
export const scriptHref = (hash: string): string => `${statsPath("/rules")}?script=${encodeURIComponent(hash)}`;

/** An absolute link to a variant's file: `bundle_url` is a path on the
 *  server (`base` has no trailing slash). */
export function bundleHref(base: string, bundleUrl: string): string {
  if (/^https?:\/\//.test(bundleUrl)) return bundleUrl;
  return `${base}${bundleUrl.startsWith("/") ? "" : "/"}${bundleUrl}`;
}

// ---------- names ----------

const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;
const SURROGATE = /[\ud800-\udfff]/;
const RESERVED_GLYPHS = /[\ue405-\ue407]/;

/** What is wrong with a variant name, or null when the server takes it:
 *  1 to 31 characters (UTF-16 units), no space at either end, no control
 *  characters, no `|`, nothing above U+FFFF, and none of the game's
 *  glyphs U+E405 to U+E407. */
export function nameProblem(name: string): string | null {
  if (name.length === 0) return "give it a name";
  if (name.length > VARIANT_NAME_MAX) return `a name is at most ${VARIANT_NAME_MAX} characters`;
  if (name.trim() !== name) return "a name can't start or end with a space";
  if (CONTROL.test(name)) return "a name can't hold control characters";
  if (name.includes("|")) return "a name can't hold |";
  if (SURROGATE.test(name)) return "a name can't hold emoji or other characters above U+FFFF";
  if (RESERVED_GLYPHS.test(name)) return "a name can't hold the game's button glyphs";
  return null;
}

/** What is wrong with a description, or null: at most 150 characters
 *  (UTF-16 units), no control characters, no `|`. */
export function descriptionProblem(text: string): string | null {
  if (text.length > VARIANT_DESCRIPTION_MAX) return `a description is at most ${VARIANT_DESCRIPTION_MAX} characters`;
  if (CONTROL.test(text)) return "a description can't hold control characters or line breaks";
  if (text.includes("|")) return "a description can't hold |";
  return null;
}

/** The file a variant downloads as: its name with the characters file
 *  systems refuse replaced by `_`, and `.h2variant`. */
export function variantFileName(name: string): string {
  const safe = name
    .replace(/[\u0000-\u001f\u007f-\u009f<>:"/\\|?*]/g, "_")
    .trim()
    .replace(/[. ]+$/, "")
    .replace(/^\.+/, "");
  return `${safe || "game variant"}${VARIANT_EXT}`;
}

// ---------- options ----------

/** One option's value made what the script declares: an int whole and in
 *  its range, a bool a bool, an enum one of its value names. Anything it
 *  can't read is the option's default. */
export function coerceOption(o: ScriptOption, raw: unknown): OptionValue {
  switch (o.kind) {
    case "bool":
      if (typeof raw === "boolean") return raw;
      if (raw === "true" || raw === 1 || raw === "1") return true;
      if (raw === "false" || raw === 0 || raw === "0") return false;
      return o.default === true;
    case "enum": {
      if (typeof raw === "string" && o.values.includes(raw)) return raw;
      if (typeof raw === "number" && Number.isInteger(raw) && raw >= 0 && raw < o.values.length) return o.values[raw]!;
      return typeof o.default === "string" && o.values.includes(o.default) ? o.default : (o.values[0] ?? "");
    }
    case "int": {
      const n = typeof raw === "number" ? raw : typeof raw === "string" && raw.trim() !== "" ? Number(raw) : Number.NaN;
      const dflt = typeof o.default === "number" ? o.default : o.lo;
      if (!Number.isFinite(n)) return dflt;
      const lo = Math.min(o.lo, o.hi), hi = Math.max(o.lo, o.hi);
      return Math.min(hi, Math.max(lo, Math.round(n)));
    }
  }
}

/** Every declared option's value, from what was typed (missing ones get
 *  their default); names the script doesn't declare are dropped. */
export function coerceOptions(declared: ScriptOption[], raw: Record<string, unknown>): OptionValues {
  const out: OptionValues = {};
  for (const o of declared) out[o.name] = coerceOption(o, raw[o.name]);
  return out;
}

/** Each declared option at its default. */
export const defaultOptions = (declared: ScriptOption[]): OptionValues => coerceOptions(declared, {});

// ---------- reading answers ----------

export function parseVariant(body: unknown): Variant | null {
  const v = obj(body);
  const id = str(v.id);
  if (!HASH.test(id)) return null;
  const author = obj(v.author);
  const scriptHash = optStr(v.script_hash);
  return {
    id,
    name: str(v.name) || "Untitled",
    description: str(v.description),
    author: { xuid: str(author.xuid), gamertag: str(author.gamertag) },
    engine: num(v.engine),
    game_type: str(v.game_type) || engineName(num(v.engine)),
    record_hex: str(v.record_hex),
    script_hash: scriptHash && HASH.test(scriptHash) ? scriptHash : null,
    script_name: optStr(v.script_name),
    visibility: v.visibility === "unlisted" ? "unlisted" : "public",
    created: str(v.created),
    bundle_url: str(v.bundle_url) || `/v1/variants/${id}/h2variant`,
    settings: arr(v.settings).map(obj).filter((s) => typeof s.label === "string").map((s) => ({ label: str(s.label), value: typeof s.value === "string" ? s.value : String(s.value ?? "") })),
    eligible: arr(v.eligible).filter((e): e is string => typeof e === "string"),
  };
}

/** `GET /v1/variants?author=`: newest first, as the server sends them. */
export function parseVariants(body: unknown): Variant[] {
  return arr(obj(body).items).map(parseVariant).filter((v): v is Variant => v !== null);
}

/** Whether a value looks like a format 2 bundle. */
export function isBundle(v: unknown): v is H2Variant {
  const b = obj(v);
  return b.format === "h2recomp-game-variant" && b.version === 2 && typeof b.name === "string" && typeof b.record_hex === "string" && typeof b.engine === "number";
}

export function parseBundleAnswer(body: unknown): BundleAnswer {
  const b = obj(body);
  const bundle = b.bundle;
  if (!isBundle(bundle)) throw new Error("the server sent an unreadable variant file");
  return { id: str(b.id), file_name: str(b.file_name) || variantFileName(bundle.name), bundle };
}

// ---------- the file ----------

/** A bundle as the file's text. */
export const bundleText = (bundle: H2Variant): string => JSON.stringify(bundle, null, 2) + "\n";

/** Saves a bundle as a file through a Blob link. */
export function downloadBundle(bundle: H2Variant, fileName: string, doc: Pick<Document, "createElement"> = document): void {
  const url = URL.createObjectURL(new Blob([bundleText(bundle)], { type: "application/json" }));
  const a = doc.createElement("a");
  a.href = url;
  a.download = fileName;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// ---------- the API ----------

const readJson = async (r: Response): Promise<unknown> => (await requireOk(r)).json();
const getJson = (fetchFn: typeof fetch, url: string, bearer?: string | null) =>
  fetchFn(url, { headers: { accept: "application/json", ...(bearer ? { authorization: `Bearer ${bearer}` } : {}) } });
const postJson = (fetchFn: typeof fetch, url: string, body: unknown, headers: Record<string, string> = {}) =>
  fetchFn(url, { method: "POST", headers: { accept: "application/json", "content-type": "application/json", ...headers }, body: JSON.stringify(body) });
const writer = async (me: LocalAccount, aud: string) => ({ authorization: `Bearer ${await issueToken(me, aud, CONTENT_WRITE)}` });

function variantOrThrow(body: unknown): Variant {
  const v = parseVariant(body);
  if (!v) throw new Error("the server sent an unreadable variant");
  return v;
}

/** `GET /v1/variants/{id}`; an ApiError 404 when unknown or deleted. */
export async function fetchVariant(api: string, id: string, fetchFn: typeof fetch = fetch): Promise<Variant> {
  return variantOrThrow(await readJson(await getJson(fetchFn, `${api}/variants/${encodeURIComponent(id)}`)));
}

/** `GET /v1/variants?author=`: the public ones, and the unlisted ones too
 *  when `bearer` is the author's `read` token. */
export async function fetchVariants(api: string, author: string, bearer: string | null = null, fetchFn: typeof fetch = fetch): Promise<Variant[]> {
  return parseVariants(await readJson(await getJson(fetchFn, `${api}/variants?author=${encodeURIComponent(author)}`, bearer)));
}

/** `POST /v1/variants`: publishes a bundle (200 when that exact variant
 *  was already published, 201 when this published it). */
export async function publishVariant(api: string, aud: string, me: LocalAccount, bundle: H2Variant, visibility: Visibility, fetchFn: typeof fetch = fetch): Promise<Variant> {
  return variantOrThrow(await readJson(await postJson(fetchFn, `${api}/variants`, { bundle, visibility }, await writer(me, aud))));
}

export interface FromScript { script_hash: string; name: string; description: string; visibility: Visibility; options: OptionValues }

/** `POST /v1/variants/from-script`: the server builds the record for the
 *  script's carrier and publishes it. */
export async function publishFromScript(api: string, aud: string, me: LocalAccount, body: FromScript, fetchFn: typeof fetch = fetch): Promise<Variant> {
  return variantOrThrow(await readJson(await postJson(fetchFn, `${api}/variants/from-script`, body, await writer(me, aud))));
}

/** A script by its saved hash, or by its source when it isn't saved. */
export type ScriptRef = { script_hash: string } | { source: string };
export interface BundleRequest { name: string; description: string; options: OptionValues }

/** `POST /v1/variants/bundle`: builds a bundle without publishing it. No
 *  token, so it works signed out. */
export async function makeBundle(api: string, script: ScriptRef, req: BundleRequest, fetchFn: typeof fetch = fetch): Promise<BundleAnswer> {
  return parseBundleAnswer(await readJson(await postJson(fetchFn, `${api}/variants/bundle`, { ...script, ...req })));
}

/** `DELETE /v1/variants/{id}`, by its author. */
export async function deleteVariant(api: string, aud: string, me: LocalAccount, id: string, fetchFn: typeof fetch = fetch): Promise<void> {
  const r = await fetchFn(`${api}/variants/${encodeURIComponent(id)}`, { method: "DELETE", headers: { accept: "application/json", ...(await writer(me, aud)) } });
  const b = obj(await readJson(r));
  if (b.ok !== true) throw new ApiError(r.status, "invalid", "the server didn't delete it");
}

/** Whether an error is the server's 404. */
export const notFound = (e: unknown): boolean => e instanceof ApiError && e.status === 404;
