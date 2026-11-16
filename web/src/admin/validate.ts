// The admin editors' rules and the PUT bodies they send — pure functions,
// tested in test/admin.test.ts. The limits are the game's (docs/live/live-motd.md
// §3.1 and §5.2, mirrored by live-proto's `motd` and `hopper`); the server
// checks them all again, so this is for immediate, friendly errors.

// ---------- the MOTD ----------

export const MOTD_MAX_MESSAGES = 32;
/** Longest string, in UTF-16 code units (`string.length` counts them). */
export const MOTD_MAX_UNITS = 255;

export interface MotdDoc {
  header: string;
  messages: string[];
}

/** What is wrong with this MOTD, or null. */
export function motdError(doc: MotdDoc): string | null {
  const text = (s: string, what: string): string | null => {
    if (s.includes("\0")) return `${what} contains a NUL character`;
    // A surrogate pair is two units, as in the game's UTF-16 fields.
    if (s.length > MOTD_MAX_UNITS) return `${what} is longer than ${MOTD_MAX_UNITS} characters`;
    return null;
  };
  if (doc.messages.length < 1) return "the message of the day needs at least one message";
  if (doc.messages.length > MOTD_MAX_MESSAGES)
    return `at most ${MOTD_MAX_MESSAGES} messages, got ${doc.messages.length}`;
  return (
    text(doc.header, "the header") ??
    doc.messages.map((m, i) => text(m, `message ${i + 1}`)).find((e) => e) ??
    null
  );
}

// ---------- the hoppers ----------

export const HOPPER_COUNT = 16;
export const MAX_RECORDS = 100;
/** The name field is 0x85 bytes; the string field 0x48. */
export const MAX_NAME = 0x84;
export const MAX_STRING = 0x47;
export const DWORS = 18;
/** The title and description slots, in UTF-8 bytes (minus the NUL). */
export const MAX_TITLE = 15;
export const MAX_DESCRIPTION = 127;
/** Session setup clamps a record's weight (`dwords[0]`) to 1..1000. */
export const MAX_WEIGHT = 1000;

/** What a record's game variant says about itself (read-only). */
export interface RecordVariant {
  name: string;
  /** Game-engine index: 1 CTF, 2 Slayer, 3 Oddball, 4 King, 7 Juggernaut,
   *  8 Territories, 9 Assault. */
  engine: number;
  team: boolean;
}

/** One entry of a hopper: a map (`id`, a multiplayer level id) with a game
 *  variant (the bytes after `+0x24c`, which `string`, `flags` and
 *  `dwords[1]` lie inside), picked at random by `dwords[0]`, its weight. */
export interface HopperRecord {
  id: number;
  string: string;
  flags: number;
  dwords: [number, number];
  variant?: RecordVariant | null;
}

/** `GET /v1/titles/{title}/hopper`, one hopper of it. */
export interface HopperView {
  index: number;
  type: number;
  status: number;
  record_count: number;
  name: string;
  /** What the playlist list shows (English slot; written to all nine). */
  title: string;
  description: string;
  ranges: [number, number, number];
  dwords: number[];
  records: HopperRecord[];
}

/** UTF-8 length, the unit the title and description slots count in. */
export function utf8Length(s: string): number {
  return new TextEncoder().encode(s).length;
}

const U32_MAX = 0xffff_ffff;

function isU32(n: number): boolean {
  return Number.isInteger(n) && n >= 0 && n <= U32_MAX;
}

