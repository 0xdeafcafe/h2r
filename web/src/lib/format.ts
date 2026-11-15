export function clock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), r = s % 60;
  const mm = h ? String(m).padStart(2, "0") : String(m);
  return (h ? h + ":" : "") + mm + ":" + String(r).padStart(2, "0");
}

export function ago(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return "never";
  const d = Math.max(0, (now - Date.parse(iso)) / 1000);
  if (d < 10) return "just now";
  if (d < 60) return `${Math.floor(d)}s ago`;
  if (d < 3600) return `${Math.floor(d / 60)}m ago`;
  if (d < 86400) return `${Math.floor(d / 3600)}h ago`;
  return `${Math.floor(d / 86400)}d ago`;
}

export function titleCase(s: string | null | undefined): string {
  if (!s) return "";
  return s.replace(/[_-]+/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

/** Seconds between two ISO times (or until now). */
export function between(a: string, b: string | null, now = Date.now()): number {
  return ((b ? Date.parse(b) : now) - Date.parse(a)) / 1000;
}

/** The kill feed's wording for a knockout. */
export function killVerb(kind: string, _seed: number): string {
  switch (kind) {
    case "suicide": return "committed suicide";
    case "betrayal": return "betrayed";
    case "world": return "died";
    default: return "killed";
  }
}

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/);
  return ((parts[0]?.[0] ?? "?") + (parts[1]?.[0] ?? "")).toUpperCase();
}

/** A stable hue for an avatar. */
export function hueFor(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
  return h % 360;
}
