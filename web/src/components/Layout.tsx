import { Show, type JSX } from "solid-js";
import { A } from "@solidjs/router";
import { live } from "../state/live";
import ServerPicker from "./ServerPicker";
import { Mascot } from "./bits";

const STATUS_TEXT: Record<string, string> = {
  idle: "waking up",
  connecting: "saying hello",
  "catching-up": "catching up",
  live: "live",
  reconnecting: "reconnecting",
  stopped: "paused",
};

export default function Layout(props: { children?: JSX.Element }) {
  return (
    <div class="app">
      <header class="topbar">
        <A href="/" class="brand" end>
          <Mascot size={34} />
          <span class="brand__name">h2 <em>live</em></span>
        </A>
        <nav class="nav">
          <A href="/" end>Home</A>
          <A href="/players">Players</A>
          <A href="/events">Event log</A>
        </nav>
        <div class="topbar__right">
          <span class={`conn conn--${live.status()}`} title={`stream position ${live.state.position}`}>
            {STATUS_TEXT[live.status()] ?? live.status()}
            <Show when={live.status() === "reconnecting" && live.retryIn()}>{(ms) => <> in {Math.ceil(ms() / 1000)}s</>}</Show>
          </span>
          <ServerPicker />
        </div>
      </header>
      <main class="main">{props.children}</main>
      <footer class="footer">
        h2 live · an unofficial fan service for h2recomp · position {live.state.position}
        <Show when={live.verifySummary.bad > 0}> · <span class="warn">{live.verifySummary.bad} bad signatures</span></Show>
      </footer>
    </div>
  );
}
