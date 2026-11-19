import { watchHeaderEmblems } from "./lib/emblem";
import { lazy } from "solid-js";
import { render } from "solid-js/web";
import { Navigate, Route, Router, useLocation, useParams } from "@solidjs/router";
import "./styles/theme.css";
import "./styles/app.css";
import "./styles/dash.css";
import "./styles/stats.css";
import Dashboard from "./dash/Dashboard";
import Veil from "./dash/Veil";
import StatsLayout, { Title } from "./stats/Layout";
import { live } from "./state/live";
import { startGameArt } from "./state/gameart";
import { ThemeColor } from "./themeColor";
import { GAMES_ROOT, OLD_STATS_ROOT, STATS_ROOT, movedStatsPath, statsPath } from "./routes";

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
  theater: () => import("./theater/Theater"),
  events: () => import("./stats/Events"),
  clans: () => import("./stats/Clans"),
  ranks: () => import("./stats/Ranks"),
  mods: () => import("./stats/Mods"),
  admin: () => import("./admin/Admin"),
  signin: () => import("./signin/SignIn"),
  deckStyle: () => import("./deck/StylePage"),
  usage: () => import("./deck/UsagePage"),
  eventLog: () => import("./dash/EventLog"),
};
const Game = lazy(parts.game);
const Games = lazy(parts.games);
const Home = lazy(parts.home);
const Players = lazy(parts.players);
const ServiceRecordPage = lazy(() => parts.player().then((m) => ({ default: m.ServiceRecordPage })));
const HistoryPage = lazy(() => parts.player().then((m) => ({ default: m.HistoryPage })));
const CareerPage = lazy(() => parts.player().then((m) => ({ default: m.CareerPage })));
const CampaignRecordPage = lazy(parts.campaign);
const FilmsPage = lazy(() => parts.player().then((m) => ({ default: m.FilmsPage })));
const Theater = lazy(parts.theater);
const Events = lazy(parts.events);
const Ranks = lazy(parts.ranks);
const PlayerRanksPage = lazy(() => parts.ranks().then((m) => ({ default: m.PlayerRanksPage })));
const Clans = lazy(parts.clans);
const ClanPage = lazy(() => parts.clans().then((m) => ({ default: m.ClanPage })));
const Mods = lazy(parts.mods);
const ModPage = lazy(() => parts.mods().then((m) => ({ default: m.ModPage })));
const Admin = lazy(parts.admin);
const SignIn = lazy(parts.signin);
const DeckStyle = lazy(parts.deckStyle);
const Usage = lazy(parts.usage);
const EventLog = lazy(parts.eventLog);

const idle = (f: () => void) =>
  "requestIdleCallback" in globalThis ? requestIdleCallback(f, { timeout: 3000 }) : setTimeout(f, 1500);
addEventListener("load", () => idle(() => { for (const load of Object.values(parts)) void load(); }));

live.start();
startGameArt();

function NotFound() {
  return <div class="s-full"><Title class="s-title--bar">Page Not Found</Title><p class="s-pad">That page isn't here. Try the links above.</p></div>;
}

const OldGame = () => <Navigate href={statsPath(`/games/${useParams().id}`)} />;
// The 3D view had a pop-out window of its own here; it is in the game page
// now: the same game with the 3D view showing (its `cam`, `cut`, `see` and
// `ao` settings kept), which brings the viewer into view.
const Old3dView = () => {
  const at = useLocation();
  const q = new URLSearchParams(at.search);
  q.set("view", "3d");
  return <Navigate href={`${statsPath(`/games/${useParams().id}`)}?${q}`} />;
};
// Clans were called teams here first (the protocol still says team).
const OldTeam = () => <Navigate href={statsPath(`/clans/${useParams().id}`)} />;
const OldPlayer = () => <Navigate href={statsPath(`/players/${useParams().tag}`)} />;
// The stats site was at /stats first: the same page where it is now, in
// place of the old entry (the servers answer these with a 301 too).
const OldStats = () => {
  const at = useLocation();
  return <Navigate href={`${movedStatsPath(at.pathname) ?? STATS_ROOT}${at.search}${at.hash}`} />;
};

watchHeaderEmblems(document);
render(
  () => (
    <Router root={(p) => <><ThemeColor /><Veil {...p} /></>}>
      <Route path="/dash/clans" component={() => <Navigate href="/dash/teams" />} />
      <Route path="/dash/clans/:tag" component={() => <Navigate href={`/dash/teams/${useParams().tag}`} />} />
      <Route path="/dash/admin/style" component={DeckStyle} />
      <Route path="/dash/admin/usage" component={Usage} />
      <Route path="/dash/admin/hosts" component={lazy(() => import("./deck/HostsPage"))} />
      <Route path="/dash/admin/events" component={EventLog} />
      <Route path="/dash/admin" component={() => <Navigate href="/dash/admin/usage" />} />
      <Route path={["/", "/dash/:section", "/dash/:section/:tag"]} component={Dashboard} />
      <Route path={STATS_ROOT} component={StatsLayout}>
        <Route path="/" component={Home} />
        <Route path="/games" component={Games} />
        <Route path="/games/:id" component={Game} />
        <Route path="/players" component={Players} />
        <Route path="/players/:tag" component={ServiceRecordPage} />
        <Route path="/players/:tag/history" component={HistoryPage} />
        <Route path="/players/:tag/career" component={CareerPage} />
        <Route path="/players/:tag/campaign" component={CampaignRecordPage} />
        <Route path="/players/:tag/films" component={FilmsPage} />
        <Route path="/players/:tag/ranks" component={PlayerRanksPage} />
        <Route path={["/ranks", "/ranks/:playlist"]} component={Ranks} />
        <Route path="/clans" component={Clans} />
        <Route path="/clans/:id" component={ClanPage} />
        <Route path="/teams" component={() => <Navigate href={statsPath("/clans")} />} />
        <Route path="/teams/:id" component={OldTeam} />
        <Route path="/mods" component={Mods} />
        <Route path="/mods/:id" component={ModPage} />
        <Route path="/events" component={Events} />
        <Route path="/admin" component={Admin} />
        <Route path="*404" component={NotFound} />
      </Route>
      <Route path="/theater" component={StatsLayout}>
        <Route path={["/:game", "/:game/clip/:clip"]} component={Theater} />
      </Route>
      <Route path={statsPath("/games/:id/3d")} component={Old3dView} />
      <Route path={[OLD_STATS_ROOT, `${OLD_STATS_ROOT}/*rest`]} component={OldStats} />
      <Route path={GAMES_ROOT} component={() => <Navigate href={STATS_ROOT} />} />
      <Route path="/signin" component={SignIn} />
      <Route path="/games/:id" component={OldGame} />
      <Route path="/players/:tag" component={OldPlayer} />
      <Route path="/players" component={() => <Navigate href="/dash/players" />} />
      <Route path="/events" component={() => <Navigate href={statsPath("/events")} />} />
      <Route path="*404" component={() => <Navigate href="/" />} />
    </Router>
  ),
  document.getElementById("root")!,
);
