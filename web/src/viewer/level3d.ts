// What of a loaded level the 3D view shows, and the air around it: the
// zones (`bsp_<n>`: the one in play, one picked, or all; cutscene zones on
// request), the far scenery, the layers (decals, effects), the sky and the
// fog, the water's ripples and the weather (water3d.ts, weather3d.ts). The scene hands it the glb once loaded and asks it to dress each
// frame before drawing; the toolbar reads `info()` and flips its switches.
// Kept apart from the players, vehicles and machines.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import { FOG_CLEAR, SkyDome, gameFog, useGameFog, skyColour, skyOf, skyUrl, type SkyInfo } from "./sky3d";
import { waterUniforms } from "./water3d";
import { WeatherLook, weatherOf } from "./weather3d";
import { bspNumber, isBackdrop, isCinematic, layerNode, zoneList, zoneVisibility, type ZoneBox, type ZoneEntry, type ZonePick } from "./zones";

/** What the toolbar offers for a level. */
export interface LevelInfo {
  /** The zones a picker lists (cutscene ones only when asked for), and
   *  how many the level has, and how many of them are cutscene-only. */
  zones: ZoneEntry[];
  zoneCount: number;
  cinematicCount: number;
  /** The zone in play (the frames' and the players' word, viewer/zones.ts), if any. */
  active: number | null;
  backdrop: boolean;
  decals: boolean;
  effects: boolean;
  sky: boolean;
  fog: boolean;
  weather: boolean;
}

export class LevelLook {
  private zones: Array<{ obj: THREE.Object3D; name: string; cinematic: boolean; label?: string }> = [];
  private layers: Array<{ obj: THREE.Object3D; kind: "decals" | "effects" }> = [];
  private active: number | null = null;
  private pick: ZonePick = "auto";
  private cinematic = false;
  private backdrop = false;
  private decals = true;
  private effects = true;
  private skyOn = true;
  private fogOn = true;
  private weatherOn = true;
  private weather = new WeatherLook();
  private hazeColour = new THREE.Color();
  private hazeDensity = 0.002;
  /** The uniforms the level's water shares (water3d.ts); `wet` once a
   *  level has some. */
  readonly water = waterUniforms();
  wet = false;
  private sky: SkyDome | null = null;
  private skyInfo: SkyInfo | null = null;
  private haze: THREE.FogExp2;
  private fog = new THREE.Fog(0xffffff, 1, 2);
  private background: THREE.Color | null = null;
  private generation = 0;
  private centre = new THREE.Vector3();
  /** Each zone's extent in game coordinates, measured once a level is in. */
  private boxes: ZoneBox[] = [];

  /** `gradient` is the view's own sky, used when a level has none. */
  constructor(private scene: THREE.Scene, private gradient: THREE.Texture, private changed: () => void) {
    useGameFog();
    this.haze = scene.fog instanceof THREE.FogExp2 ? scene.fog : new THREE.FogExp2(0x7f9fb6, 0.002);
    this.hazeColour.copy(this.haze.color);
    scene.add(this.weather.group);
  }

  /** Takes a loaded level: finds its zones and layers wherever they are in
   *  the tree, and loads its sky (in the background). */
  adopt(root: THREE.Object3D, assetExtras: unknown, mapUrl: string) {
    this.clear();
    const gen = this.generation;
    root.traverse((o) => {
      const kind = layerNode(o.name);
      if (kind) { this.layers.push({ obj: o, kind }); return; }
      if (bspNumber(o.name) === null && !isBackdrop(o.name)) return;
      const label = typeof o.userData?.name === "string" && o.userData.name !== o.name ? o.userData.name : undefined;
      this.zones.push({ obj: o, name: o.name, cinematic: isCinematic(o.userData, o.name, assetExtras), label });
    });
    root.updateWorldMatrix(true, true);
    const box = new THREE.Box3();
    for (const z of this.zones) {
      const bsp = bspNumber(z.name);
      if (bsp === null || this.boxes.some((b) => b.bsp === bsp)) continue;
      box.setFromObject(z.obj);
      if (box.isEmpty()) continue;
      // gl (x, y, z) is game (x, -z, y).
      this.boxes.push({ bsp, cinematic: z.cinematic, min: [box.min.x, -box.max.z, box.min.y], max: [box.max.x, -box.min.z, box.max.y] });
    }
    this.skyInfo = skyOf(assetExtras);
    const c = skyColour(this.skyInfo);
    this.background = c ? new THREE.Color().setRGB(c[0], c[1], c[2], THREE.SRGBColorSpace) : null;
    // The water reflects the sky's colour (or its fog's). Not the `glow`:
    // that is the sky light's colour (sky3d.ts), not the air's.
    const r = this.skyInfo?.clear ?? this.skyInfo?.fog?.colour;
    if (r) this.water.uWaterSky.value.setRGB(r[0], r[1], r[2], THREE.SRGBColorSpace);
    else this.water.uWaterSky.value.setRGB(0.55, 0.68, 0.8, THREE.SRGBColorSpace);
    this.weather.adopt(weatherOf(assetExtras));
    this.skyReady = Promise.resolve();
    if (this.skyInfo?.glb) {
      const dome = new SkyDome();
      this.skyReady = dome.load(skyUrl(mapUrl, this.skyInfo.glb)).then((ok) => {
        if (gen !== this.generation || !ok) { dome.dispose(); return; }
        this.sky = dome;
        this.scene.add(dome.group);
        this.apply();
      });
    }
    this.apply();
  }

