// The one-line summary beside the Games page's skull filter button.

import { skullName } from "../lib/skulls";

/** "Skulls: Assassins, Thunderstorm" for up to two, "3 skulls" past that, "" for none. */
export function skullSummary(wanted: readonly number[]): string {
  if (!wanted.length) return "";
  if (wanted.length <= 2) return `Skulls: ${wanted.map(skullName).join(", ")}`;
  return `${wanted.length} skulls`;
}
