// Date and number formats the stats pages share (US-style, as the
// record pages of the era wrote them).

const pad = (n: number) => String(n).padStart(2, "0");

/** 9/7/2007 */
export function usDate(iso: string | null | undefined): string {
  if (!iso) return "–";
  const d = new Date(iso);
  return `${d.getMonth() + 1}/${d.getDate()}/${d.getFullYear()}`;
}

/** 9/7/2007 10:28:29 AM */
export function usDateTime(iso: string | null | undefined): string {
  if (!iso) return "–";
  const d = new Date(iso);
  const h = d.getHours();
  return `${usDate(iso)} ${h % 12 || 12}:${pad(d.getMinutes())}:${pad(d.getSeconds())} ${h < 12 ? "AM" : "PM"}`;
}

/** 9.06.2007 */
export function dotDate(iso: string | null | undefined): string {
  if (!iso) return "–";
  const d = new Date(iso);
  return `${d.getMonth() + 1}.${pad(d.getDate())}.${d.getFullYear()}`;
}

/** Friday, September 07, 2007 */
export function longDate(iso: string | null | undefined): string {
  if (!iso) return "–";
  const d = new Date(iso);
  const day = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"][d.getDay()];
  const month = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"][d.getMonth()];
  return `${day}, ${month} ${pad(d.getDate())}, ${d.getFullYear()}`;
}

/** 00:19:59 */
export function hms(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  return `${pad(Math.floor(s / 3600))}:${pad(Math.floor((s % 3600) / 60))}:${pad(s % 60)}`;
}

export function num(n: number): string {
  return n.toLocaleString("en-US");
}

export function ratio(kills: number, deaths: number): string {
  return deaths ? (kills / deaths).toFixed(2) : kills.toFixed(2);
}

/** "5 minutes ago", for sentences. */
export function agoWords(iso: string | null | undefined, now: number): string {
  if (!iso) return "never";
  const d = Math.max(0, (now - Date.parse(iso)) / 1000);
  const say = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"} ago`;
  if (d < 60) return "moments ago";
  if (d < 3600) return say(Math.floor(d / 60), "minute");
  if (d < 86400) return say(Math.floor(d / 3600), "hour");
  return say(Math.floor(d / 86400), "day");
}
