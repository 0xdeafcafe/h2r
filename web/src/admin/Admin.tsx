// The Halo admin area, a stats-site page at /stats/admin shown only to
// admins: the message-of-the-day editor, the playlist (hopper) builder and
// custom content moderation (map packages, blobs, reports, uploads). Only
// the game's own things live here; running the service (invites, accounts
// and admins, importing an account, the server's status) is the dashboard's
// "server" screens, linked from the title bar (./tabs.ts).

import { Show, createSignal } from "solid-js";
import { A } from "@solidjs/router";
import { Crumbs, Panel, Tabs, Title } from "../stats/Layout";
import { identity } from "../state/identity";
import { adminState } from "./state";
import MotdEditor from "./MotdEditor";
import HopperBuilder from "./HopperBuilder";
import ContentAdmin from "./ContentAdmin";
import { ADMIN_TABS, IMPORT_ACCOUNT, SERVER_ADMIN, adminTab } from "./tabs";
import "../styles/admin.css";

export default function Admin() {
  // ?tab=hopper opens the playlists (the dashboard links here).
  const asked = typeof location !== "undefined" ? new URLSearchParams(location.search).get("tab") : null;
  const [tab, setTab] = createSignal<string>(adminTab(asked));
  const admin = adminState.isMeAdmin;
  return (
    <div class="s-full">
      <Crumbs trail={[["Home", "/stats"], ["Halo Admin"]]} />
      <Title class="s-title--bar" right={<A class="adm-server-link" href={SERVER_ADMIN}>Dashboard server admin ›</A>}>
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
              <Show when={tab() === "hopper"}>
                <HopperBuilder />
              </Show>
              <Show when={tab() === "content"}>
                <ContentAdmin />
              </Show>
            </Show>
          )}
        </Show>
      </div>
    </div>
  );
}
