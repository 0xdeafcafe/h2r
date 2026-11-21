import type { Kill } from "../state/reducer";

export type FeedKill = Pick<Kill, "position" | "tick" | "victim" | "killer" | "weapon" | "headshot" | "kind">;

export function killFeedRows(kills: readonly FeedKill[], limit: number) {
  const first = Math.max(0, kills.length - Math.max(0, limit));
  const visible = kills.slice(first);
  const counts = new Map<string, number>();
  const fingerprint = (kill: FeedKill) => `${kill.tick}/${kill.victim}/${kill.killer}/${kill.kind}`;
  const old = new Set(visible.filter((kill) => kill.position <= 0).map(fingerprint));
  // Legacy snapshots can lack a log position. Assign duplicates in event
  // order, including ones that have already fallen out of the visible feed.
  if (old.size) {
    for (let i = 0; i < first; i++) {
      const kill = kills[i]!;
      if (kill.position > 0) continue;
      const key = fingerprint(kill);
      if (old.has(key)) counts.set(key, (counts.get(key) ?? 0) + 1);
    }
  }
  return visible.map((kill) => {
    if (kill.position > 0) return { key: `event/${kill.position}`, kill };
    const fingerprintKey = fingerprint(kill);
    const occurrence = counts.get(fingerprintKey) ?? 0;
    counts.set(fingerprintKey, occurrence + 1);
    return { key: `legacy/${fingerprintKey}/${occurrence}`, kill };
  }).reverse();
}
