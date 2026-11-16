// Small pieces both sides use for clans and friends: the clan emblem (the
// leader's emblem from their games when known, else one picked from the
// clan id, on a clan-coloured field) and presence words for friends. A clan
// is the protocol's "team" (Xbox Live Teams), hence the type names.

import { mapTitle } from "../state/gameart";
import { Show } from "solid-js";
import Emblem from "../components/Emblem";
import { live } from "../state/live";
import { latestAppearance } from "../state/social-model";
import { hueFor } from "../lib/format";
import type { Team } from "../api/social";
import type { FriendEntry } from "../api/social";
import { presenceKind, type PresenceKind } from "../components/bits";
import type { AccountState } from "../state/reducer";

/** The team's colour: a hue from its id, so it is stable everywhere. */
export const teamHue = (t: Pick<Team, "id">) => hueFor(t.id);

export function ClanEmblem(props: { team: Team; size: number; class?: string }) {
  const appearance = () => latestAppearance(live.state, props.team.creator);
  return (
    <span class={`team-emblem ${props.class ?? ""}`} style={{ "--team-hue": String(teamHue(props.team)), width: `${props.size}px`, height: `${props.size}px` }}>
      <Emblem seed={props.team.id} appearance={appearance()} size={props.size} title={`${props.team.properties.name} emblem`} />
    </span>
  );
}

/** A friend's presence: the live account state when the stream knows it,
 *  else what the friends document said. */
export function friendPresence(f: Pick<FriendEntry, "xuid" | "presence">): { kind: PresenceKind; game_id: string | null; account: AccountState | undefined } {
  const a = live.state.accounts[f.xuid];
  if (a) return { kind: presenceKind(a), game_id: a.online ? a.presence?.game_id ?? null : null, account: a };
  if (!f.presence) return { kind: "offline", game_id: null, account: undefined };
  return { kind: f.presence.game_id ? "playing" : "online", game_id: f.presence.game_id, account: undefined };
}

export const PRESENCE_WORDS: Record<PresenceKind, string> = { playing: "in a game", online: "online", away: "away", offline: "offline" };

/** "playing Lockout" when the game is known, else the plain word. */
export function presenceLine(f: Pick<FriendEntry, "xuid" | "presence">): string {
  const p = friendPresence(f);
  if (p.kind === "playing" && p.game_id) {
    const g = live.state.games[p.game_id];
    if (g) return `playing ${mapTitle(g.map)}`;
  }
  return PRESENCE_WORDS[p.kind];
}

export function Dot(props: { kind: PresenceKind }) {
  return <Show when={props.kind !== "offline"}><span class={`soc-dot soc-dot--${props.kind}`} aria-hidden="true" /></Show>;
}