/** What is wrong with this hopper (empty when it will save). */
export function hopperErrors(h: HopperView): string[] {
  const errs: string[] = [];
  if (!isU32(h.type) || h.type < 1 || h.type > 5)
    errs.push("type must be 1 to 5");
  if (!isU32(h.status) || h.status > 2)
    errs.push("status must be 0 (not loaded), 1 (ok) or 2 (error)");
  if (!h.title) errs.push("the playlist needs a title");
  if (h.title.includes("\0") || h.description.includes("\0"))
    errs.push("the title and description cannot contain NUL");
  if (utf8Length(h.title) > MAX_TITLE)
    errs.push(`the title is longer than ${MAX_TITLE} bytes`);
  if (utf8Length(h.description) > MAX_DESCRIPTION)
    errs.push(`the description is longer than ${MAX_DESCRIPTION} bytes`);
  if (!asciiOk(h.name)) errs.push("the name must be ASCII");
  if (h.name.length > MAX_NAME)
    errs.push(`the name is longer than ${MAX_NAME} characters`);
  if (h.records.length < 1 || h.records.length > MAX_RECORDS)
    errs.push(`1 to ${MAX_RECORDS} entries, got ${h.records.length}`);
  if (h.record_count !== h.records.length)
    errs.push("record_count does not match the entry list");
  if (h.ranges.length !== 3 || !h.ranges.every(isU32))
    errs.push("the three range fields must be whole numbers 0 to 4294967295");
  if (h.dwords.length !== DWORS)
    errs.push(`the dword array has ${DWORS} entries; leave it as loaded`);
  h.records.forEach((r, i) => {
    if (!isU32(r.id)) errs.push(`entry ${i + 1}: the id must be a whole number 0 to 4294967295`);
    if (!asciiOk(r.string)) errs.push(`entry ${i + 1}: the string must be ASCII`);
    if (r.string.length > MAX_STRING)
      errs.push(`entry ${i + 1}: the string is longer than ${MAX_STRING} characters`);
    if (![r.flags, ...r.dwords].every(isU32))
      errs.push(`entry ${i + 1}: flags and dwords must be whole numbers 0 to 4294967295`);
  });
  return errs;
}

function asciiOk(s: string): boolean {
  for (const c of s) if (c.codePointAt(0)! > 0x7f) return false;
  return true;
}

// ---------- the PUT body ----------

/** A working copy of a record, remembering where its unknown bytes come
 *  from: the entry's position in the hopper as published (`src`), which the
 *  server needs to move template blobs whole when entries are reordered,
 *  copied or added. Null only for a record the server will take from its
 *  own position. */
export interface EditRecord extends HopperRecord {
  src: number | null;
  /** The hopper `src` is in, when not this one (a record copied across
   *  playlists, e.g. by a preset). */
  srcHopper?: number;
}

export interface EditHopper {
  index: number;
  type: number;
  status: number;
  name: string;
  title: string;
  description: string;
  ranges: [number, number, number];
  dwords: number[];
  records: EditRecord[];
}

/** The working copy of the whole file: every record tagged with its source
 *  slot. */
export function toEditable(hoppers: HopperView[]): EditHopper[] {
  return hoppers.map((h) => ({
    index: h.index,
    type: h.type,
    status: h.status,
    name: h.name,
    title: h.title ?? "",
    description: h.description ?? "",
    ranges: [...h.ranges] as [number, number, number],
    dwords: [...h.dwords],
    records: h.records.map((r, i) => ({ ...r, dwords: [...r.dwords] as [number, number], src: i })),
  }));
}

export interface PatchRecord {
  id: number;
  string: string;
  flags: number;
  dwords: [number, number];
  from?: number;
  from_hopper?: number;
}

export interface PatchHopper {
  index: number;
  type: number;
  status: number;
  name: string;
  title: string;
  description: string;
  ranges: [number, number, number];
  dwords: number[];
  record_count: number;
  records: PatchRecord[];
}

export interface HoppersPatch {
  hoppers: PatchHopper[];
}

function sameRecord(a: EditRecord, b: EditRecord): boolean {
  return (
    a.id === b.id && a.string === b.string && a.flags === b.flags &&
    a.dwords[0] === b.dwords[0] && a.dwords[1] === b.dwords[1] &&
    a.src === b.src && (a.srcHopper ?? null) === (b.srcHopper ?? null)
  );
}

