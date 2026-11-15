import { render } from "solid-js/web";
import { Navigate, Route, Router, useParams } from "@solidjs/router";
import "./styles/theme.css";
import "./styles/app.css";
import "./styles/dash.css";
import "./styles/stats.css";
import Dashboard from "./dash/Dashboard";
import StatsLayout, { Title } from "./stats/Layout";
import Game from "./stats/Game";
import Games from "./stats/Games";
import Players from "./stats/Players";
import { CareerPage, HistoryPage, ServiceRecordPage } from "./stats/Player";
import Events from "./stats/Events";
import Admin from "./admin/Admin";
import { live } from "./state/live";
import { startGameArt } from "./state/gameart";

live.start();
startGameArt();

function NotFound() {
  return <div class="s-full"><Title class="s-title--bar">Page Not Found</Title><p class="s-pad">That page isn't here. Try the links above.</p></div>;
}

const OldGame = () => <Navigate href={`/stats/games/${useParams().id}`} />;
const OldPlayer = () => <Navigate href={`/stats/players/${useParams().tag}`} />;

render(
  () => (
    <Router>
      <Route path={["/", "/dash/:section", "/dash/:section/:tag"]} component={Dashboard} />
      <Route path="/stats" component={StatsLayout}>
        <Route path="/" component={Games} />
        <Route path="/games/:id" component={Game} />
        <Route path="/players" component={Players} />
        <Route path="/players/:tag" component={ServiceRecordPage} />
        <Route path="/players/:tag/history" component={HistoryPage} />
        <Route path="/players/:tag/career" component={CareerPage} />
        <Route path="/events" component={Events} />
        <Route path="/admin" component={Admin} />
        <Route path="*404" component={NotFound} />
      </Route>
      <Route path="/games/:id" component={OldGame} />
      <Route path="/players/:tag" component={OldPlayer} />
      <Route path="/players" component={() => <Navigate href="/dash/players" />} />
      <Route path="/events" component={() => <Navigate href="/stats/events" />} />
      <Route path="*404" component={() => <Navigate href="/" />} />
    </Router>
  ),
  document.getElementById("root")!,
);
