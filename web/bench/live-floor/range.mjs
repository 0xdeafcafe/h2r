// Binary arithmetic coding with E1/E2/E3 renormalization. The probability
// table lives on the connection; each packet terminates its own interval.
const HALF = 0x80000000,
  QUARTER = 0x40000000;
export class Range {
  low = 0;
  high = 0xffffffff;
  pending = 0;
  code = 0;
  at = 0;
  out = [];
  constructor(models, input) {
    this.models = models;
    this.input = input;
    if (input)
      for (let i = 0; i < 32; i++) this.code = this.code * 2 + this.read();
  }
  read() {
    const i = this.at++;
    return ((this.input[i >>> 3] ?? 0) >>> (7 - (i & 7))) & 1;
  }
  emit(b) {
    const i = this.at++;
    if (!(i & 7)) this.out.push(0);
    this.out[i >>> 3] |= b << (7 - (i & 7));
  }
  settled(b) {
    this.emit(b);
    while (this.pending > 0) {
      this.emit(1 - b);
      this.pending--;
    }
  }
  bit(context, wanted) {
    const index = context & (this.models.length - 1);
    const p = this.models[index] || 2048;
    const cut = this.low + Math.floor(((this.high - this.low + 1) * p) / 4096);
    const b = this.input ? +(this.code >= cut) : +!!wanted;
    this.models[index] = b
      ? p - ((p + 31) >>> 5) || 1
      : Math.min(4095, p + ((4096 - p + 31) >>> 5));
    if (b) this.low = cut;
    else this.high = cut - 1;
    for (;;) {
      let offset;
      if (this.high < HALF) offset = 0;
      else if (this.low >= HALF) offset = HALF;
      else if (this.low >= QUARTER && this.high < 3 * QUARTER) offset = QUARTER;
      else break;
      if (this.input) this.code = 2 * (this.code - offset) + this.read();
      else if (offset === QUARTER) this.pending++;
      else this.settled(+(offset === HALF));
      this.low = 2 * (this.low - offset);
      this.high = 2 * (this.high - offset) + 1;
    }
    return b;
  }
  // A magnitude prefix, then the mantissa, in independent adaptive contexts.
  uint(ctx, want = 0) {
    const n = this.input ? 0 : Math.floor(Math.log2(1 + want));
    let k = 0;
    while (this.bit(ctx + Math.min(16, k), k < n)) {
      if (++k > 52) throw Error("oversize integer");
    }
    let v = 1;
    for (let j = k - 1; j >= 0; j--) {
      v =
        v * 2 +
        this.bit(
          ctx + 18 + Math.min(j, 12),
          Math.floor((1 + want) / 2 ** j) % 2,
        );
    }
    return v - 1;
  }
  signed(ctx, want = 0) {
    const u = this.uint(ctx, want < 0 ? -2 * want - 1 : 2 * want);
    return u % 2 ? -(u + 1) / 2 : u / 2;
  }
  finish() {
    this.pending++;
    this.settled(+(this.low >= QUARTER));
    return Uint8Array.from(this.out);
  }
}
