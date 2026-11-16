// The Admin area, a stats-site page at /stats/admin shown only to admins:
// the message-of-the-day editor, the playlist (hopper) builder, custom
// content moderation (map packages, blobs, reports, uploads), plus the
// one-off import that signs the browser in as an existing CLI account
// (docs/live/live-admin.md).

import { Show, createSignal } from "solid-js";
import { A } from "@solidjs/router";
import { Crumbs, Panel, Tabs, Title } from "../stats/Layout";
import { identity } from "../state/identity";
import { adminState } from "./state";
import MotdEditor from "./MotdEditor";
import HopperBuilder from "./HopperBuilder";
import ImportAccount from "./ImportAccount";
import ContentAdmin from "./ContentAdmin";
import "../styles/admin.css";

const TABS = [
  { id: "motd", label: "Message of the Day" },
  { id: "hopper", label: "Playlists" },
  { id: "content", label: "Custom content" },
  { id: "import", label: "Import account" },
];

export default function Admin() {
  // ?tab=hopper opens the playlists (the dashboard's server screen links here).
  const asked = typeof location !== "undefined" ? new URLSearchParams(location.search).get("tab") : null;
  const [tab, setTab] = createSignal(TABS.some((t) => t.id === asked) ? asked! : "motd");
  const admin = adminState.isMeAdmin;
  return (
    <div class="s-full">
      <Crumbs trail={[["Home", "/stats"], ["Admin"]]} />
      <Title class="s-title--bar" right={<A href="/stats">Back to the stats</A>}>
        Admin
      </Title>
      <div class="adm">
        <Show
          when={identity.me()}
          fallback={
            <Panel title="Sign in first">
              <p>
                The admin area needs a signed-in browser account. <A href="/dash/signup">Sign up</A>{" "}
                makes a new one; to administer as an account the CLI holds (whose key this browser
                never saw), export it and import it here.
              </p>
              <ImportAccount />
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
                      <p>
                        <b>{me().gamertag}</b> is not an admin on this server. The operator grants
                        it with <code>h2 live admin grant {me().gamertag}</code>, or configures it
                        at startup (<code>--admin</code>, <code>H2_LIVE_ADMINS</code>).
                      </p>
                    }
                  >
                    <p>Asking the server whether {me().gamertag} is an admin…</p>
                  </Show>
                  <details class="adm-hint">
                    <summary>Signed in as someone else?</summary>
                    <p>
                      Sign out in the header, then import the admin account below.{" "}
                      <button class="s-btn" onClick={() => identity.signOut()}>
                        Sign out
                      </button>
                    </p>
                  </details>
                </Panel>
              }
            >
              <Tabs tabs={TABS} active={tab()} onPick={setTab} label="Admin sections" />
              <Show when={tab() === "motd"}>
                <MotdEditor />
              </Show>
              <Show when={tab() === "hopper"}>
                <HopperBuilder />
              </Show>
              <Show when={tab() === "content"}>
                <ContentAdmin />
              </Show>
              <Show when={tab() === "import"}>
                <ImportAccount />
              </Show>
            </Show>
          )}
        </Show>
      </div>
    </div>
  );
}
