// Research codec only. Packet boundaries are real: decoding has no access
// to the following packet, the source frame, or a corpus-trained table.
import { deflateRawSync, inflateRawSync } from "node:zlib";
import { Range } from "./range.mjs";
import { project, restore } from "./columns.mjs";

const utf8 = new TextEncoder(),
  text = new TextDecoder("utf8", { fatal: true });
const slots = 1 << 17;
function context(parent, key) {
  let h = parent;
  for (let i = 0; i < key.length; i++)
    h = Math.imul(h ^ key.charCodeAt(i), 16777619);
  return h >>> 0;
}
const scales = {
  pos: 1024,
  p: 1024,
  vel: 128,
  amount: 256,
  body: 63,
  shield: 32,
  speed: 16,
  frame: 16,
  position: 1023,
};
function node(path = 2166136261, scale = 1) {
  return {
    path,
    scale,
    kind: -1,
    n: 0,
    last: 0,
    older: 0,
    order: 0,
    scores: [0, 0, 0],
    keys: [],
    children: [],
    len: 0,
  };
}
const ABSENT = 0,
  NUMBER = 1,
  STRING = 2,
  FALSE = 3,
  TRUE = 4,
  ARRAY = 5,
  OBJECT = 6,
  RAW = 7,
  NIL = 8;
function kind(v, scale) {
  if (v === undefined) return ABSENT;
  if (v === null) return NIL;
  if (typeof v === "number") {
    const n = v * scale;
    return Number.isSafeInteger(n) &&
      Math.abs(n) < 2 ** 48 &&
      Object.is(n / scale, v) &&
      !Object.is(n, -0)
      ? NUMBER
      : RAW;
  }
  if (typeof v === "boolean") return v ? TRUE : FALSE;
  if (typeof v === "string") return STRING;
  return Array.isArray(v) ? ARRAY : OBJECT;
}
function leb(out, v) {
  do {
    const b = v % 128;
    v = Math.floor(v / 128);
    out.push(b + (v ? 128 : 0));
  } while (v);
}
function getLeb(b, c) {
  let v = 0,
    m = 1;
  while (c.i < b.length && m <= 2 ** 49) {
    const x = b[c.i++];
    v += (x & 127) * m;
    if (x < 128) return v;
    m *= 128;
  }
  throw Error("invalid packet length");
}
const rawBuffer = new DataView(new ArrayBuffer(8));
function bitsOf(n) {
  rawBuffer.setFloat64(0, n);
  return rawBuffer.getBigUint64(0).toString(16);
}
function fromBits(s) {
  rawBuffer.setBigUint64(0, BigInt("0x" + s));
  return rawBuffer.getFloat64(0);
}

