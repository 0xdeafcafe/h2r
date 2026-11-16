// The 3D view itself: a ViewerScene in a box, with its camera controls.
// Loaded lazily (it brings three.js), by ViewerWindow and the pop-out page.

import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import type { MapFrame } from "../components/MapCanvas";
import type { RosterEntry } from "../map/render";
import { onAnimationFrame } from "../map/scheduler";
import { gameArt } from "../state/gameart";
import { clampCut, nextSee, stepCut, type SeeMode } from "./cutaway";
import type { LevelInfo } from "./level3d";
import { modelsAnimate } from "./models";
import { zoneInPlay, type ZonePick } from "./zones";
import { savePref, texturesPref } from "./textures3d";
import { KIND_LABELS } from "./scenario3d";
import { ViewerScene, hasWebGl, type ViewMode } from "./scene";
import "./viewer.css";

export interface Viewer3DProps {
  frame: (now: number) => MapFrame;
  roster: () => Map<number, RosterEntry>;
  /** The map's glb, or null when the server has none (a grid floor then). */
  glb: () => string | null;
  highlight?: () => number | null;
  /** Wall time of the last seek: players jump instead of gliding. */
  soughtAt?: () => number;
  /** Camera to open with, and a player to follow. */
  mode?: ViewMode;
  follow?: number | null;
  ao?: boolean;
  /** Open with the height cut on, at this height (game z); and how the
   *  see-through starts (`auto` unless false or another mode is given). */
  cut?: number | null;
  seeThrough?: boolean | SeeMode;
}

const reducedMotion = () => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;

