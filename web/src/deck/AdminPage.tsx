import { A } from "@solidjs/router";
import { For, Show, createMemo, createResource, onCleanup, type JSX } from "solid-js";
import { fetchHosts, type HostsAdmin } from "../admin/hosts";
import { LIVE_ADMIN_USAGE } from "../admin/server";
import { adminState } from "../admin/state";
import { fetchUsage, type UsageOverview } from "../admin/usage";
import type { WireEvent } from "../api/types";
import { eventSummary, readEventWindow } from "../lib/eventLog";
import type { AdminHosts } from "../lib/hosts";
import { LIVE_ADMIN } from "../routes";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { DECK_ENTRY } from "./back";
import { bytes, compact } from "./chart";
import { DeckPage, Panel } from "./Deck";
import "../styles/deck-admin.css";

const USAGE_DAYS = 7;
const RECENT_EVENT_COUNT = 3;

export default function AdminPage() {
  const admin = adminState.isMeAdmin;
  const context = createMemo(() => {
    const me = identity.me(), wk = live.wellKnown();
    return admin() === true && me && wk && me.server_id === wk.id ? { api: live.server().api, aud: wk.id, me } : null;
  });

  return (
    <DeckPage title="live admin" trail={[["dashboard", "/"], ["server", "/dash/server"], ["live admin"]]} back="/dash/settings">
      <Show when={admin() === true && identity.me()} fallback={
        <Panel title="Admin access" span={6}>
          <p class="dk-note">
            <Show when={identity.me()} fallback={<><A href="/dash/signin">Sign in</A> with a server admin account.</>}>
              {admin() === null ? "Checking admin access…" : "These tools are available to server admins."}
            </Show>
          </p>
        </Panel>
      }>
        <Show when={context()} keyed fallback={<AdminCards />}>
          {(ctx) => <AdminOverview context={ctx} />}
        </Show>
      </Show>
    </DeckPage>
  );
}

function AdminOverview({ context }: { context: HostsAdmin }) {
  const abort = new AbortController();
  onCleanup(() => abort.abort());
  const [usage] = createResource(() =>
    fetchUsage(context.api, context.aud, context.me, USAGE_DAYS).catch(() => null));
  const [hosts] = createResource(() => fetchHosts(context).catch(() => null));
  const [events] = createResource(async () => {
    try {
      const api = live.api();
      const { position } = await api.events({ limit: 1 }, abort.signal);
      const recent = await readEventWindow(api, position ?? live.state.position, abort.signal);
      return recent.slice(-RECENT_EVENT_COUNT).reverse();
    } catch {
      return null;
    }
  });

  return <AdminCards usage={usage()} hosts={hosts()} events={events()} />;
}

export function AdminCards(props: {
  usage?: UsageOverview | null;
  hosts?: AdminHosts | null;
  events?: WireEvent[] | null;
}) {
  return (
    <>
      <AdminCard id="usage" title="Usage" href={LIVE_ADMIN_USAGE}>
        <Summary data={props.usage} loading="Reading usage…" error="Usage is unavailable.">
          {(usage) => {
            const uploaded = Object.values(usage.totals.up).reduce((total, count) => total + count, 0);
            const { events } = usage.totals;
            const active = usage.accounts_active;
            return (
              <Show when={uploaded || events || active} fallback={<p class="dk-note">No usage recorded for the last {USAGE_DAYS} days.</p>}>
                <p class="dk-note">Last {USAGE_DAYS} days</p>
                <Headlines items={[
                  ["Active accounts", compact(active)],
                  ["Events written", compact(events)],
                  ["Data uploaded", bytes(uploaded)],
                ]} />
              </Show>
            );
          }}
        </Summary>
      </AdminCard>
      <AdminCard id="hosts" title="Hosts" href={`${LIVE_ADMIN}/hosts`}>
        <Summary data={props.hosts} loading="Reading hosts…" error="Host inventory is unavailable.">
          {(hosts) => (
            <>
              <Headlines items={[
                ["Running hosts", compact(hosts.running.length)],
                ["Players connected", compact(hosts.running.reduce((total, host) => total + host.players, 0))],
                ["Recent failures", compact(hosts.recent.filter((host) => host.state === "failed").length)],
              ]} />
              <Show when={!hosts.running.length}><p class="dk-note">No dedicated hosts are currently running.</p></Show>
            </>
          )}
        </Summary>
      </AdminCard>
      <AdminCard id="events" title="Event Log" href={`${LIVE_ADMIN}/events`}>
        <Summary data={props.events} loading="Reading events…" error="The event log is unavailable.">
          {(events) => (
            <Show when={events.length} fallback={<p class="dk-note">No visible events in the latest log window.</p>}>
              <ul class="dk-admin-events">
                <For each={events}>{(event) => {
                  const summary = eventSummary(event);
                  return (
                    <li>
                      <div><b>{event.type}</b><Show when={summary}><span> · {summary}</span></Show></div>
                      <time dateTime={event.at}>{event.at.slice(0, 16).replace("T", " ")} UTC</time>
                    </li>
                  );
                }}</For>
              </ul>
            </Show>
          )}
        </Summary>
      </AdminCard>
      <AdminCard id="style" title="Style" href={`${LIVE_ADMIN}/style`}>
        <p class="dk-note">Preview the dashboard’s panels, charts, and tables with sample data.</p>
      </AdminCard>
    </>
  );
}

function AdminCard(props: { id: string; title: string; href: string; children: JSX.Element }) {
  const heading = `dk-admin-${props.id}`;
  return (
    <A class="dk-panel dk-span-6 dk-admin-card" href={props.href} state={DECK_ENTRY} aria-labelledby={heading}>
      <div class="dk-face">
        <header class="dk-panel-head">
          <h2 id={heading}>{props.title}</h2>
          <span aria-hidden="true">›</span>
        </header>
        {props.children}
      </div>
    </A>
  );
}

function Summary<T>(props: { data: T | null | undefined; loading: string; error: string; children: (data: T) => JSX.Element }) {
  return (
    <Show when={props.data} fallback={<p class="dk-note" role="status">{props.data === null ? props.error : props.loading}</p>}>
      {(data) => props.children(data())}
    </Show>
  );
}

function Headlines(props: { items: [string, string][] }) {
  return (
    <dl class="dk-admin-headlines">
      <For each={props.items}>{([label, value]) => <div><dt>{label}</dt><dd>{value}</dd></div>}</For>
    </dl>
  );
}
