// The dashboard's friends and teams screens, list screens in the console
// style: a tilted list panel, the preview pod and a detail plate with the
// selected row's actions. Up / down pick a row, left / right pick an
// action, A runs it, B goes back. Writes are signed as the signed-in
// account (state/social.ts).

import { For, Match, Show, Switch, createEffect, createMemo, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { social, type WriteResult } from "../state/social";
import {
  MEMBER, PRIV, ROLE_LABEL, can, friendWrites, leaderOf, randomTeamId, roleOf, sortedMembers, teamPropsError, teamWrites,
  type Team, type TeamMember,
} from "../api/social";
import { gamertagError } from "../state/account";
import Emblem from "../components/Emblem";
import { ago } from "../lib/format";
import { now } from "../lib/now";
import { BigPod, Glyph, Plate, Prompts } from "./parts";
import type { Action } from "./input";
import { ListPanel } from "./Dashboard";
import { TeamEmblem, friendPresence, presenceLine } from "../stats/teamBits";

interface Row { key: string; label: string; meta?: string }
interface Act { label: string; run: () => void | Promise<unknown>; danger?: boolean }

export interface SocialProps {
  section: "friends" | "teams";
  tag?: string;
  go: (href: string, dir: "deeper" | "back") => void;
  back: () => void;
  /** Hands the screen's input handler to the dashboard. */
  bind: (fn: ((a: Action) => boolean) | null) => void;
}

const stats = (tag: string) => `/stats/players/${encodeURIComponent(tag)}`;

export default function SocialScreen(props: SocialProps) {
  return (
    <Switch>
      <Match when={props.section === "friends"}><FriendsScreen {...props} /></Match>
      <Match when={props.section === "teams" && !props.tag}><TeamsScreen {...props} /></Match>
      <Match when={props.section === "teams" && props.tag} keyed>{(id) => <TeamScreen {...props} id={id} />}</Match>
    </Switch>
  );
}

/** The screen frame every social list shares: list, pod, plate, prompts,
 *  and the keyboard / gamepad handling for them. */
function SocialList(props: SocialProps & {
  title: string; rows: Row[]; empty: string;
  preview: (r: Row | undefined) => JSX.Element;
  plate: (r: Row | undefined) => { small: string; title: string; body: JSX.Element };
  acts: (r: Row | undefined) => Act[];
  /** A row whose plate is a form: A submits it. */
  form?: (r: Row | undefined) => string | null;
  initial?: string;
}) {
  const [sel, setSel] = createSignal(0);
  const [ai, setAi] = createSignal(0);
  const [msg, setMsg] = createSignal<{ ok: boolean; text: string } | null>(null);
  const row = () => props.rows[sel()];
  const acts = createMemo(() => props.acts(row()));
  // Start on the row the route named, once the list has it.
  let placed = !props.initial;
  createEffect(() => {
    if (placed) return;
    const i = props.rows.findIndex((r) => r.key === props.initial || r.label === props.initial);
    if (i >= 0) { setSel(i); placed = true; }
  });
  createEffect(() => { if (sel() >= props.rows.length) setSel(Math.max(0, props.rows.length - 1)); });
  createEffect(() => { row()?.key; setAi(0); setMsg(null); });
  createEffect(() => { if (ai() >= acts().length) setAi(Math.max(0, acts().length - 1)); });

  async function run(a: Act | undefined) {
    if (!a) return;
    setMsg(null);
    const r = await a.run();
    const w = r as WriteResult | undefined;
    if (w && typeof w === "object" && "ok" in w) setMsg(w.ok ? { ok: true, text: "done" } : { ok: false, text: w.error });
  }
  function select() {
    const f = props.form?.(row());
    if (f) { (document.getElementById(f) as HTMLFormElement | null)?.requestSubmit(); return; }
    void run(acts()[ai()]);
  }
  function act(a: Action): boolean {
    const n = props.rows.length;
    switch (a) {
      case "down": setSel(Math.min(n - 1, sel() + 1)); return true;
      case "up": setSel(Math.max(0, sel() - 1)); return true;
      case "pageDown": setSel(Math.min(n - 1, sel() + 6)); return true;
      case "pageUp": setSel(Math.max(0, sel() - 6)); return true;
      case "right": if (acts().length) setAi((ai() + 1) % acts().length); return true;
      case "left": if (acts().length) setAi((ai() + acts().length - 1) % acts().length); return true;
      case "select": select(); return true;
      case "back": props.back(); return true;
      default: return false;
    }
  }
  onMount(() => props.bind(act));
  onCleanup(() => props.bind(null));
  const plate = createMemo(() => props.plate(row()));
  return (
    <div class="dx-screen dx-list dx-pages dx-social">
      <ListPanel title={props.title} rows={props.rows} sel={sel()} onSel={setSel} onAct={(i) => { setSel(i); select(); }} empty={props.empty} />
      <div class="dx-preview"><BigPod>{props.preview(row())}</BigPod></div>
      <Plate small={plate().small} title={plate().title}>
        {plate().body}
        <Show when={acts().length}>
          <div class="soc-acts" role="group" aria-label="actions">
            <For each={acts()}>
              {(a, i) => (
                <button type="button" class="soc-act" classList={{ on: ai() === i(), "soc-act--danger": !!a.danger }} disabled={social.busy()}
                  onMouseEnter={() => setAi(i())} onClick={() => { setAi(i()); void run(a); }}>{a.label}</button>
              )}
            </For>
          </div>
        </Show>
        <Show when={msg()}>{(m) => <p class={m().ok ? "soc-ok" : "dx-error"} role="status">{m().text}</p>}</Show>
      </Plate>
      <Prompts onA={select} onB={props.back} aLabel={props.form?.(row()) ? "send" : acts()[ai()]?.label ?? "select"} />
    </div>
  );
}

/** One-field form inside a plate (add friend, recruit). */
function TagForm(props: { id: string; label: string; button: string; onSubmit: (tag: string) => Promise<string | null> }) {
  const [v, setV] = createSignal("");
  const [err, setErr] = createSignal<string | null>(null);
  const [done, setDone] = createSignal<string | null>(null);
  async function submit(e: Event) {
    e.preventDefault();
    setErr(null); setDone(null);
    const t = v().trim();
    const bad = gamertagError(t);
    if (bad) { setErr(`gamertag: ${bad}`); return; }
    const r = await props.onSubmit(t);
    if (r) setErr(r); else { setDone(`sent to ${t}`); setV(""); }
  }
  return (
    <form id={props.id} class="dx-form soc-form" onSubmit={submit}>
      <label for={props.id + "-in"}>{props.label}</label>
      <input id={props.id + "-in"} value={v()} maxLength={15} onInput={(e) => setV(e.currentTarget.value)} autocomplete="off" spellcheck={false} placeholder="gamertag" />
      <button type="submit" class="dx-go" disabled={social.busy() || !v().trim()}>{props.button}</button>
      <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
      <Show when={done()}><p class="soc-ok" role="status">{done()}</p></Show>
    </form>
  );
}

const resultText = (r: WriteResult) => (r.ok ? null : r.error);

// ---------- friends ----------

function FriendsScreen(props: SocialProps) {
  const doc = () => social.mine();
  const rows = createMemo((): Row[] => {
    const d = doc();
    if (!identity.me()) return [];
    const out: Row[] = [{ key: "+add", label: "add friend", meta: "by gamertag" }, { key: "+teams", label: "teams", meta: "xbox live teams" }];
    if (!d) return out;
    for (const r of d.requests_in) out.push({ key: "in:" + r.xuid, label: r.gamertag, meta: "wants to be friends" });
    const fs = [...d.friends].sort((a, b) => Number(friendPresence(b).kind !== "offline") - Number(friendPresence(a).kind !== "offline"));
    for (const f of fs) out.push({ key: "f:" + f.xuid, label: f.gamertag, meta: presenceLine(f) });
    for (const r of d.requests_out) out.push({ key: "out:" + r.xuid, label: r.gamertag, meta: "request sent" });
    return out;
  });
  const xuidOf = (r: Row | undefined) => r?.key.split(":")[1] ?? "";
  const kindOf = (r: Row | undefined) => r?.key.split(":")[0] ?? "";
  const me = () => identity.me()!;
  const [confirm, setConfirm] = createSignal<string | null>(null);
  return (
    <SocialList {...props} title="friends" rows={rows()} initial={props.tag}
      empty={identity.me() ? "loading friends" : "sign up to add friends"}
      form={(r) => (r?.key === "+add" ? "dash-add-friend" : null)}
      preview={(r) => (
        <Show when={r && r.key !== "+add" && r.key !== "+teams"} fallback={<FriendGlyph />}>
          <Emblem seed={xuidOf(r)} size={220} class="dx-emblem" />
        </Show>
      )}
      plate={(r) => {
        if (!identity.me()) return { small: "friends", title: "not signed in", body: <p class="soc-note">Sign up on this server to keep a friends list. It is the same list the game shows.</p> };
        if (!r || r.key === "+add") return {
          small: "friends", title: "add friend",
          body: <TagForm id="dash-add-friend" label="gamertag" button="send request" onSubmit={async (t) => {
            const who = await social.lookup(t).catch(() => null);
            if (!who) return "no account has that gamertag";
            return resultText(await social.write(friendWrites.request(me().xuid, who.xuid)));
          }} />,
        };
        if (r.key === "+teams") return { small: "friends", title: "teams", body: <p class="soc-note">Teams on this server: the ones you lead, belong to or are invited to, and every other team.</p> };
        const k = kindOf(r), x = xuidOf(r);
        const f = doc()?.friends.find((y) => y.xuid === x);
        const req = doc()?.requests_in.find((y) => y.xuid === x) ?? doc()?.requests_out.find((y) => y.xuid === x);
        return {
          small: k === "in" ? "friend request" : k === "out" ? "request sent" : friendPresence({ xuid: x, presence: f?.presence ?? null }).kind,
          title: r.label,
          body: (
            <dl>
              <Show when={f} fallback={<><dt>asked</dt><dd>{ago(req?.at, now())}</dd></>}>
                {(fr) => <><dt>status</dt><dd>{presenceLine(fr())}</dd><dt>friends since</dt><dd>{ago(fr().friends_since, now())}</dd></>}
              </Show>
            </dl>
          ),
        };
      }}
      acts={(r): Act[] => {
        if (!identity.me()) return [{ label: "sign up", run: () => props.go("/dash/signup", "deeper") }];
        if (!r || r.key === "+add") return [];
        if (r.key === "+teams") return [{ label: "open teams", run: () => props.go("/dash/teams", "deeper") }];
        const k = kindOf(r), x = xuidOf(r);
        const profile: Act = { label: "profile", run: () => props.go(`/dash/players/${encodeURIComponent(r.label)}`, "deeper") };
        if (k === "in") return [
          { label: "accept", run: () => social.write(friendWrites.accept(me().xuid, x)) },
          { label: "decline", run: () => social.write(friendWrites.decline(me().xuid, x)), danger: true },
          profile,
        ];
        if (k === "out") return [{ label: "cancel request", run: () => social.write(friendWrites.remove(me().xuid, x)), danger: true }, profile];
        const p = friendPresence({ xuid: x, presence: doc()?.friends.find((y) => y.xuid === x)?.presence ?? null });
        const out: Act[] = [];
        if (p.game_id) out.push({ label: "watch game", run: () => props.go(`/stats/games/${p.game_id}`, "deeper") });
        out.push(profile, { label: "service record", run: () => props.go(stats(r.label), "deeper") });
        out.push(confirm() === x
          ? { label: "really remove", danger: true, run: () => { setConfirm(null); return social.write(friendWrites.remove(me().xuid, x)); } }
          : { label: "remove", danger: true, run: () => { setConfirm(x); } });
        return out;
      }}
    />
  );
}

function FriendGlyph() {
  return (
    <svg class="dx-glyph soc-glyph" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M8.5 11A3.5 3.5 0 1 1 8.4 11M2 21C2 17 5 15 8.5 15S15 17 15 21M16 4A3 3 0 1 1 15.9 4M17.5 13C20 13.5 22 15.3 22 19M19 7V13M16 10H22" transform="translate(0 -1)" />
    </svg>
  );
}

// ---------- teams ----------

function TeamsScreen(props: SocialProps) {
  const mine = () => { const me = identity.me(); return me ? social.teamsOf(me.xuid) : []; };
  const rows = createMemo((): Row[] => {
    const me = identity.me();
    const out: Row[] = me ? [{ key: "+create", label: "create a team", meta: "you lead it" }] : [];
    const seen = new Set<string>();
    for (const t of mine()) {
      seen.add(t.id);
      const role = roleOf(t, me?.xuid);
      out.push({ key: t.id, label: t.properties.name, meta: role === "recruit" ? "invitation" : ROLE_LABEL[role ?? "member"].toLowerCase() });
    }
    for (const t of social.teams().sort((a, b) => a.properties.name.localeCompare(b.properties.name))) {
      if (seen.has(t.id)) continue;
      const n = Object.keys(t.members).length;
      out.push({ key: t.id, label: t.properties.name, meta: `${n} member${n === 1 ? "" : "s"}` });
    }
    return out;
  });
  const team = (r: Row | undefined) => (r && r.key !== "+create" ? social.team(r.key) : null);
  return (
    <SocialList {...props} title="teams" rows={rows()}
      empty={social.teamsLoaded() ? "no teams on this server yet" : "loading teams"}
      form={(r) => (r?.key === "+create" ? "dash-create-team" : null)}
      preview={(r) => <Show when={team(r)} keyed fallback={<Glyph name="career" />}>{(t) => <TeamEmblem team={t} size={220} class="dx-emblem" />}</Show>}
      plate={(r) => {
        const t = team(r);
        if (!t) return { small: "new team", title: "create a team", body: <CreateTeamForm onCreated={(id) => props.go(`/dash/teams/${id}`, "deeper")} /> };
        const role = roleOf(t, identity.me()?.xuid);
        return {
          small: role ? (role === "recruit" ? "invited" : ROLE_LABEL[role]) : "team",
          title: t.properties.name,
          body: <TeamFacts team={t} />,
        };
      }}
      acts={(r): Act[] => {
        const t = team(r);
        if (!t) return [];
        const open: Act = { label: "open", run: () => props.go(`/dash/teams/${t.id}`, "deeper") };
        const page: Act = { label: "team page", run: () => props.go(`/stats/teams/${t.id}`, "deeper") };
        if (roleOf(t, identity.me()?.xuid) === "recruit") return [
          { label: "join", run: () => social.write(teamWrites.answer(t.id, true)) },
          { label: "decline", danger: true, run: () => social.write(teamWrites.answer(t.id, false)) },
          open,
        ];
        return [open, page];
      }}
    />
  );
}

function TeamFacts(props: { team: Team }) {
  const t = () => props.team;
  const online = () => Object.keys(t().members).filter((x) => live.state.accounts[x]?.online).length;
  return (
    <dl>
      <Show when={t().properties.motto}><dt>motto</dt><dd>{t().properties.motto}</dd></Show>
      <dt>leader</dt><dd>{leaderOf(t())?.identity.gamertag ?? "none"}</dd>
      <dt>members</dt><dd>{Object.keys(t().members).length} <span>on the team</span> {online()} <span>online</span></dd>
      <dt>founded</dt><dd>{ago(t().created_at, now())}</dd>
    </dl>
  );
}

function CreateTeamForm(props: { onCreated: (id: string) => void }) {
  const [name, setName] = createSignal("");
  const [motto, setMotto] = createSignal("");
  const [err, setErr] = createSignal<string | null>(null);
  async function submit(e: Event) {
    e.preventDefault();
    setErr(null);
    const p = { name: name().trim(), motto: motto().trim(), description: "", url: "" };
    const bad = teamPropsError(p);
    if (bad) { setErr(bad); return; }
    const id = randomTeamId();
    const r = await social.write(teamWrites.create(id, p));
    if (r.ok) props.onCreated(id); else setErr(r.error);
  }
  return (
    <form id="dash-create-team" class="dx-form soc-form" onSubmit={submit}>
      <label for="dash-team-name">name</label>
      <input id="dash-team-name" value={name()} maxLength={15} onInput={(e) => setName(e.currentTarget.value)} autocomplete="off" spellcheck={false} placeholder="up to 15 characters" />
      <label for="dash-team-motto">motto</label>
      <input id="dash-team-motto" value={motto()} maxLength={255} onInput={(e) => setMotto(e.currentTarget.value)} autocomplete="off" placeholder="optional" />
      <button type="submit" class="dx-go" disabled={social.busy() || !name().trim()}>create team</button>
      <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
    </form>
  );
}

function TeamScreen(props: SocialProps & { id: string }) {
  const t = () => social.team(props.id);
  const me = () => identity.me();
  const rows = createMemo((): Row[] => {
    const team = t();
    if (!team) return [];
    const out: Row[] = can(team, me()?.xuid, PRIV.recruit) ? [{ key: "+recruit", label: "recruit", meta: "by gamertag" }] : [];
    for (const m of sortedMembers(team)) {
      const role = roleOf(team, m.identity.xuid);
      const a = live.state.accounts[m.identity.xuid];
      out.push({ key: m.identity.xuid, label: m.identity.gamertag, meta: `${ROLE_LABEL[role ?? "member"].toLowerCase()}${a?.online ? " · online" : ""}` });
    }
    for (const r of Object.values(team.recruits)) out.push({ key: r.identity.xuid, label: r.identity.gamertag, meta: "invited" });
    return out;
  });
  const member = (r: Row | undefined): TeamMember | undefined => {
    const team = t();
    return team && r ? team.members[r.key] ?? team.recruits[r.key] : undefined;
  };
  return (
    <Show when={t()} fallback={<TeamMissing {...props} />}>
      {(team) => (
        <SocialList {...props} title={team().properties.name} rows={rows()} empty="nobody on this team"
          back={() => props.go("/dash/teams", "back")}
          form={(r) => (r?.key === "+recruit" ? "dash-recruit" : null)}
          preview={() => <TeamEmblem team={team()} size={220} class="dx-emblem" />}
          plate={(r) => {
            const m = member(r);
            if (!m) return {
              small: team().properties.motto || "team", title: "recruit",
              body: <TagForm id="dash-recruit" label="gamertag" button="recruit" onSubmit={async (tag) => {
                const who = await social.lookup(tag).catch(() => null);
                if (!who) return "no account has that gamertag";
                return resultText(await social.write(teamWrites.recruit(team().id, who, MEMBER)));
              }} />,
            };
            const role = roleOf(team(), m.identity.xuid);
            const p = friendPresence({ xuid: m.identity.xuid, presence: null });
            return {
              small: role === "recruit" ? "invited" : ROLE_LABEL[role ?? "member"], title: m.identity.gamertag,
              body: (
                <dl>
                  <dt>status</dt><dd>{presenceLine({ xuid: m.identity.xuid, presence: null })}</dd>
                  <dt>{role === "recruit" ? "invited" : "joined"}</dt><dd>{ago(m.joined_at, now())}</dd>
                  <dt>team</dt><dd>{team().properties.name}</dd>
                  <Show when={p.kind === "offline" && p.account?.last_seen}><dt>seen</dt><dd>{ago(p.account!.last_seen, now())}</dd></Show>
                </dl>
              ),
            };
          }}
          acts={(r): Act[] => {
            const m = member(r);
            const x = me()?.xuid;
            if (!m) return [];
            const tm = team();
            const target = m.identity.xuid;
            const role = roleOf(tm, target);
            const out: Act[] = [];
            if (x === target && role === "recruit") {
              out.push({ label: "join", run: () => social.write(teamWrites.answer(tm.id, true)) });
              out.push({ label: "decline", danger: true, run: () => social.write(teamWrites.answer(tm.id, false)) });
            }
            const p = friendPresence({ xuid: target, presence: null });
            if (p.game_id) out.push({ label: "watch game", run: () => props.go(`/stats/games/${p.game_id}`, "deeper") });
            out.push({ label: "profile", run: () => props.go(`/dash/players/${encodeURIComponent(m.identity.gamertag)}`, "deeper") });
            if (x === target && role !== "recruit" && role !== "leader") out.push({ label: "leave team", danger: true, run: () => social.write(teamWrites.remove(tm.id, x)) });
            if (x && x !== target && target !== tm.creator && can(tm, x, PRIV.removeMember)) {
              out.push({ label: role === "recruit" ? "cancel invite" : "remove", danger: true, run: () => social.write(teamWrites.remove(tm.id, target)) });
            }
            return out;
          }}
        />
      )}
    </Show>
  );
}

function TeamMissing(props: SocialProps) {
  onMount(() => props.bind((a) => { if (a === "back" || a === "select") { props.go("/dash/teams", "back"); return true; } return false; }));
  onCleanup(() => props.bind(null));
  return (
    <div class="dx-screen dx-pods dx-pages">
      <div class="dx-centre"><BigPod wire><Glyph name="empty" /></BigPod></div>
      <Plate small="teams" title={social.teamsLoaded() ? "no such team" : "loading"} main>
        <p class="soc-note">{social.teamsLoaded() ? "That team is gone, or was never here." : "Reading this server's teams."}</p>
      </Plate>
      <Prompts onB={() => props.go("/dash/teams", "back")} />
    </div>
  );
}
