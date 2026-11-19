// Forward byte-range decoder for v8 packed telemetry. Range and code are
// unsigned 32-bit registers. All interval products are exact JS integers.
import { PackError } from "./pack";

const SCALE = 4096;
const PRIOR = [16, 3, 3, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
const PRIOR_TOTAL = PRIOR.reduce((sum, count) => sum + count, 0);

class Distribution {
  readonly counts = Uint16Array.from(PRIOR);
  readonly ends = new Uint16Array(16);
  private total = PRIOR_TOTAL;
  private seen = 0;

  constructor() { this.rebuild(); }

  private rebuild(): void {
    let mass = 0, peak = 0;
    for (let i = 0; i < 16; i++) {
      const count = this.counts[i]!;
      if (count > this.counts[peak]!) peak = i;
      mass += 1 + Math.floor(count * (SCALE - 16) / this.total);
      this.ends[i] = mass;
    }
    for (let i = peak; i < 16; i++) this.ends[i] = this.ends[i]! + SCALE - mass;
  }

  observe(symbol: number): void {
    this.counts[symbol] = this.counts[symbol]! + 1;
    this.total++;
    const n = ++this.seen;
    if (n % 128 !== 0 && (n >= 128 || (n & (n - 1)) !== 0)) return;
    if (this.total >= 2048) {
      this.total = 0;
      for (let i = 0; i < 16; i++) {
        const half = (this.counts[i]! + 1) >>> 1;
        this.counts[i] = half;
        this.total += half;
      }
    }
    this.rebuild();
  }
}

/** Models persist across records of one stream. Allocate integer tables
 * only for field contexts the stream actually uses. */
export class RangeModels {
  readonly binary: Uint16Array;
  readonly integers: (Distribution | undefined)[];

  constructor(contexts: number) {
    this.binary = new Uint16Array(contexts).fill(2048);
    this.integers = new Array(Math.ceil(contexts / 32));
  }
}

export class RangeReader {
  private at: number;
  private code: number;
  private range = 0xffffffff;

  constructor(private readonly bytes: Uint8Array, private readonly models: RangeModels,
              start = 0, private readonly end = bytes.length) {
    if (start < 0 || end > bytes.length || end - start < 5 || bytes[start] !== 0)
      throw new PackError("invalid range packet header");
    this.at = start + 5;
    this.code = new DataView(bytes.buffer, bytes.byteOffset + start + 1, 4).getUint32(0);
  }

  private slot(): number {
    const slot = Math.floor(this.code / (this.range >>> 12));
    if (slot >= SCALE) throw new PackError("invalid range interval");
    return slot;
  }

  private advance(start: number, width: number): void {
    const unit = this.range >>> 12;
    this.code -= unit * start;
    this.range = unit * width;
    while (this.range < 0x1000000) {
      if (this.at >= this.end) throw new PackError("truncated range packet");
      this.range *= 256;
      this.code = ((this.code << 8) | this.bytes[this.at++]!) >>> 0;
    }
  }

  finish(): void {
    if (this.at !== this.end || this.code !== 0) throw new PackError("invalid range packet ending");
  }

  bit(ctx: number): boolean {
    const p = this.models.binary[ctx]!;
    const one = this.slot() >= p;
    this.advance(one ? p : 0, one ? SCALE - p : p);
    this.models.binary[ctx] = one ? Math.max(1, p - Math.ceil(p / 32))
      : Math.min(SCALE - 1, p + Math.ceil((SCALE - p) / 32));
    return one;
  }

  private uniform(bits: number): number {
    const shift = 12 - bits;
    const symbol = this.slot() >>> shift;
    this.advance(symbol << shift, 1 << shift);
    return symbol;
  }

  literal(): number { return this.uniform(8); }

  uint(ctx: number): number {
    const group = ctx >>> 5;
    const d = this.models.integers[group] ??= new Distribution();
    const slot = this.slot();
    let s = 0;
    while (slot >= d.ends[s]!) s++;
    const start = s === 0 ? 0 : d.ends[s - 1]!;
    this.advance(start, d.ends[s]! - start);
    d.observe(s);
    if (s < 8) return s;
    if (s < 15) return (1 << (s - 5)) + this.uniform(s - 5);
    let value = 1024;
    for (let shift = 0; shift <= 49; shift += 7) {
      const b = this.literal();
      value += (b & 127) * 2 ** shift;
      if (!Number.isSafeInteger(value) || (shift > 0 && b === 0)) break;
      if (b < 128) return value;
    }
    throw new PackError("range integer exceeds the exact numeric range");
  }

  signed(ctx: number): number {
    const n = this.uint(ctx);
    return n % 2 === 0 ? n / 2 : -Math.floor(n / 2) - 1;
  }
}
