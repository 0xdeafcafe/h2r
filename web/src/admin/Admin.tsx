// The Halo admin area, a stats-site page at /games/halo2/admin shown only to
// admins: the message-of-the-day editor, the playlist (hopper) builder and
// custom content moderation and Clanker Training. Only
// the game's own things live here; running the service (invites, accounts
// and admins, importing an account, the server's status) is the dashboard's
// "server" screens, linked from the title bar (./tabs.ts).

import { Show, lazy } from "solid-js";
import { A, useSearchParams } from "@solidjs/router";
import { Crumbs, Panel, Tabs, Title } from "../stats/Layout";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { adminState } from "./state";
import MotdEditor from "./MotdEditor";
import HopperBuilder from "./HopperBuilder";
import ContentAdmin from "./ContentAdmin";
import { ADMIN_TABS, IMPORT_ACCOUNT, SERVER_ADMIN, adminTab } from "./tabs";
import "../styles/admin.css";
import { STATS_ROOT } from "../routes";

const ClankerTraining = lazy(() => import("./ClankerTraining"));

export default function Admin() {
  const [params, setParams] = useSearchParams<{ tab?: string; run?: string; batch?: string }>();
  const tab = () => adminTab(params.tab ?? null);
  const setTab = (id: string) => setParams({ tab: id === "motd" ? undefined : id, run: undefined, batch: undefined });
  const admin = adminState.isMeAdmin;
  return (
    <div class="s-full">
      <Crumbs trail={[["Home", STATS_ROOT], ["Halo Admin"]]} />
      <Title class="s-title--bar" right={<A class="adm-server-link" href={SERVER_ADMIN}>Server admin ›</A>}>
        Halo Admin
      </Title>
      <div class="adm">
        <Show
          when={identity.me()}
          fallback={
            <Panel title="Sign in first">
              <p class="adm-note">
                The Halo admin area needs a signed-in admin account. Sign in on the{" "}
                <A href="/dash/signin">dashboard</A>; an account made in the game can be brought into
                this browser there too (<A href={IMPORT_ACCOUNT}>import account</A>).
              </p>
            </Panel>
          }
        >
          {(me) => (
            <Show
              when={admin() === true}
              fallback={
                <Panel title={admin() === null ? "Checking…" : "Not an admin"}>
                  <Show
                    when={admin() === null}
                    fallback={
                      <p class="adm-note">
                        <b>{me().gamertag}</b> is not an admin on this server. Admins are chosen by
                        whoever runs it; the <A href={SERVER_ADMIN}>server admin</A> on the dashboard
                        lists them.
                      </p>
                    }
                  >
                    <p class="adm-note">Asking the server whether {me().gamertag} is an admin…</p>
                  </Show>
                  <p class="adm-note">
                    Signed in as someone else? Sign out above, then sign in or{" "}
                    <A href={IMPORT_ACCOUNT}>import</A> the admin account on the dashboard.
                  </p>
                </Panel>
              }
            >
              <Tabs tabs={[...ADMIN_TABS]} active={tab()} onPick={setTab} label="Halo admin sections" />
              <Show when={tab() === "motd"}>
                <MotdEditor />
              </Show>
              <Show when={tab() === "hopper" && `${live.server().base}|${me().xuid}`} keyed>
                <HopperBuilder />
              </Show>
              <Show when={tab() === "content"}>
                <ContentAdmin />
              </Show>
              <Show when={tab() === "training"}>
                <ClankerTraining />
              </Show>
            </Show>
          )}
        </Show>
      </div>
    </div>
  );
}