export class Connection {
  constructor() {
    this.reset();
  }
  reset() {
    this.root = node();
    this.models = new Uint16Array(slots);
    this.strings = [];
    this.ids = new Map();
    this.seq = 0;
  }
  string(r, base, value) {
    let id = r.uint(base, this.ids.get(value) ?? this.strings.length);
    if (id === this.strings.length) {
      const input = r.input ? undefined : utf8.encode(value);
      const n = r.uint(base + 32, input?.length);
      if (n > 65535) throw Error("literal too long");
      const bytes = r.input ? new Uint8Array(n) : input;
      for (let i = 0; i < n; i++) {
        let v = 0;
        for (let j = 7; j >= 0; j--)
          v = 2 * v + r.bit(base + 64 + j, ((input?.[i] ?? 0) >>> j) & 1);
        if (r.input) bytes[i] = v;
      }
      const s = r.input ? text.decode(bytes) : value;
      this.ids.set(s, id);
      this.strings.push(s);
    }
    if (id >= this.strings.length) throw Error("unknown string");
    return this.strings[id];
  }
  visit(r, t, v) {
    const base = t.path;
    const type = r.input ? 0 : kind(v, t.scale);
    if (r.bit(base, type !== t.kind)) {
      t.kind = r.uint(base + 1, type);
      t.n = 0;
      t.last = 0;
      t.older = 0;
      t.scores.fill(0);
      t.order = 0;
    }
    switch (t.kind) {
      case ABSENT:
        return undefined;
      case FALSE:
        return false;
      case TRUE:
        return true;
      case NIL:
        return null;
      case NUMBER: {
        const x = t.last,
          dx = t.last - t.older;
        // Causally choose hold, linear extrapolation or a rounded fractional
        // slope learned from the previous errors. Encoder and reader train alike.
        const estimates = [x, x + dx, Math.round(x + dx / 2)];
        const pred = estimates[t.order];
        const q =
          pred +
          r.signed(
            base + 40 + (t.order ? 32 : 0),
            r.input ? 0 : v * t.scale - pred,
          );
        for (let i = 0; i < 3; i++)
          t.scores[i] =
            0.875 * t.scores[i] + Math.log2(1 + Math.abs(q - estimates[i]));
        t.order = t.scores[1] < t.scores[0] ? 1 : 0;
        if (t.scores[2] < t.scores[t.order]) t.order = 2;
        t.older = t.n ? t.last : q;
        t.last = q;
        t.n++;
        return q / t.scale;
      }
      case RAW:
      case STRING: {
        const value = t.kind === RAW && !r.input ? bitsOf(v) : v;
        if (r.bit(base + 107, value !== this.strings[t.last] || !t.n)) {
          const s = this.string(r, base + 108, value);
          t.last = this.ids.get(s);
          t.n = 1;
        }
        return t.kind === RAW
          ? fromBits(this.strings[t.last])
          : this.strings[t.last];
      }
      case ARRAY: {
        const n = t.len + r.signed(base + 183, r.input ? 0 : v.length - t.len);
        if (n < 0 || n > 4096) throw Error("bad array size");
        t.len = n;
        const out = r.input ? new Array(n) : null;
        for (let i = 0; i < n; i++) {
          // Entity ordinals share a field model. Coordinate axes have their own.
          const child = (t.children[i] ??= node(
            context(
              base,
              t.scale !== 1 || n === 2 || n === 3 ? "[" + i + "]" : "[]",
            ),
            t.scale,
          ));
          const val = this.visit(r, child, r.input ? undefined : v[i]);
          if (out) out[i] = val;
        }
        return out;
      }
      case OBJECT: {
        // State carries the field dictionary and its order. Additions come on the
        // wire before first use. Disappearing optional fields become ABSENT.
        let added;
        if (!r.input) added = Object.keys(v).filter((k) => !t.keys.includes(k));
        const count = r.uint(base + 218, added?.length);
        for (let i = 0; i < count; i++) {
          const k = this.string(r, base + 252, added?.[i]);
          t.keys.push(k);
          t.children.push(node(context(base, k), scales[k] ?? 1));
        }
        const out = r.input ? {} : null;
        for (let i = 0; i < t.keys.length; i++) {
          const key = t.keys[i];
          const value = this.visit(
            r,
            t.children[i],
            r.input ? undefined : v[key],
          );
          if (out && value !== undefined) out[key] = value;
        }
        return out;
      }
      default:
        throw Error("invalid type");
    }
  }
  encode(frame, key) {
    const seq = this.seq;
    if (key) this.reset();
    this.seq = seq + 1;
    const r = new Range(this.models);
    this.visit(r, this.root, project(frame));
    const payload = r.finish();
    const head = [];
    leb(head, seq * 2 + Number(key));
    const record = [];
    leb(record, head.length + payload.length);
    return Uint8Array.from([...record, ...head, ...payload]);
  }
  decode(packet) {
    const c = { i: 0 },
      n = getLeb(packet, c);
    if (n !== packet.length - c.i) throw Error("truncated packet");
    const tag = getLeb(packet, c),
      seq = Math.floor(tag / 2);
    if (tag % 2) this.reset();
    else if (this.seq !== seq) throw Error("need snapshot or keyframe");
    this.seq = seq + 1;
    return restore(
      this.visit(new Range(this.models, packet.subarray(c.i)), this.root),
    );
  }
  snapshot() {
    // Only for late joins. This includes the adaptive tables, prediction
    // histories, schema, dictionary and next expected sequence.
    const table = Buffer.from(this.models.buffer).toString("base64");
    return deflateRawSync(
      JSON.stringify([this.seq, this.root, this.strings, table]),
    );
  }
  resume(snapshot) {
    const [seq, root, strings, table] = JSON.parse(inflateRawSync(snapshot));
    this.seq = seq;
    this.root = root;
    this.strings = strings;
    this.ids = new Map(strings.map((s, i) => [s, i]));
    const bytes = Buffer.from(table, "base64");
    new Uint8Array(this.models.buffer).set(bytes);
  }
}
