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
import { live } from "./state/live";
import { startGameArt } from "./state/gameart";

// The dashboard (the front page) is in the main chunk; the stats pages,
// admin and sign-in are chunks of their own that come in once the browser
// is idle, so moving to them waits on nothing. The 3D view, with
// three.js, loads only when opened.
const parts = {
  game: () => import("./stats/Game"),
  games: () => import("./stats/Games"),
  home: () => import("./stats/Home"),
  players: () => import("./stats/Players"),
  player: () => import("./stats/Player"),
  campaign: () => import("./stats/CampaignRecord"),
  events: () => import("./stats/Events"),
  clans: () => import("./stats/Clans"),
  ranks: () => import("./stats/Ranks"),
  admin: () => import("./admin/Admin"),
  signin: () => import("./signin/SignIn"),
};
const Game = lazy(parts.game);
const Games = lazy(parts.games);
const Home = lazy(parts.home);
const Players = lazy(parts.players);
const ServiceRecordPage = lazy(() => parts.player().then((m) => ({ default: m.ServiceRecordPage })));
const HistoryPage = lazy(() => parts.player().then((m) => ({ default: m.HistoryPage })));
const CareerPage = lazy(() => parts.player().then((m) => ({ default: m.CareerPage })));
const CampaignRecordPage = lazy(parts.campaign);
const Events = lazy(parts.events);
const Ranks = lazy(parts.ranks);
const PlayerRanksPage = lazy(() => parts.ranks().then((m) => ({ default: m.PlayerRanksPage })));
const Clans = lazy(parts.clans);
const ClanPage = lazy(() => parts.clans().then((m) => ({ default: m.ClanPage })));
const Admin = lazy(parts.admin);
const SignIn = lazy(parts.signin);
const DashboardConcept = lazy(() => import("./dash/concepts/Concept"));
const Viewer3DPage = lazy(() => import("./viewer/Popout"));

const idle = (f: () => void) =>
  "requestIdleCallback" in globalThis ? requestIdleCallback(f, { timeout: 3000 }) : setTimeout(f, 1500);
addEventListener("load", () => idle(() => { for (const load of Object.values(parts)) void load(); }));

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
      <Route path="/dash/concepts/:concept" matchFilters={{ concept: ["orbital", "silverline", "switchboard"] }} component={DashboardConcept} />
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
        <Route path="/players/:tag/ranks" component={PlayerRanksPage} />
        <Route path={["/ranks", "/ranks/:playlist"]} component={Ranks} />
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
