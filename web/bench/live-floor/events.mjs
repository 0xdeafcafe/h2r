// The seeded MP generator draws these values from its PRNG. Other event
// timing (e.g. the firing schedule) follows the fixture clock and player id.
// Fit marginal entropy to observed draws, giving the hypothetical cost if
// their frequencies and the entire deterministic scene were shared.
export function eventBits(frames) {
  const columns = new Map();
  function see(field, symbol) {
    let h = columns.get(field);
    if (!h) {
      h = new Map();
      columns.set(field, h);
    }
    h.set(symbol, (h.get(symbol) ?? 0) + 1);
  }
  for (const f of frames) {
    const hits = f.hits ?? [];
    see("hit count", hits.length);
    for (const h of hits) {
      for (const key of ["from", "to", "weapon", "amount", "ago"])
        see("hit " + key, h[key]);
      const i = f.players.findIndex((p) =>
        p.pos.every((v, j) => Object.is(v, h.pos[j])),
      );
      if (i < 0)
        throw Error(
          "expected the seeded MP hit to land on a player's position",
        );
      see("hit position slot", i);
    }
    for (const p of f.players) {
      if (p.shots !== undefined) see("shots when firing", p.shots);
      see("reload", !!p.actions?.length);
    }
  }
  let bits = 0;
  for (const h of columns.values()) {
    const n = [...h.values()].reduce((a, b) => a + b, 0);
    for (const v of h.values()) bits += v * Math.log2(n / v);
  }
  return bits;
}