  /** Settles when the current level's sky has loaded (or failed to). */
  skyReady: Promise<void> = Promise.resolve();

  clear() {
    this.generation++;
    this.zones = [];
    this.boxes = [];
    this.layers = [];
    this.sky?.dispose();
    this.sky = null;
    this.skyInfo = null;
    this.background = null;
    this.weather.clear();
    this.wet = false;
    this.apply();
  }

  /** The zone in play; whether that changed it. */
  setActive(bsp: number | null): boolean {
    if (bsp === this.active) return false;
    this.active = bsp;
    this.apply();
    return true;
  }

  /** The zones' extents (game coordinates), cutscene ones marked. */
  zoneBoxes(): readonly ZoneBox[] { return this.boxes; }
  /** One zone's extent, if the level has it. */
  zoneBox(bsp: number): ZoneBox | undefined { return this.boxes.find((b) => b.bsp === bsp); }
  /** Whether the view follows the zone in play (no zone picked by hand). */
  following(): boolean { return this.pick === "auto"; }
  /** Whether cutscene zones are offered. */
  cinematicOn(): boolean { return this.cinematic; }

  setPick(p: ZonePick) { this.pick = p; this.apply(); }
  setCinematic(on: boolean) { this.cinematic = on; this.apply(); }
  setBackdrop(on: boolean) { this.backdrop = on; this.apply(); }
  setDecals(on: boolean) { this.decals = on; this.apply(); }
  setEffects(on: boolean) { this.effects = on; this.apply(); }
  setSky(on: boolean) { this.skyOn = on; this.apply(); }
  setFog(on: boolean) { this.fogOn = on; this.apply(); }
  setWeather(on: boolean) { this.weatherOn = on; this.weather.setOn(on); this.apply(); }

  /** Moves what moves by itself (the water's ripples, the weather) to
   *  `time` (seconds), the weather around `focus`, `distance` from the
   *  camera. Whether anything did: the view must be drawn again. */
  animate(time: number, focus: THREE.Vector3, distance: number): boolean {
    const weather = this.weather.active();
    if (weather) this.weather.place(time, focus, distance);
    if (this.wet) this.water.uWaterTime.value = time;
    return weather || this.wet;
  }

  info(): LevelInfo {
    const nodes = this.zones.map((z) => ({ name: z.name, cinematic: z.cinematic, label: z.label }));
    const all = zoneList(nodes, { cinematic: true });
    return {
      zones: zoneList(nodes, { cinematic: this.cinematic }),
      zoneCount: all.length,
      cinematicCount: all.filter((z) => z.cinematic).length,
      active: this.active,
      backdrop: this.zones.some((z) => isBackdrop(z.name)),
      decals: this.layers.some((l) => l.kind === "decals"),
      effects: this.layers.some((l) => l.kind === "effects"),
      sky: !!this.skyInfo,
      fog: !!this.skyInfo?.fog,
      weather: this.weather.has(),
    };
  }

  /** Shows and hides what the switches say, and sets the background. */
  private apply() {
    const show = zoneVisibility(this.zones, this.active, { backdrop: this.backdrop, cinematic: this.cinematic, pick: this.pick });
    this.zones.forEach((z, i) => { z.obj.visible = show[i]!; });
    for (const l of this.layers) l.obj.visible = l.kind === "decals" ? this.decals : this.effects;
    if (this.sky) this.sky.group.visible = this.skyOn;
    this.scene.background = this.skyOn && this.background ? this.background : this.gradient;
    this.weather.group.visible = this.weatherOn;
    this.changed();
  }

  /** Whether the level's sky model is drawn now. */
  skyShown(): boolean { return !!this.sky && this.skyOn; }

  /** Where the level is (its middle and radius in the scene): the far
   *  haze is set by its size when the sky has no fog of its own. */
  setExtent(centre: THREE.Vector3, radius: number) {
    this.centre.copy(centre);
    this.haze.density = 0.11 / radius;
    this.hazeDensity = this.haze.density;
  }

  /** Before each draw: the sky around the camera, and the fog measured
   *  from the point `focus` units in front of it. */
  beforeDraw(camera: THREE.PerspectiveCamera, focus: number) {
    if (this.sky?.group.visible) this.sky.follow(camera);
    const f = this.skyInfo?.fog;
    if (!this.fogOn) { this.scene.fog = null; return; }
    if (!f) {
      // No fog of its own: the game draws such a level clear. Only the
      // viewer's faint haze, so far scenery still reads as far. (The sky's
      // `glow` is not a fog: it is its light's colour and power, the
      // radiosity record; as a haze it drowned Halo 1's maps in cyan.)
      this.haze.color.copy(this.hazeColour);
      this.haze.density = this.hazeDensity;
      this.scene.fog = this.haze;
      return;
    }
    const r = gameFog(f, focus);
    FOG_CLEAR.value = 1 - r.cap;
    this.fog.color.setRGB(f.colour[0], f.colour[1], f.colour[2], THREE.SRGBColorSpace);
    this.fog.near = r.near;
    this.fog.far = r.far;
    this.scene.fog = this.fog;
  }

  dispose() { this.clear(); this.weather.dispose(); }
}
