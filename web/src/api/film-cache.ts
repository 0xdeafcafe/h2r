// Per-reader cache. Both limits matter for a long film with many tiny
// event records or a short film with several large scene checkpoints.
export class FilmCache<T> {
  private readonly entries = new Map<number, {value: T; bytes: number}>();
  private bytes = 0;

  constructor(private readonly limit: number, private readonly maxBytes: number) {}

  get(key: number): T | undefined {
    const entry = this.entries.get(key);
    if (!entry) return undefined;
    this.entries.delete(key);
    this.entries.set(key, entry);
    return entry.value;
  }

  set(key: number, value: T, bytes: number) {
    const existing = this.entries.get(key);
    if (existing) {
      this.bytes -= existing.bytes;
      this.entries.delete(key);
    }
    if (bytes > this.maxBytes) return;
    this.entries.set(key, {value, bytes});
    this.bytes += bytes;
    while (this.bytes > this.maxBytes || this.entries.size > this.limit) {
      const oldest = this.entries.entries().next().value!;
      this.entries.delete(oldest[0]);
      this.bytes -= oldest[1].bytes;
    }
  }

  *values(): IterableIterator<T> {
    for (const entry of this.entries.values()) yield entry.value;
  }
}
