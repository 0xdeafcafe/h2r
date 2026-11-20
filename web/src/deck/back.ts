/** Marks an entry pushed by a dashboard or deck link. The flag travels with
 *  that entry through browser Back, Forward and reload. */
export const DECK_ENTRY = { fromDashboard: true } as const;

export function deckBackTarget(state: { fromDashboard?: boolean } | null, fallback: string): -1 | string {
  return state?.fromDashboard === true ? -1 : fallback;
}
