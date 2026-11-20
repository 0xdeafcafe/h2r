import { A } from "@solidjs/router";
import { For, Show } from "solid-js";
import { LIVE_ADMIN_USAGE } from "../admin/server";
import { adminState } from "../admin/state";
import { LIVE_ADMIN } from "../routes";
import { identity } from "../state/identity";
import { DECK_ENTRY } from "./back";
import { DeckPage, Panel } from "./Deck";

const LINKS = [
  { label: "usage", href: LIVE_ADMIN_USAGE },
  { label: "hosts", href: `${LIVE_ADMIN}/hosts` },
  { label: "event log", href: `${LIVE_ADMIN}/events` },
  { label: "style", href: `${LIVE_ADMIN}/style` },
];

export default function AdminPage() {
  const admin = adminState.isMeAdmin;
  return (
    <DeckPage title="live admin" trail={[["dashboard", "/"], ["server", "/dash/server"], ["live admin"]]} back="/dash/settings">
      <Show when={admin() === true} fallback={
        <Panel title="Admin access" span={6}>
          <p class="dk-note">
            <Show when={identity.me()} fallback={<><A href="/dash/signin">Sign in</A> with a server admin account.</>}>
              {admin() === null ? "Checking admin access…" : "These tools are available to server admins."}
            </Show>
          </p>
        </Panel>
      }>
        <Panel title="Server tools" span={6}>
          <nav aria-label="Server tools">
            <ul>
              <For each={LINKS}>{({ label, href }) => <li><A href={href} state={DECK_ENTRY}>{label}</A></li>}</For>
            </ul>
          </nav>
        </Panel>
      </Show>
    </DeckPage>
  );
}
