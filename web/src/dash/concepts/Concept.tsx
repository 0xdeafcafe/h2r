import { For, Match, Show, Switch, createEffect, createSignal, on, onCleanup } from "solid-js";
import { A, useNavigate, useParams, useSearchParams } from "@solidjs/router";
import Tunnel from "../Tunnel";
import { Cables, ChromeFrame, Icon, Mark, Orb } from "./Art";
import { Bulletin, Counters, Preview, ScreenContent } from "./Screens";
import { concepts, destinations, messages, online, players, screenFrom, sessions, type Concept, type Section } from "./model";
import { useControls } from "./controls";
import "./concepts.css";

export default function ConceptPage() {
  const params = useParams<{ concept: Concept }>();
  const [query, update] = useSearchParams();
  const navigate = useNavigate();
  const concept = () => params.concept;
  const screen = () => screenFrom(query.screen);
  const isHome = () => screen() === "home";
  const [tag, setTag] = createSignal("LowOrbit");
  const [selected, setSelected] = createSignal<Section>("friends");
  let root!: HTMLDivElement;
  const destination = () => destinations.find(d => d.id === screen());
  const title = () => isHome() ? ({ orbital: "Main menu", silverline: "Live menu", switchboard: "Relay overview" }[concept()]) : screen() === "account" ? "Live ID" : screen() === "server" ? "Server usage" : destination()?.label;
  const href = (id: Concept) => "/dash/concepts/"+id+(isHome() ? "" : "?screen="+screen());
  const open = (id: Section) => { setSelected(id); update({ screen: id }); };
  const back = () => {
    const previous = destination();
    if (previous) { setSelected(previous.id); update({ screen: undefined }); }
    else navigate("/");
  };
  const compare = (dir: number) => {
    const index = concepts.findIndex(c => c.id === concept());
    navigate(href(concepts[(index+dir+concepts.length)%concepts.length]!.id));
  };
  const originalTitle = document.title;
  onCleanup(() => { document.title = originalTitle; });
  useControls(() => root, back, compare);
  createEffect(on([concept, screen], () => {
    document.title = (title() ?? "Dashboard")+" · "+concepts.find(c => c.id === concept())?.name+" · h2 Live";
    queueMicrotask(() => {
      const initial = isHome() ? root.querySelector<HTMLElement>(`[data-menu="${selected()}"]`) : root.querySelector<HTMLElement>("[data-initial]");
      initial?.focus({ preventScroll: true });
    });
  }));
  function Menu(props: { layout: "fan" | "blades" | "rail" }) {
    return <nav class={`cc-menu cc-${props.layout}`} aria-label="Player services"><For each={destinations}>{(d,i) =>
      <button data-nav data-menu={d.id} classList={{ "cc-lit": (isHome() ? selected() : screen()) === d.id }}
        style={{ "--arc": ["0px","26px","36px","26px","0px"][i()] }}
        aria-current={screen() === d.id ? "page" : undefined}
        onFocus={() => setSelected(d.id)} onPointerMove={e => { if (e.pointerType === "mouse" && e.currentTarget !== document.activeElement) e.currentTarget.focus({ preventScroll: true }); }} onClick={() => open(d.id)}>
        <i class="cc-socket" aria-hidden="true"/><span class="cc-blade-face"><Icon name={d.id}/><strong>{d.label}</strong><span class="cc-chevron" aria-hidden="true">›</span></span>
      </button>
    }</For></nav>;
  }
  const material = () => <ScreenContent screen={screen()} tag={tag()} save={setTag}/>;
  return <div ref={root} class="cc" data-concept={concept()} data-screen={screen()}>
    <div class="cc-backdrop"><Tunnel aim={{ x: concept() === "silverline" ? .63 : .27, y: .51, dim: concept() === "switchboard" ? 1 : .3 }}/></div>
    <header class="cc-mast"><div class="cc-width"><div class="cc-brand"><Mark/><span>h2 <b>Live</b></span></div>
      <div class="cc-relay"><i class="cc-led"/>Northline relay</div>
      <div class="cc-profile"><span class="cc-avatar"><Icon name="account"/></span><span><small>LIVE ID</small><b>{tag()}</b></span></div>
    </div></header>
    <main class="cc-main cc-width">
      <Cables layout={concept()+"/"+screen()}/>
      <Show when={concept() !== "silverline"}>
        <div class="cc-title-plate"><i class="cc-twin"/><div><small>{concept() === "switchboard" ? "RELAY CONTROL" : "PLAYER SERVICES"}</small><h1>{title()}</h1></div><span class="cc-vents" aria-hidden="true"/></div>
      </Show>
      <Switch>
        <Match when={concept() === "orbital"}>
          <Show when={isHome()} fallback={<div class="cc-orb-section"><div class="cc-orb-module"><Orb/><span class="cc-id-stamp">NORTHLINE · NL–017</span></div>{material()}</div>}>
            <div class="cc-orbit-stage"><div class="cc-engine"><Orb/></div><Menu layout="fan"/>
              <aside class="cc-orbit-monitor"><Preview selected={selected()} tag={tag()} hint={destinations.find(d => d.id === selected())!.detail}/></aside>
            </div>
          </Show>
        </Match>
        <Match when={concept() === "silverline"}>
          <div class="cc-shell"><ChromeFrame/><div class="cc-shell-inner">
            <div class="cc-shell-title"><div><small>PLAYER SERVICES</small><h1>{title()}</h1></div><span>18 NOV 2026</span></div>
            <Show when={isHome()} fallback={material()}>
              <div class="cc-blade-home"><div><Menu layout="blades"/><Counters/></div><Bulletin/></div>
            </Show>
          </div></div>
        </Match>
        <Match when={concept() === "switchboard"}>
          <div class="cc-ops"><aside><div class="cc-rail-head"><span class="cc-mini-ball"/>Northline</div><Menu layout="rail"/>
            <div class="cc-rail-status"><i class="cc-led"/>{online.length} friends online</div>
            <Show when={isHome()}><ul class="cc-rail-roster"><For each={online}>{f => <li><span>{f.tag}</span><small>{f.state}</small></li>}</For></ul></Show>
          </aside>
            <div class="cc-console">{material()}</div>
          </div>
        </Match>
      </Switch>
    </main>
    <footer class="cc-footer cc-width"><div class="cc-prompts"><button data-nav onClick={back}><i>B</i> Back</button><span><i>A</i> Select</span></div>
      <div class="cc-compare"><span>Concept</span><nav aria-label="Compare concepts"><For each={concepts}>{c =>
        <A data-nav href={href(c.id)} aria-current={concept() === c.id ? "page" : undefined}>{c.name}</A>
      }</For></nav></div>
    </footer>
    <div class="cc-status cc-width"><span><i class="cc-led"/> Relay connected</span><span>{players} players <b>·</b> {sessions.length} sessions <b>·</b> {messages.length} messages</span><span>Preview data</span></div>
  </div>;
}
