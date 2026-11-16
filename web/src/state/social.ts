// The friends and teams store. Reads the server's documents, follows the
// stream (friend events on account streams, team events on team streams)
// with the pure folds in social-model.ts so pages change without a reload,
// and re-reads shortly after each change to stay exact. Writes are signed
// events (state/sign.ts) as the signed-in account.

import { createEffect, createRoot, createSignal, on } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import { live } from "./live";
import { identity } from "./identity";
import { postEvent, WriteError } from "./sign";
import { applyFriendEvent, applyTeamEvent, friendCounterpart, teamIdOf } from "./social-model";
import {
  FRIEND_EVENT_TYPES, TEAM_EVENT_TYPES, emptyFriends, socialApi, teamErrorText,
  type EventWrite, type FriendsDoc, type Team,
} from "../api/social";
import type { WireEvent, Xuid } from "../api/types";

export type WriteResult = { ok: true } | { ok: false; error: string };

function createSocial() {
  const api = () => socialApi(live.server());
  const tagOf = (x: Xuid) => live.state.accounts[x]?.gamertag ?? x;

  // ----- friends: the signed-in account's document, and others' on demand -----
  const [friendDocs, setFriendDocs] = createStore<Record<Xuid, FriendsDoc>>({});
  const [loaded, setLoaded] = createSignal<Record<Xuid, boolean>>({});
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  const later = (key: string, fn: () => void, ms = 500) => {
    clearTimeout(timers.get(key));
    timers.set(key, setTimeout(() => { timers.delete(key); fn(); }, ms));
  };

  function loadFriends(xuid: Xuid, gamertag: string) {
    const srv = live.server().base;
    void api().friends(gamertag).then((d) => {
      if (live.server().base !== srv) return;
      setFriendDocs(xuid, reconcile(d));
      setLoaded((m) => ({ ...m, [xuid]: true }));
    }, () => setLoaded((m) => ({ ...m, [xuid]: true })));
  }

  /** `xuid`'s friends document (loaded on first ask), or null while loading. */
  function friendsOf(xuid: Xuid, gamertag: string): FriendsDoc | null {
    if (!(xuid in loaded()) && !timers.has("f-" + xuid)) later("f-" + xuid, () => loadFriends(xuid, gamertag), 0);
    return loaded()[xuid] ? friendDocs[xuid] ?? emptyFriends() : null;
  }

  const mine = () => { const me = identity.me(); return me ? friendsOf(me.xuid, me.gamertag) : null; };

  // ----- teams: every active team, kept current from the stream -----
  const [teams, setTeams] = createStore<Record<string, Team>>({});
  const [teamsLoaded, setTeamsLoaded] = createSignal(false);
  const [teamsError, setTeamsError] = createSignal<string | null>(null);
  let teamsAsked = false;

  function loadTeams() {
    teamsAsked = true;
    const srv = live.server().base;
    void api().teams().then((list) => {
      if (live.server().base !== srv) return;
      setTeams(reconcile(Object.fromEntries(list.map((t) => [t.id, t]))));
      setTeamsLoaded(true);
      setTeamsError(null);
    }, (e) => { setTeamsLoaded(true); setTeamsError((e as Error).message); });
  }
  function reloadTeam(id: string) {
    void api().team(id).then((t) => {
      if (t) setTeams(id, reconcile(t));
      else setTeams(produce((s) => { delete s[id]; }));
    }, () => {});
  }
  const ensureTeams = () => { if (!teamsAsked) loadTeams(); };

  // A server change starts over.
  createEffect(on(() => live.server().base, () => {
    setFriendDocs(reconcile({}));
    setLoaded({});
    setTeams(reconcile({}));
    setTeamsLoaded(false);
    if (teamsAsked) loadTeams();
  }, { defer: true }));

  function onEvent(ev: WireEvent) {
    if (FRIEND_EVENT_TYPES.has(ev.type)) {
      for (const xuid of Object.keys(loaded())) {
        const other = friendCounterpart(xuid, ev);
        if (!other) continue;
        const doc = friendDocs[xuid];
        if (doc) setFriendDocs(xuid, reconcile(applyFriendEvent(JSON.parse(JSON.stringify(doc)) as FriendsDoc, xuid, ev, tagOf)));
        const tag = tagOf(xuid);
        later("f-" + xuid, () => loadFriends(xuid, tag));
      }
      return;
    }
    if (TEAM_EVENT_TYPES.has(ev.type)) {
      const id = teamIdOf(ev);
      if (!id || !teamsAsked) return;
      const prev = teams[id] ? (JSON.parse(JSON.stringify(teams[id])) as Team) : null;
      const next = applyTeamEvent(prev, ev, tagOf);
      if (next && !next.deleted) setTeams(id, reconcile(next));
      else if (next?.deleted) setTeams(produce((s) => { delete s[id]; }));
      later("t-" + id, () => reloadTeam(id));
      return;
    }
    // The server's note to a recruit; the team stream says the same, but a
    // missed one is caught here.
    if (ev.type === "TeamRecruitReceived" && teamsAsked) {
      const id = String((ev.data as { team_id?: unknown })?.team_id ?? "").toLowerCase();
      if (id) later("t-" + id, () => reloadTeam(id));
    }
  }
  live.onEvent(onEvent);

  // ----- writes -----
  const [busy, setBusy] = createSignal(false);
  async function write(w: EventWrite): Promise<WriteResult> {
    const me = identity.me();
    const wk = live.wellKnown();
    if (!me) return { ok: false, error: "sign in first" };
    if (!wk) return { ok: false, error: "this server didn't say who it is; try again in a moment" };
    setBusy(true);
    try {
      await postEvent(live.server().api, wk.id, me, w);
      // The stream brings the event back; re-read as well in case it lags.
      later("f-" + me.xuid, () => loadFriends(me.xuid, me.gamertag), 300);
      const tid = teamIdOf(w);
      if (tid) later("t-" + tid, () => reloadTeam(tid), 300);
      return { ok: true };
    } catch (e) {
      if (e instanceof WriteError) return { ok: false, error: teamErrorText(e.teamError, e.message || e.code) };
      return { ok: false, error: (e as Error).message };
    } finally {
      setBusy(false);
    }
  }

  return {
    friendsOf,
    mine,
    teams: () => { ensureTeams(); return Object.values(teams).filter((t) => !t.deleted); },
    team: (id: string): Team | null => { ensureTeams(); return teams[id.toLowerCase()] ?? null; },
    /** Teams `xuid` is on or recruited to. */
    teamsOf: (xuid: Xuid): Team[] => { ensureTeams(); return Object.values(teams).filter((t) => !t.deleted && (t.members[xuid] || t.recruits[xuid])); },
    teamsLoaded,
    teamsError,
    lookup: (gamertag: string) => api().identity(gamertag),
    write,
    busy,
  };
}

export const social = createRoot(createSocial);