function sameHopper(edited: EditHopper, original: EditHopper): boolean {
  return (
    edited.type === original.type && edited.status === original.status &&
    edited.name === original.name &&
    edited.title === original.title && edited.description === original.description &&
    edited.ranges.every((r, i) => r === original.ranges[i]) &&
    edited.dwords.every((d, i) => d === original.dwords[i]) &&
    edited.records.length === original.records.length &&
    edited.records.every((r, i) => {
      const o = original.records[i];
      return o !== undefined && sameRecord(r, o);
    })
  );
}

/** The PUT body for the edits, against the file they came from: only the
 *  hoppers that changed, each record carrying `from` (its source slot)
 *  whenever it is not simply itself in place. An empty patch means nothing
 *  changed; callers skip the PUT. */
export function buildPatch(original: EditHopper[], edited: EditHopper[]): HoppersPatch {
  const hoppers: PatchHopper[] = [];
  for (const h of edited) {
    const was = original.find((o) => o.index === h.index);
    if (was && sameHopper(h, was)) continue;
    hoppers.push({
      index: h.index,
      type: h.type,
      status: h.status,
      name: h.name,
      title: h.title,
      description: h.description,
      ranges: [...h.ranges] as [number, number, number],
      dwords: [...h.dwords],
      record_count: h.records.length,
      records: h.records.map((r, slot) => {
        const p: PatchRecord = {
          id: r.id,
          string: r.string,
          flags: r.flags,
          dwords: [...r.dwords] as [number, number],
        };
        // The server reads a record without `from` as the one in its own slot.
        const across = r.srcHopper !== undefined && r.srcHopper !== h.index;
        if (r.src !== null && (across || r.src !== slot)) p.from = r.src;
        if (across && r.src !== null) p.from_hopper = r.srcHopper;
        return p;
      }),
    });
  }
  return { hoppers };
}

// ---------- importing an existing account ----------

/** The browser's account entry (src/state/account.ts): `key` is the
 *  base64url seed. */
export interface BrowserAccount {
  xuid: string;
  gamertag: string;
  id: string;
  server: string;
  server_id: string;
  kid: string;
  key: string;
  x: string;
}

/**
 * Parses what `h2 live account export` writes (and the older JWK form):
 * one entry or a list. The key may be the bare seed (`"key": "..."`) or a
 * private JWK (`"key": {kty, crv, x, d}`). Never returns half an entry:
 * anything missing is an error naming it.
 */
export function parseImportEntry(text: string): { ok: true; accounts: BrowserAccount[] } | { ok: false; error: string } {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return { ok: false, error: "that is not JSON" };
  }
  const entries = Array.isArray(value) ? value : [value];
  if (entries.length === 0) return { ok: false, error: "no accounts in it" };
  const accounts: BrowserAccount[] = [];
  for (const e of entries) {
    const o = e as Record<string, unknown>;
    const str = (k: string): string | null => (typeof o[k] === "string" ? (o[k] as string) : null);
    const xuid = str("xuid");
    const gamertag = str("gamertag");
    const server = str("server");
    if (!xuid || !gamertag || !server) {
      return { ok: false, error: "an entry needs xuid, gamertag and server" };
    }
    let key = str("key");
    let x = str("x");
    const serverId = str("server_id");
    if (!key && o.key && typeof o.key === "object") {
      const jwk = o.key as Record<string, unknown>;
      key = typeof jwk.d === "string" ? jwk.d : null;
      x = x ?? (typeof jwk.x === "string" ? jwk.x : null);
    }
    if (!key || !x) return { ok: false, error: `${gamertag}'s entry has no Ed25519 key` };
    accounts.push({
      xuid,
      gamertag,
      id: str("id") ?? (xuid && serverId ? `acct:${xuid}@${serverId}` : ""),
      server,
      server_id: serverId ?? "",
      kid: str("kid") ?? "",
      key,
      x,
    });
  }
  return { ok: true, accounts };
}