export default function Viewer3D(props: Viewer3DProps) {
  let host!: HTMLDivElement;
  let scene: ViewerScene | null = null;
  const [failed, setFailed] = createSignal<string | null>(null);
  const [mode, setMode] = createSignal<ViewMode>(props.mode ?? "orbit");
  const [follow, setFollow] = createSignal<number | null>(props.follow ?? null);
  const [ao, setAo] = createSignal(props.ao ?? true);
  // Seeing into the level: the height cut (off to begin with) and the
  // see-through to the player in focus (on).
  const [cut, setCut] = createSignal(props.cut != null);
  const [cutAt, setCutAt] = createSignal<number | null>(props.cut ?? null);
  const [range, setRange] = createSignal<[number, number]>([0, 1]);
  const [see, setSee] = createSignal<SeeMode>(props.seeThrough === false ? "off" : typeof props.seeThrough === "string" ? props.seeThrough : "auto");
  const cutValue = () => clampCut(cutAt() ?? (range()[0] + range()[1]) / 2, range());
  const applyCut = () => scene?.setCut(cut(), cutValue());
  const toggleCut = () => { if (scene) setRange(scene.cutRange()); setCut(!cut()); applyCut(); };
  const moveCut = (h: number) => { setCutAt(clampCut(h, range())); if (!cut()) setCut(true); applyCut(); };
  const pickSee = (m: SeeMode) => { setSee(m); scene?.setSeeMode(m); };
  const toggleSee = () => pickSee(nextSee(see()));
  // The flycam: f toggles it; while it is on, WASD, the arrows, Q, E, Space,
  // Ctrl and Shift are its (none of them is another key here).
  const [flying, setFlying] = createSignal(false);
  const [flyHint, setFlyHint] = createSignal("");
  let hintTimer: ReturnType<typeof setTimeout> | undefined;
  const hint = (text: string, ms = 1600) => { setFlyHint(text); clearTimeout(hintTimer); hintTimer = setTimeout(() => setFlyHint(""), ms); };
  onCleanup(() => clearTimeout(hintTimer));
  let root!: HTMLDivElement;
  const toggleFly = () => {
    if (!scene) return;
    scene.setFly(!scene.flying);
    if (scene.flying) hint("Flycam: WASD or arrows move, Q E or Ctrl Space down and up, drag to look, wheel for speed, Shift for faster, F to leave", 5000);
    else setFlyHint("");
    root.focus();
  };
  const onKeyUp = (e: KeyboardEvent) => { if (scene?.flyKey(e, false)) { e.preventDefault(); e.stopPropagation(); } };
  const onKey = (e: KeyboardEvent) => {
    const typing = (e.target as HTMLElement).closest("input, select, textarea");
    // Stopped here: the pop-out page takes Space for play and pause.
    if (!typing && scene?.flyKey(e, true)) { e.preventDefault(); e.stopPropagation(); return; }
    if (e.ctrlKey || e.metaKey || e.altKey || typing) return;
    if (e.key === "f") toggleFly();
    else if (e.key === "c") toggleCut();
    else if (e.key === "[" || e.key === "]") { if (scene) setRange(scene.cutRange()); moveCut(stepCut(cutValue(), e.key === "]" ? 1 : -1, range())); }
    else if (e.key === "x") toggleSee();
    else return;
    e.preventDefault();
  };
  // AI-controlled bipeds (campaign) go unnamed unless asked; the button only shows when there are some.
  const [hasActors, setHasActors] = createSignal(false);
  const [actorNames, setActorNames] = createSignal(false);
  // A campaign level comes in zones, and may have far scenery around it:
  // the zone in play is shown (or one picked, or all), the scenery isn't,
  // unless asked; decals, effects, the sky and its fog are, unless not.
  const [level, setLevel] = createSignal<LevelInfo | null>(null);
  const [zonePick, setZonePick] = createSignal<ZonePick>("auto");
  const [show, setShow] = createSignal({ cinematic: false, backdrop: false, decals: true, effects: true, sky: true, fog: true });
  const flip = (k: keyof ReturnType<typeof show>) => {
    const on = !show()[k];
    setShow({ ...show(), [k]: on });
    if (!scene) return;
    if (k === "cinematic") scene.setCinematic(on);
    else if (k === "backdrop") scene.setBackdrop(on);
    else if (k === "decals") scene.setDecals(on);
    else if (k === "effects") scene.setEffects(on);
    else if (k === "sky") scene.setSky(on);
    else scene.setFog(on);
    setLevel(scene.levelInfo());
  };
  const applyLevel = (s: ViewerScene) => {
    const o = show();
    s.setCinematic(o.cinematic); s.setBackdrop(o.backdrop); s.setDecals(o.decals); s.setEffects(o.effects); s.setSky(o.sky); s.setFog(o.fog);
    s.setZonePick(zonePick());
    setLevel(s.levelInfo());
  };
  // Surface pictures (remembered), and the scenario's placed objects with
  // each kind on or off.
  const [textures, setTextures] = createSignal(texturesPref());
  const [hasTextures, setHasTextures] = createSignal(false);
  const flipTextures = () => { const on = !textures(); setTextures(on); savePref(on); scene?.setTextures(on); };
  const [objectKinds, setObjectKinds] = createSignal<string[]>([]);
  const [objects, setObjects] = createSignal(true);
  const [hiddenKinds, setHiddenKinds] = createSignal<ReadonlySet<string>>(new Set());
  const flipKind = (k: string) => {
    const hidden = new Set(hiddenKinds());
    const on = hidden.has(k);
    if (on) hidden.delete(k); else hidden.add(k);
    setHiddenKinds(hidden);
    scene?.setObjectKind(k, on);
  };
  const applyObjects = (s: ViewerScene) => {
    setHasTextures(s.hasTextures());
    s.setTextures(textures());
    setObjectKinds(s.objectKinds());
    s.setObjects(objects());
    for (const k of s.objectKinds()) s.setObjectKind(k, !hiddenKinds().has(k));
  };
  // Teleporters, their links and lifts, when the level's glb lists any.
  const [hasFeatures, setHasFeatures] = createSignal(false);
  const [features, setFeatures] = createSignal(true);
  const [mapState, setMapState] = createSignal<"loading" | "map" | "grid">("grid");
  // The server's art manifest takes a moment: only say "no geometry" once it has had one.
  const [late, setLate] = createSignal(false);
  const lateTimer = setTimeout(() => setLate(true), 2500);
  onCleanup(() => clearTimeout(lateTimer));
  // Models from before version 2 have no clips: say why nobody moves.
  const staleModels = createMemo(() => {
    const models = gameArt()?.manifest.models;
    return !!models && Object.keys(models.bipeds).length > 0 && !modelsAnimate(models);
  });
  const people = createMemo(() => [...props.roster()].map(([index, r]) => ({ index, ...r })).sort((a, b) => a.name.localeCompare(b.name)));

  const pick = (m: ViewMode, index: number | null = follow()) => {
    if (m === "follow" && index === null) index = people()[0]?.index ?? null;
    if (m === "follow" && index === null) return;
    setMode(m);
    setFollow(m === "follow" ? index : follow());
    scene?.setMode(m, index);
  };

  onMount(() => {
    if (!hasWebGl()) { setFailed("This browser can't draw 3D here (WebGL is off or unavailable). The 2D map still shows the game."); return; }
    try {
      scene = new ViewerScene(host, { reducedMotion: reducedMotion(), onPick: (i) => pick("follow", i) });
    } catch {
      setFailed("The 3D view couldn't start on this device. The 2D map still shows the game.");
      return;
    }
    const s = scene;
    s.onAoDropped = () => setAo(false);
    s.onFlyChange = setFlying;
    s.onFlySpeed = (label) => hint(label);
    if (ao()) setAo(s.setAo(true));
    s.setSeeMode(see());
    s.setTextures(textures());
    setRange(s.cutRange());
    applyCut();
    const resize = () => { const r = host.getBoundingClientRect(); s.resize(Math.round(r.width), Math.round(r.height)); };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(host);
    let visible = true;
    const io = new IntersectionObserver((es) => { visible = es.some((e) => e.isIntersecting); });
    io.observe(host);
    let seen = 0, started = false;
    const stop = onAnimationFrame((now) => {
      if (!visible || document.hidden) return;
      const sought = props.soughtAt?.() ?? 0;
      const frame = props.frame(now);
      s.update(now, frame, props.roster(), props.highlight?.() ?? null, sought !== seen);
      seen = sought;
      if (!!frame.actors?.length !== hasActors()) setHasActors(!!frame.actors?.length);
      if ((frame.bsp ?? null) !== (level()?.active ?? null)) setLevel(s.levelInfo());
      // The opening camera, once there are players to follow.
      if (!started && frame.players.length) { started = true; if (mode() !== "orbit") pick(mode(), follow()); }
    });
    onCleanup(() => { stop(); ro.disconnect(); io.disconnect(); s.dispose(); scene = null; });
  });

  // Player models, when the server's art has them: each glb is fetched the
  // first time a player needs it. Without them players are capsules.
  createEffect(() => {
    const art = gameArt();
    scene?.setModels(art?.root ?? null, art?.manifest.models);
  });

  // The map: load it when its address is known; a grid floor otherwise.
  createEffect(on(() => props.glb(), (url) => {
    if (!scene) return;
    if (!url) { setMapState("grid"); return; }
    setMapState("loading");
    void scene.loadMap(url).then((ok) => {
      if (props.glb() !== url) return;
      setMapState(ok ? "map" : "grid");
      // The map's own heights are the cut's range now.
      if (scene) { setRange(scene.cutRange()); applyCut(); setZonePick("auto"); applyLevel(scene); applyObjects(scene); setHasFeatures(scene.hasFeatures()); scene.setFeatures(features()); }
    });
  }));

  return (
    <div class="v3" tabindex="0" ref={root} onKeyDown={onKey} onKeyUp={onKeyUp} onFocusOut={(e) => { if (!root.contains(e.relatedTarget as Node | null)) scene?.flyRelease(); }}>
      <Show when={!failed()} fallback={<p class="v3-fail">{failed()}</p>}>
        <div ref={host} class="v3-host" />
        <Show when={flyHint()}><div class="v3-fly-hint" role="status">{flyHint()}</div></Show>
        <div class="v3-tools" role="group" aria-label="Camera">
          <button classList={{ on: mode() === "orbit" }} aria-pressed={mode() === "orbit"} onClick={() => pick("orbit")} title="Drag to orbit, scroll to zoom, right-drag to pan">Orbit</button>
          <button classList={{ on: mode() === "top" }} aria-pressed={mode() === "top"} onClick={() => pick("top")} title="Straight down, north up">Top-down</button>
          <button classList={{ on: flying() }} aria-pressed={flying()} onClick={toggleFly} title="Flycam: fly freely. WASD or arrows move, Q and E (or Ctrl and Space) go down and up, drag to look, wheel changes speed, Shift is faster. A gamepad works too. Key: f">Fly</button>
          <select
            classList={{ on: mode() === "follow" }}
            aria-label="Follow a player"
            value={mode() === "follow" && follow() !== null ? String(follow()) : ""}
            onChange={(e) => { const v = e.currentTarget.value; if (v === "") pick("orbit"); else pick("follow", +v); }}
          >
            <option value="">Follow…</option>
            <For each={people()}>{(p) => <option value={p.index}>{p.name}</option>}</For>
          </select>
          <button onClick={() => scene?.reset()} title="Back to the whole map">Reset</button>
          <button classList={{ on: ao() }} aria-pressed={ao()} onClick={() => setAo(scene?.setAo(!ao()) ?? false)} title="Ambient occlusion: soft shading in corners (costs frame rate)">AO</button>
          <select
            classList={{ on: see() !== "off" }}
            aria-label="See-through"
            value={see()}
            onChange={(e) => pickSee(e.currentTarget.value as SeeMode)}
            title="See-through: walls, roofs and floors between the camera and players are cut away. In focus: the player you follow or point at in the scoreboard, else everyone. Key: x"
          >
            <option value="off">See-through off</option>
            <option value="auto">See-through: in focus</option>
            <option value="always">See-through: everyone</option>
          </select>
          <button classList={{ on: cut() }} aria-pressed={cut()} onClick={toggleCut} title="Height cut: hide the level above a height, to look into rooms from above. Following a player, it stays just over their head. Keys: c, [ and ]">Cut</button>
          <Show when={hasActors()}>
            <button classList={{ on: actorNames() }} aria-pressed={actorNames()} onClick={() => { setActorNames(!actorNames()); scene?.setActorNames(actorNames()); }} title="Name the AI by what they are">AI names</button>
          </Show>
          <Show when={(level()?.zoneCount ?? 0) > 1}>
            <select
              classList={{ on: zonePick() !== "auto" }}
              aria-label="Zone"
              value={String(zonePick())}
              onChange={(e) => { const v = e.currentTarget.value; const p: ZonePick = v === "auto" || v === "all" ? v : +v; setZonePick(p); scene?.setZonePick(p); }}
              title="This level is in zones and the game loads one at a time: follow the one in play, or pick one, or show them all"
            >
              <option value="auto">{zoneInPlay(level())}</option>
              <option value="all">All zones</option>
              <For each={level()?.zones ?? []}>{(z) => <option value={z.bsp}>{z.label}{z.cinematic ? " (cutscene)" : ""}</option>}</For>
            </select>
          </Show>
          <Show when={level()}>
            <details class="v3-menu">
              <summary title="What of the level to draw">Show</summary>
              <div class="v3-menu-list">
                <Show when={level()!.cinematicCount > 0}>
                  <label title="Zones only the game's cutscenes use: list them, and show them with all zones"><input type="checkbox" checked={show().cinematic} onChange={() => flip("cinematic")} /> Cutscene zones</label>
                </Show>
                <Show when={level()!.backdrop}>
                  <label title="The far scenery around the playable space"><input type="checkbox" checked={show().backdrop} onChange={() => flip("backdrop")} /> Backdrop</label>
                </Show>
                <Show when={level()!.decals}>
                  <label title="Marks, stains, signs and painted letters on the level's surfaces"><input type="checkbox" checked={show().decals} onChange={() => flip("decals")} /> Decals</label>
                </Show>
                <Show when={level()!.effects}>
                  <label title="Glows, light shafts, fire, waterfalls and other effects drawn into the air"><input type="checkbox" checked={show().effects} onChange={() => flip("effects")} /> Effects</label>
                </Show>
                <Show when={level()!.sky}>
                  <label title="The level's sky, drawn around the camera"><input type="checkbox" checked={show().sky} onChange={() => flip("sky")} /> Sky</label>
                </Show>
                <label title="Fog: the level's own, measured from what the camera looks at"><input type="checkbox" checked={show().fog} onChange={() => flip("fog")} /> Fog</label>
                <Show when={objectKinds().length > 0}>
                  <label title="The objects the level places: scenery, crates, weapons and their spawns, vehicles, controls"><input type="checkbox" checked={objects()} onChange={() => { setObjects(!objects()); scene?.setObjects(objects()); }} /> Objects</label>
                  <For each={objectKinds()}>{(k) => (
                    <label class="v3-menu-sub" title={`Placed objects of this kind (${k})`}>
                      <input type="checkbox" checked={!hiddenKinds().has(k)} disabled={!objects()} onChange={() => flipKind(k)} /> {KIND_LABELS[k] ?? k}
                    </label>
                  )}</For>
                </Show>
              </div>
            </details>
          </Show>
          <Show when={hasTextures()}>
            <button classList={{ on: textures() }} aria-pressed={textures()} onClick={flipTextures} title="The level's surface pictures, or plain colours by kind of surface (floor, wall, metal, ...)">Textures</button>
          </Show>
          <Show when={hasFeatures()}>
            <button classList={{ on: features() }} aria-pressed={features()} onClick={() => { setFeatures(!features()); scene?.setFeatures(features()); }} title="Teleporters with their links, and gravity lifts">Lifts &amp; teleporters</button>
          </Show>
          <Show when={cut()}>
            <input
              class="v3-cut" type="range" aria-label="Cut height" title={mode() === "follow" ? "Following the player's height" : "Cut height"}
              min={range()[0]} max={range()[1]} step={(range()[1] - range()[0]) / 100 || 0.01} value={cutValue()} disabled={mode() === "follow"}
              onInput={(e) => moveCut(+e.currentTarget.value)}
            />
          </Show>
        </div>
        <Show when={mapState() === "loading"}><div class="v3-note">Loading the map…</div></Show>
        <Show when={mapState() === "grid" && late()}><div class="v3-note">No 3D geometry for this map; the floor grid is fitted to where players have been.</div></Show>
        <Show when={mapState() !== "loading" && staleModels()}>
          <div class="v3-note">This server's player models are from an older art extraction, so they can't animate. Extract the art again: <code>h2 live assets extract</code>.</div>
        </Show>
      </Show>
    </div>
  );
}
