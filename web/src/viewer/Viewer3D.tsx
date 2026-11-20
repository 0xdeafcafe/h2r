// The 3D view itself: a ViewerScene in a box, with its camera controls.
// Loaded lazily (it brings three.js), by ViewerWindow and the pop-out page.

import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import type { MapFrame } from "../components/MapCanvas";
import type { RosterEntry } from "../map/render";
import { onAnimationFrame } from "../map/scheduler";
import { gameArt, gameArtReady } from "../state/gameart";
import { clampCut, nextSee, stepCut, type SeeMode } from "./cutaway";
import type { LevelInfo } from "./level3d";
import { modelsAnimate } from "./models";
import { zoneInPlay, type ZonePick } from "./zones";
import { savePref, texturesPref } from "./textures3d";
import { KIND_LABELS } from "./scenario3d";
import { ViewerScene, hasWebGl, type ViewMode } from "./scene";
import type { CameraStart } from "./start";
import type { ViewerDirector } from "./camera";
import { cameraFresh, type CameraRequest } from "./sync";
import { Overlay } from "./Overlay.tsx";
import { loadProgress, overlayState } from "./overlay";
import { ViewMenu, type ViewPanel, type ViewRow } from "./ViewMenu";
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
  player?: string;
  title: string;
  mapName: string;
  backdrop?: string | null;
  on2D: () => void;
  start?: (frame: MapFrame, player?: string) => CameraStart;
  priorityPlayers?: () => readonly (number | null)[];
  /** Camera requests from the replay (a killcam): each new one is taken. */
  camera?: () => CameraRequest | null;
  director?: ViewerDirector;
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
  // A cutscene: the frames hold no one; the view says so.
  const [cutscene, setCutscene] = createSignal(false);
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
  const [flySpeed, setFlySpeed] = createSignal("");
  let hintTimer: ReturnType<typeof setTimeout> | undefined;
  const hint = (text: string, ms = 1600) => { setFlyHint(text); clearTimeout(hintTimer); hintTimer = setTimeout(() => setFlyHint(""), ms); };
  onCleanup(() => clearTimeout(hintTimer));
  let root!: HTMLDivElement;
  const fly = (on: boolean) => {
    if (!scene || scene.flying === on) return;
    scene.setFly(on);
    if (scene.flying) hint("Move with WASD or the arrow keys. Q and E move down and up. Drag to look, click to lock the pointer, scroll to change speed, and hold Shift to boost. Press F to leave. On touch screens, one finger looks and two fingers fly.", 6000);
    else setFlyHint("");
  };
  const toggleFly = () => {
    if (!scene) return;
    explicit = true;
    fly(!scene.flying);
    root.focus();
  };
  const onKeyUp = (e: KeyboardEvent) => { if (scene?.flyKey(e, false)) { e.preventDefault(); e.stopPropagation(); } };
  const onKey = (e: KeyboardEvent) => {
    touchHints();
    const typing = (e.target as HTMLElement).closest("input, select, textarea, [contenteditable]");
    const menuControl = (e.target as HTMLElement).closest(".vo-menu");
    if (menuControl || !controlsShown()) return;
    const key = e.key.toLowerCase();
    // Stopped here: the pop-out page takes Space for play and pause.
    if (!typing && scene?.flyKey(e, true)) { e.preventDefault(); e.stopPropagation(); return; }
    if (e.ctrlKey || e.metaKey || e.altKey || typing) return;
    if (key === "f") {
      const d = props.director;
      if (d) {
        const previous = mode();
        d.pick(flying() ? (previous === "top" ? "top-down" : previous) : "free", d.selection().follow);
      } else toggleFly();
    } else if (key === "c") toggleCut();
    else if (e.key === "[" || e.key === "]") { if (scene) setRange(scene.cutRange()); moveCut(stepCut(cutValue(), e.key === "]" ? 1 : -1, range())); }
    else if (key === "x") toggleSee();
    else if (key === "v") {
      const d = props.director;
      if (d) d.pick(mode() === "eyes" && !flying() ? "follow" : "eyes", d.selection().follow);
      else toggleEyes();
    } else if (key === "r") reset();
    else if (key === "t") {
      if (props.director) props.director.pick("top-down", props.director.selection().follow);
      else pick("top");
    } else if (key === "m") {
      setBare(false);
      if (menu()) closeMenu(true);
      else setMenu("view");
    } else if (key === "h") {
      setBare(!bare());
      closeMenu(false);
      root.focus({ preventScroll: true });
    } else return;
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
  const [show, setShow] = createSignal({ cinematic: false, backdrop: false, decals: true, effects: true, sky: true, fog: true, weather: true });
  const flip = (k: keyof ReturnType<typeof show>) => {
    const on = !show()[k];
    setShow({ ...show(), [k]: on });
    if (!scene) return;
    if (k === "cinematic") scene.setCinematic(on);
    else if (k === "backdrop") scene.setBackdrop(on);
    else if (k === "decals") scene.setDecals(on);
    else if (k === "effects") scene.setEffects(on);
    else if (k === "sky") scene.setSky(on);
    else if (k === "weather") scene.setWeather(on);
    else scene.setFog(on);
    setLevel(scene.levelInfo());
  };
  const applyLevel = (s: ViewerScene) => {
    const o = show();
    s.setCinematic(o.cinematic); s.setBackdrop(o.backdrop); s.setDecals(o.decals); s.setEffects(o.effects); s.setSky(o.sky); s.setFog(o.fog); s.setWeather(o.weather);
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
  const [sceneReady, setSceneReady] = createSignal(false);
  const [phase, setPhase] = createSignal<"fetching" | "decoding" | "ok" | "failed">("fetching");
  const [reveal, setReveal] = createSignal<"started" | "done">();
  const [progress, setProgress] = createSignal(loadProgress("boot"));
  const [surfaces, setSurfaces] = createSignal({ done: 0, total: 0 });
  const [retry, setRetry] = createSignal(0);
  const [late, setLate] = createSignal(false);
  const state = () => overlayState({
    sceneReady: sceneReady(), unavailable: !!failed(), glb: props.glb(),
    manifestReady: gameArtReady(), waitExpired: late(), phase: phase(), reveal: reveal(),
  });
  const controlsShown = () => ["reveal", "ready", "none", "error"].includes(state());
  const [bare, setBare] = createSignal(false);
  const [keysIdle, setKeysIdle] = createSignal(false);
  let keysTimer: ReturnType<typeof setTimeout> | undefined;
  let revealTimer: ReturnType<typeof setTimeout> | undefined;
  const touchHints = () => {
    setKeysIdle(false);
    clearTimeout(keysTimer);
    keysTimer = setTimeout(() => setKeysIdle(true), 4000);
  };
  createEffect(on(controlsShown, on => { if (on) touchHints(); }));
  onCleanup(() => { clearTimeout(keysTimer); clearTimeout(revealTimer); });
  // Models from before version 2 have no clips: say why nobody moves.
  const staleModels = createMemo(() => {
    const models = gameArt()?.manifest.models;
    return !!models && Object.keys(models.bipeds).length > 0 && !modelsAnimate(models);
  });
  const people = createMemo(() => [...props.roster()].map(([index, r]) => ({ index, ...r })).sort((a, b) => a.name.localeCompare(b.name)));

  const onPlayer = (m: ViewMode) => m === "follow" || m === "eyes";
  const pick = (m: ViewMode, index: number | null = follow()) => {
    explicit = true;
    if (onPlayer(m) && index === null) index = people()[0]?.index ?? null;
    if (onPlayer(m) && index === null) return;
    if (scene?.flying) {
      fly(false);
      if (!props.director) root.focus();
    }
    setMode(m);
    setFollow(onPlayer(m) ? index : follow());
    scene?.setMode(m, index);
  };
  /** First person and back to the chase camera, on the player followed (or the first). */
  const toggleEyes = () => pick(mode() === "eyes" ? "follow" : "eyes");
  // A killcam (or another window's) asks for a camera: take each new one,
  // and one made just before this view opened (it opened for it).
  let opened = false;
  let opening: CameraStart | null = null;
  let openingApplied = false;
  let adopted: number | null = null;
  let explicit = props.mode !== undefined;
  createEffect(on(() => props.camera?.() ?? null, (c) => {
    if (c && (opened || cameraFresh(c))) { explicit = true; pick(c.mode, c.follow); }
    opened = true;
  }));

  const applySelection = (selection = props.director?.selection()) => {
    if (!selection) return;
    if (selection.mode === "free") fly(true);
    else if (selection.mode !== "top-down") pick(selection.mode, selection.follow);
  };
  createEffect(on(() => props.director?.selection(), (selection) => {
    applySelection(selection);
    if (scene?.flying) root.focus({ preventScroll: true });
  }));

  onMount(() => {
    if (!hasWebGl()) { setFailed("This browser can't draw 3D here (WebGL is off or unavailable). The 2D map still shows the game."); return; }
    const perf = new URLSearchParams(location.search).has("perf");
    const still = reducedMotion();
    try {
      scene = new ViewerScene(host, {
        reducedMotion: still,
        reveal: !perf,
        onPick: (i) => props.director ? props.director.pick("follow", i) : pick("follow", i),
      });
    } catch {
      setFailed("The 3D view couldn't start on this device. The 2D map still shows the game.");
      return;
    }
    const s = scene;
    // `?perf` in the address: the scene on window for the perf harness
    // (web/bench/viewer.bench.ts) to read its renderer's counts.
    if (perf) (window as unknown as { h2Viewer?: ViewerScene }).h2Viewer = s;
    s.onAoDropped = () => setAo(false);
    s.onFlyChange = setFlying;
    s.onFlySpeed = setFlySpeed;
    s.onSurfaceProgress = (done, total) => setSurfaces({ done, total });
    let revealedAt = 0;
    s.onReveal = (state) => {
      if (state === "started") {
        revealedAt = performance.now();
        setProgress(loadProgress("reveal"));
        setReveal("started");
      } else {
        const remaining = (still ? 200 : 420) - (performance.now() - revealedAt);
        if (remaining > 0) revealTimer = setTimeout(() => setReveal("done"), remaining);
        else setReveal("done");
      }
    };
    s.setCurtain(true);
    setSceneReady(true);
    const lateTimer = setTimeout(() => setLate(true), 2500);
    onCleanup(() => clearTimeout(lateTimer));
    applySelection();
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
    let seen = 0, sourced: number | null = null;
    const stop = onAnimationFrame((now) => {
      if (!visible || document.hidden) return;
      const frame = props.frame(now);
      const sought = props.soughtAt?.() ?? 0;
      if (adopted === null && !props.glb() && (gameArtReady() || late())) adopted = now;
      const automatic = () => props.start?.(frame, props.player) ?? { kind: "bounds" } as const;
      if (!opening && (frame.players.length > 0 || (adopted !== null && now - adopted >= 1500))) {
        opening = explicit || props.director ? { kind: "bounds" } : automatic();
      }
      if (opening && !openingApplied && adopted !== null) {
        openingApplied = true;
        if (!explicit && !props.director && !s.flying) {
          // A player can leave while the geometry is still on the wire.
          const index = opening.kind === "player" ? opening.index : null;
          const start = index !== null && !frame.players.some(p => p.index === index) ? automatic() : opening;
          s.setStart(start);
          setMode(start.kind === "player" ? "follow" : "orbit");
          if (start.kind === "player") setFollow(start.index);
        } else {
          if (props.director) applySelection();
          let index = follow();
          if (onPlayer(mode()) && index === null) {
            index = people()[0]?.index ?? null;
            setFollow(index);
          }
          if (!s.flying) s.setInitialMode(mode(), index);
        }
      }
      if (adopted !== null && (openingApplied || phase() !== "ok") && sourced !== adopted) {
        sourced = adopted;
        s.setCurtain(false);
      }
      s.update(now, frame, props.roster(), props.highlight?.() ?? null, sought !== seen, props.priorityPlayers?.());
      seen = sought;
      if (!!frame.actors?.length !== hasActors()) setHasActors(!!frame.actors?.length);
      if (!!frame.cinematic !== cutscene()) setCutscene(!!frame.cinematic);
      if ((frame.bsp ?? null) !== (level()?.active ?? null)) setLevel(s.levelInfo());
    });
    onCleanup(() => { stop(); ro.disconnect(); io.disconnect(); s.dispose(); scene = null; });
  });

  // Player models, when the server's art has them: each glb is fetched the
  // first time a player needs it. Without them players are capsules.
  createEffect(() => {
    const art = gameArt();
    scene?.setModels(art?.root ?? null, art?.manifest.models);
  });

  // Each retry starts its own byte, decode and first-draw sequence.
  createEffect(on([sceneReady, props.glb, retry], ([ready, url]) => {
    const s = scene;
    if (!ready || !s) return;
    let current = true;
    onCleanup(() => { current = false; });
    adopted = null;
    s.setCurtain(true);
    clearTimeout(revealTimer);
    setReveal(undefined);
    closeMenu(false);
    setSurfaces({ done: 0, total: 0 });
    setPhase("fetching");
    setProgress(loadProgress("loading"));
    if (!url) { s.useGrid(); return; }
    void s.loadMap(url, (event) => {
      if (!current) return;
      if (!event) {
        setPhase("decoding");
        setProgress(p => loadProgress("decoding", 0, undefined, p.lit));
      } else if (phase() === "fetching") {
        setProgress(p => loadProgress("loading", event.loaded, event.lengthComputable ? event.total : undefined, p.lit));
      }
    }).then((result) => {
      if (!current) return;
      adopted = performance.now();
      setPhase(result);
      setRange(s.cutRange()); applyCut(); setZonePick("auto");
      applyLevel(s); applyObjects(s); setHasFeatures(s.hasFeatures()); s.setFeatures(features());
    });
  }));

  const reset = () => {
    explicit = true;
    if (props.director) props.director.pick("orbit", props.director.selection().follow);
    else if (onPlayer(mode())) pick("orbit");
    if (flying()) fly(false);
    scene?.reset();
  };
  const [menu, setMenu] = createSignal<"view" | "follow" | null>(null);
  const [path, setPath] = createSignal<string[]>([]);
  let viewButton!: HTMLButtonElement;
  let followButton!: HTMLButtonElement;
  const trigger = () => menu() === "follow" ? followButton : viewButton;
  const closeMenu = (restore: boolean) => {
    const control = trigger();
    setMenu(null);
    setPath([]);
    if (restore) control?.focus({ preventScroll: true });
  };
  const back = () => path().length ? setPath(p => p.slice(0, -1)) : closeMenu(true);
  const toggleRow = (label: string, checked: boolean, pick: () => void, show = true, disabled = false): ViewRow =>
    ({ label, checked, pick, show, disabled });
  const choice = (label: string, selected: boolean, pick: () => void): ViewRow => ({ label, selected, pick });
  const selectRow = (title: string, value: string | undefined, rows: readonly ViewRow[], show = true): ViewRow =>
    ({ label: title, value, panel: { title, rows }, show });

  const seeLabels = { off: "Off", auto: "In focus", always: "Everyone" } as const;
  const pickZone = (p: ZonePick) => { setZonePick(p); scene?.setZonePick(p); };
  const layerRows = () => [
    toggleRow("Cutscene zones", show().cinematic, () => flip("cinematic"), (level()?.cinematicCount ?? 0) > 0),
    toggleRow("Backdrop", show().backdrop, () => flip("backdrop"), !!level()?.backdrop),
    toggleRow("Decals", show().decals, () => flip("decals"), !!level()?.decals),
    toggleRow("Effects", show().effects, () => flip("effects"), !!level()?.effects),
    toggleRow("Sky", show().sky, () => flip("sky"), !!level()?.sky),
    toggleRow("Fog", show().fog, () => flip("fog")),
    toggleRow("Weather", show().weather, () => flip("weather"), !!level()?.weather),
    toggleRow("Objects", objects(), () => { setObjects(!objects()); scene?.setObjects(objects()); }, objectKinds().length > 0),
    ...objectKinds().map(k => toggleRow(KIND_LABELS[k] ?? k, !hiddenKinds().has(k), () => flipKind(k), true, !objects())),
  ];
  const viewPanel = (): ViewPanel => ({
    title: "View",
    rows: [
      toggleRow("Ambient occlusion", ao(), () => setAo(scene?.setAo(!ao()) ?? false)),
      selectRow("See-through", seeLabels[see()], (["off", "auto", "always"] as const).map(
        m => choice(seeLabels[m], see() === m, () => pickSee(m)))),
      toggleRow("Height cut", cut(), toggleCut),
      toggleRow("Textures", textures(), flipTextures, hasTextures()),
      toggleRow("Lifts & teleporters", features(), () => { setFeatures(!features()); scene?.setFeatures(features()); }, hasFeatures()),
      toggleRow("AI names", actorNames(), () => { setActorNames(!actorNames()); scene?.setActorNames(actorNames()); }, hasActors()),
      selectRow("Zone", zonePick() === "auto" ? zoneInPlay(level()) : zonePick() === "all" ? "All zones"
        : level()?.zones.find(z => z.bsp === zonePick())?.label, [
        choice(zoneInPlay(level()), zonePick() === "auto", () => pickZone("auto")),
        choice("All zones", zonePick() === "all", () => pickZone("all")),
        ...(level()?.zones ?? []).map(z => choice(z.label + (z.cinematic ? " (cutscene)" : ""), zonePick() === z.bsp, () => pickZone(z.bsp))),
      ], (level()?.zoneCount ?? 0) > 1),
      selectRow("Level layers", undefined, layerRows(), !!level() && phase() === "ok"),
    ],
  });
  const following = () => onPlayer(mode()) && !flying();
  const followName = () => following() ? people().find(p => p.index === follow())?.name : undefined;
  const panels = createMemo(() => {
    if (menu() === "follow") return [{ title: "Follow a player", rows: people().map(p =>
      choice(p.bot ? `${p.name} (BOT)` : p.name, following() && p.index === follow(),
        () => pick(mode() === "eyes" ? "eyes" : "follow", p.index))) }];
    const out = [viewPanel()];
    for (const label of path()) {
      const row = out.at(-1)!.rows.find(r => r.label === label && r.show !== false);
      if (!row || !("panel" in row)) break;
      out.push(row.panel);
    }
    return out;
  });
  const status = () => {
    const { done, total } = surfaces();
    const detail = done < total ? `Surfaces ${done}/${total}` : flying() ? `Flycam ${flySpeed().replace(/^Speed /, "")}` : cutscene() ? "Cutscene" : "";
    return (
      <>
        <div>{props.mapName}{detail ? ` · ${detail}` : ""}</div>
        <Show when={flyHint()}><div class="vo-note">{flyHint()}</div></Show>
        <Show when={staleModels()}>
          <div class="vo-note vo-note--warn">This server's player models can't animate. Extract the art again with{" "}<code>h2 live assets extract</code>.</div>
        </Show>
      </>
    );
  };

  return (
    <div class="v3" tabindex="0" ref={root} onKeyDown={onKey} onKeyUp={onKeyUp} onPointerMove={touchHints}
      onFocusOut={(e) => { if (!root.contains(e.relatedTarget as Node | null)) scene?.flyRelease(); }}>
      <div ref={host} class="v3-host" />
      <Overlay state={state()} title={props.title} backdrop={props.backdrop} progress={progress()}
        error={failed()} onRetry={() => setRetry(n => n + 1)} on2D={props.on2D} bare={bare()} status={status()}>
        <div class="vo-rail" role="group" aria-label="Viewer controls">
          <Show when={!props.director}>
            <div class="vo-tabs" role="group" aria-label="Camera">
              <button type="button" aria-pressed={!flying() && mode() === "orbit"} onClick={() => pick("orbit")}>Orbit</button>
              <button type="button" aria-pressed={!flying() && mode() === "top"} onClick={() => pick("top")} title="Straight down, north up (T)">Top</button>
              <button type="button" aria-pressed={flying()} onClick={toggleFly} title="Fly freely (F)">Fly</button>
            </div>
            <button type="button" ref={followButton} class="vo-follow" aria-haspopup="dialog" aria-expanded={menu() === "follow"}
              disabled={!people().length} onClick={() => menu() === "follow" ? closeMenu(true) : (setPath([]), setMenu("follow"))}>
              {followName() ? `Follow: ${followName()}` : "Follow"} <span aria-hidden="true">⌄</span>
            </button>
            <Show when={following()}>
              <button type="button" aria-pressed={mode() === "eyes"} onClick={toggleEyes} title="Through the followed player's eyes (V)">Eyes</button>
            </Show>
          </Show>
          <span class="vo-spacer" />
          <button type="button" onClick={reset} title="Frame the playable area (R)">Reset</button>
          <button type="button" ref={viewButton} aria-haspopup="menu" aria-expanded={menu() === "view"}
            onClick={() => menu() === "view" ? closeMenu(true) : (setPath([]), setMenu("view"))}>View</button>
        </div>
        <Show when={menu()}>
          <ViewMenu panels={panels()} picker={menu() === "follow"} open={label => setPath(p => [...p, label])}
            backLabel={path().at(-1)} back={back} close={closeMenu} trigger={trigger} />
        </Show>
        <Show when={cut()}>
          <input class="vo-cut" type="range" aria-label="Cut height" title={mode() === "follow" ? "Following the player's height" : "Cut height"}
            min={range()[0]} max={range()[1]} step={(range()[1] - range()[0]) / 100 || 0.01} value={cutValue()} disabled={mode() === "follow"}
            onInput={e => moveCut(+e.currentTarget.value)} />
        </Show>
        <div class="vo-keys" classList={{ "vo-keys--idle": keysIdle() }} aria-hidden="true">
          <span><kbd>F</kbd> Fly</span><span><kbd>V</kbd> Eyes</span><span><kbd>C</kbd> Cut</span><span><kbd>M</kbd> Menu</span>
        </div>
      </Overlay>
    </div>
  );
}
