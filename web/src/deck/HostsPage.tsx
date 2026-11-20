import { A } from "@solidjs/router";
import { Show, createMemo, createResource, createSignal, onCleanup } from "solid-js";
import { fetchHosts, stopHost, type HostsAdmin } from "../admin/hosts";
import { adminState } from "../admin/state";
import type { AdminHosts } from "../lib/hosts";
import { LIVE_ADMIN } from "../routes";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { DeckPage, Panel } from "./Deck";
import HostsBoard from "./HostsBoard";

const SERVER = "/dash/server";

export default function HostsPage() {
  const admin = adminState.isMeAdmin;
  const context = createMemo(() => {
    const me = identity.me(), wk = live.wellKnown();
    return admin() === true && me && wk && me.server_id === wk.id ? { api: live.server().api, aud: wk.id, me } : null;
  });

  return (
    <DeckPage title="Hosts" trail={[["dashboard", "/"], ["server", SERVER], ["live admin", LIVE_ADMIN], ["hosts"]]} back={SERVER}>
      <Show when={identity.me()} fallback={
        <Panel title="Sign in first"><p class="dk-note"><A href="/dash/signin">Sign in</A> with a server admin account to manage hosts.</p></Panel>
      }>
        <Show when={context()} keyed fallback={
          <Panel title={admin() === false ? "Admin access required" : "Checking admin access"}>
            <p class="dk-note">{admin() === false ? "Only this server's admins can manage hosts." : "Waiting for the server to confirm your access…"}</p>
          </Panel>
        }>
          {(ctx) => <HostsInventory context={ctx} />}
        </Show>
      </Show>
    </DeckPage>
  );
}

function message(error: unknown): string {
  return error instanceof Error ? error.message || "Request failed." : String(error);
}

type InventoryRead = { data: AdminHosts; error?: never } | { data?: never; error: string };

function HostsInventory(props: { context: HostsAdmin }) {
  const [hosts, { refetch }] = createResource(async (): Promise<InventoryRead> => {
    try {
      return { data: await fetchHosts(props.context) };
    } catch (error) {
      return { error: message(error) };
    }
  });
  const [stopping, setStopping] = createSignal<string | null>(null);
  const [stopError, setStopError] = createSignal<string | null>(null);
  let active = true;
  onCleanup(() => { active = false; });

  const busy = () => hosts.loading || stopping() !== null;

  async function stop(lease: string) {
    if (busy()) return;
    setStopping(lease);
    setStopError(null);
    try {
      const stopped = await stopHost(props.context, lease, (prompt) => window.confirm(prompt));
      if (stopped && active) await refetch();
    } catch (error) {
      if (active) setStopError(`Could not stop ${lease}. ${message(error)}`);
    } finally {
      if (active) setStopping(null);
    }
  }

  return (
    <>
      <div class="dk-span-12 dk-toolbar">
        <button type="button" class="dk-pill" onClick={() => void refetch()} disabled={busy()}>
          {hosts.loading ? "Reading hosts…" : "Refresh"}
        </button>
      </div>
      <Show when={stopError()}>{(error) => <div class="dk-span-12 dk-state dk-state--bad" role="alert">{error()}</div>}</Show>
      <Show when={hosts.latest?.error}>{(error) => <div class="dk-span-12 dk-state dk-state--bad" role="alert">Could not read hosts. {error()}</div>}</Show>
      <Show when={hosts.latest?.data}>
        {(data) => <HostsBoard data={data()} busy={busy()} stopping={stopping()} onStop={(lease) => void stop(lease)} />}
      </Show>
    </>
  );
}
