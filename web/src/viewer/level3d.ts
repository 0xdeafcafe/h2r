// What of a loaded level the 3D view shows, and the air around it: the
// zones (`bsp_<n>`: the one in play, one picked, or all; cutscene zones on
// request), the far scenery, the layers (decals, effects), the sky and the
// fog. The scene hands it the glb once loaded and asks it to dress each
// frame before drawing; the toolbar reads `info()` and flips its switches.
// Kept apart from the players, vehicles and machines.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import { SkyDome, fogRange, skyColour, skyOf, skyUrl, type SkyInfo } from "./sky3d";
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
  private sky: SkyDome | null = null;
  private skyInfo: SkyInfo | null = null;
  private haze: THREE.FogExp2;
  private fog = new THREE.Fog(0xffffff, 1, 2);
  private background: THREE.Color | null = null;
  private generation = 0;
  private centre = new THREE.Vector3();
  private radius = 1;
  /** Each zone's extent in game coordinates, measured once a level is in. */
  private boxes: ZoneBox[] = [];

  /** `gradient` is the view's own sky, used when a level has none. */
  constructor(private scene: THREE.Scene, private gradient: THREE.Texture, private changed: () => void) {
    this.haze = scene.fog instanceof THREE.FogExp2 ? scene.fog : new THREE.FogExp2(0x7f9fb6, 0.002);
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
    if (this.skyInfo?.glb) {
      const dome = new SkyDome();
      void dome.load(skyUrl(mapUrl, this.skyInfo.glb)).then((ok) => {
        if (gen !== this.generation || !ok) { dome.dispose(); return; }
        this.sky = dome;
        this.scene.add(dome.group);
        this.apply();
      });
    }
    this.apply();
  }

  clear() {
    this.generation++;
    this.zones = [];
    this.boxes = [];
    this.layers = [];
    this.sky?.dispose();
    this.sky = null;
    this.skyInfo = null;
    this.background = null;
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
    };
  }

  /** Shows and hides what the switches say, and sets the background. */
  private apply() {
    const show = zoneVisibility(this.zones, this.active, { backdrop: this.backdrop, cinematic: this.cinematic, pick: this.pick });
    this.zones.forEach((z, i) => { z.obj.visible = show[i]!; });
    for (const l of this.layers) l.obj.visible = l.kind === "decals" ? this.decals : this.effects;
    if (this.sky) this.sky.group.visible = this.skyOn;
    this.scene.background = this.skyOn && this.background ? this.background : this.gradient;
    this.changed();
  }

  /** Whether the level's sky model is drawn now. */
  skyShown(): boolean { return !!this.sky && this.skyOn; }

  /** Where the level is (its middle and radius in the scene): the far
   *  haze is set by its size when the sky has no fog of its own, and the
   *  sky's fog stretched to it. */
  setExtent(centre: THREE.Vector3, radius: number) {
    this.centre.copy(centre);
    this.radius = radius;
    this.haze.density = 0.11 / radius;
  }

  /** Before each draw: the sky around the camera, and the fog measured
   *  from the point `focus` units in front of it. */
  beforeDraw(camera: THREE.PerspectiveCamera, focus: number) {
    if (this.sky?.group.visible) this.sky.follow(camera);
    const f = this.skyInfo?.fog;
    if (!this.fogOn) { this.scene.fog = null; return; }
    if (!f) { this.scene.fog = this.haze; return; }
    const r = fogRange(f, focus, camera.position.distanceTo(this.centre) + this.radius);
    this.fog.color.setRGB(f.colour[0], f.colour[1], f.colour[2], THREE.SRGBColorSpace);
    this.fog.near = r.near;
    this.fog.far = r.far;
    this.scene.fog = this.fog;
  }

  dispose() { this.clear(); }
}
