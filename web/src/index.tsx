import { lazy } from "solid-js";
import { render } from "solid-js/web";
import { Navigate, Route, Router, useParams } from "@solidjs/router";
import "./styles/theme.css";
import "./styles/app.css";
import "./styles/dash.css";
import "./styles/stats.css";
import Dashboard from "./dash/Dashboard";
import Veil from "./dash/Veil";
import StatsLayout, { Title } from "./stats/Layout";
import Game from "./stats/Game";
import Games from "./stats/Games";
import Home from "./stats/Home";
import Players from "./stats/Players";
import { CareerPage, HistoryPage, ServiceRecordPage } from "./stats/Player";
import CampaignRecordPage from "./stats/CampaignRecord";
import Events from "./stats/Events";
import Clans, { ClanPage } from "./stats/Clans";
import Admin from "./admin/Admin";
import SignIn from "./signin/SignIn";
import { live } from "./state/live";
import { startGameArt } from "./state/gameart";

// The 3D view in a window of its own; three.js stays out of the main bundle.
const Viewer3DPage = lazy(() => import("./viewer/Popout"));

live.start();
startGameArt();

function NotFound() {
  return <div class="s-full"><Title class="s-title--bar">Page Not Found</Title><p class="s-pad">That page isn't here. Try the links above.</p></div>;
}

const OldGame = () => <Navigate href={`/stats/games/${useParams().id}`} />;
// Clans were called teams here first (the protocol still says team).
const OldTeam = () => <Navigate href={`/stats/clans/${useParams().id}`} />;
const OldPlayer = () => <Navigate href={`/stats/players/${useParams().tag}`} />;

render(
  () => (
    <Router root={Veil}>
      <Route path="/dash/teams" component={() => <Navigate href="/dash/clans" />} />
      <Route path="/dash/teams/:tag" component={() => <Navigate href={`/dash/clans/${useParams().tag}`} />} />
      <Route path={["/", "/dash/:section", "/dash/:section/:tag"]} component={Dashboard} />
      <Route path="/stats" component={StatsLayout}>
        <Route path="/" component={Home} />
        <Route path="/games" component={Games} />
        <Route path="/games/:id" component={Game} />
        <Route path="/players" component={Players} />
        <Route path="/players/:tag" component={ServiceRecordPage} />
        <Route path="/players/:tag/history" component={HistoryPage} />
        <Route path="/players/:tag/career" component={CareerPage} />
        <Route path="/players/:tag/campaign" component={CampaignRecordPage} />
        <Route path="/clans" component={Clans} />
        <Route path="/clans/:id" component={ClanPage} />
        <Route path="/teams" component={() => <Navigate href="/stats/clans" />} />
        <Route path="/teams/:id" component={OldTeam} />
        <Route path="/events" component={Events} />
        <Route path="/admin" component={Admin} />
        <Route path="*404" component={NotFound} />
      </Route>
      <Route path="/stats/games/:id/3d" component={Viewer3DPage} />
      <Route path="/signin" component={SignIn} />
      <Route path="/games/:id" component={OldGame} />
      <Route path="/players/:tag" component={OldPlayer} />
      <Route path="/players" component={() => <Navigate href="/dash/players" />} />
      <Route path="/events" component={() => <Navigate href="/stats/events" />} />
      <Route path="*404" component={() => <Navigate href="/" />} />
    </Router>
  ),
  document.getElementById("root")!,
);
