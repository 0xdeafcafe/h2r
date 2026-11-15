import { render } from "solid-js/web";
import { Route, Router } from "@solidjs/router";
import "./styles/theme.css";
import "./styles/app.css";
import Layout from "./components/Layout";
import Home from "./pages/Home";
import Game from "./pages/Game";
import { Player, Players } from "./pages/Players";
import Events from "./pages/Events";
import { Empty } from "./components/bits";
import { live } from "./state/live";

live.start();

function NotFound() {
  return <div class="page"><Empty title="This page wandered off" mood="wow">Try the links up top.</Empty></div>;
}

render(
  () => (
    <Router root={Layout}>
      <Route path="/" component={Home} />
      <Route path="/games/:id" component={Game} />
      <Route path="/players" component={Players} />
      <Route path="/players/:tag" component={Player} />
      <Route path="/events" component={Events} />
      <Route path="*404" component={NotFound} />
    </Router>
  ),
  document.getElementById("root")!,
);
