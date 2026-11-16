// The 3D game viewer's scene: a map's glb (or a grid floor when there is
// none) under a sun, with players as their species' model in their team or
// own colour, holding their weapon (a capsule when the server has no
// models), an aim ray from the head, name labels, muzzle flashes and
// tracers when they fire, damage drawn from attacker to victim, grenades
// in flight, every vehicle in the world, and fading knockout marks. A
// skinned model plays its clips (standing, walking, running, crouching, in
// the air, throwing, hitting, reloading, firing) as the frames say; a
// static one glides in the pose it was exported in. A campaign level shows
// the zone the game has loaded. Game positions (z up) are placed at glTF
// (x, z, -y), as the map geometry contract says.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { CSS2DObject, CSS2DRenderer } from "three/examples/jsm/renderers/CSS2DRenderer.js";
import { RoomEnvironment } from "three/examples/jsm/environments/RoomEnvironment.js";
import { EffectComposer } from "three/examples/jsm/postprocessing/EffectComposer.js";
import { RenderPass } from "three/examples/jsm/postprocessing/RenderPass.js";
import { GTAOPass } from "three/examples/jsm/postprocessing/GTAOPass.js";
import { OutputPass } from "three/examples/jsm/postprocessing/OutputPass.js";
import type { MapFrame } from "../components/MapCanvas";
import type { RosterEntry } from "../map/render";
import { gameToGl, yawToGl } from "../map/views";
import { FLASH_MS, ShotQueue, flashAlpha } from "../map/shots";
import { actorColour, bipedFile, modelScale, modelUrl, vehicleBasis, vehicleBox, vehicleFile, weaponFile, weaponLabel, type PlayerModels } from "./models";
import type { SampledVehicle } from "../state/frames";
import { ModelLibrary, type BipedInstance, type WeaponInstance } from "./players3d";
import { smoothPlayers, smoothVehicles, type SmoothPlayer } from "./sync";
import { BipedAnim, SpeedMeter, actionShot, clipFor, paceScale, weaponClass, type OneShot } from "./anim";
import { EventFeed } from "../map/events";
import { Effects3D } from "./effects3d";
import { bspNumber, isBackdrop, isCinematic, zoneVisibility } from "./zones";
import { machineMatrix, machineNode, placeMachine, type MachineNode } from "./machines3d";
import { buildFeatures } from "./features3d";
import { parseFeatures, type MapFeatures } from "../map/features";
import type { SampledMachine } from "../state/frames";
import { cameraFit, followDistance, markerScale, type Box3 } from "./camera";
import { SEE_RADIUS, cutHeight, cutRange, meshRole, patchSeeThrough } from "./cutaway";

export type ViewMode = "orbit" | "top" | "follow";

export interface SceneOptions {
  reducedMotion: boolean;
  /** Called when a player's label is clicked. */
  onPick?: (index: number) => void;
}

// The sun, in game coordinates: the same corner the pre-drawn views are lit from.
const SUN_GAME: [number, number, number] = [-0.45, -0.35, 0.82];
const SKY_TOP = 0x1b3a5e, SKY_HORIZON = 0x8fb0c8, HAZE = 0x7f9fb6;

interface PlayerObj {
  /** At the player's feet; never turned or scaled. */
  group: THREE.Group;
  /** Turned to the yaw: the capsule or the model, and the ground arrow. */
  turn: THREE.Group;
  body: THREE.Mesh;
  arrow: THREE.Mesh;
  ring: THREE.Mesh;
  /** The aim ray, from the head along the aim. */
  ray: THREE.Mesh;
  rayMat: THREE.MeshBasicMaterial;
  label: CSS2DObject;
  el: HTMLDivElement;
  /** The weapon's name in the label. */
  gun: HTMLElement;
  colour: string;
  name: string;
  /** The model in use and the glb it came from; null while it's a capsule. */
  biped: BipedInstance | null;
  bipedUrl: string | null;
  weapon: WeaponInstance | null;
  weaponUrl: string | null;
  weaponName: string | null;
  /** An AI-controlled biped: no name label or aim ray unless asked for, and its model keeps its own colours. */
  actor: boolean;
  /** What they are doing, for the model's clips (or the capsule's squat). */
  anim: BipedAnim;
  meter: SpeedMeter;
  /** The last one-shot started on the model. */
  shotId: number;
  /** When they were first seen dead; null while alive. */
  diedAt: number | null;
}

/** A vehicle: its model, or a box of about its size; darker when nobody is in it. */
interface VehicleObj {
  group: THREE.Group;
  box: THREE.Mesh;
  boxMat: THREE.MeshStandardMaterial;
  model: THREE.Object3D | null;
  /** What `model` was made from: the glb and the colour washed over it. */
  key: string;
  height: number;
  kind: string;
}

/** One shot on screen: a tracer and its muzzle flash. */
interface TracerObj { group: THREE.Group; beam: THREE.Mesh; flash: THREE.Mesh; beamMat: THREE.MeshBasicMaterial; flashMat: THREE.MeshBasicMaterial }

const AXIS_X = new THREE.Vector3(1, 0, 0);
/** The cut plane's height when the cut is off: above anything. */
const CUT_OFF = 1e9;
const FIRING = new THREE.Color(0xfff0b8);

interface MarkObj { group: THREE.Group; mat: THREE.MeshBasicMaterial }

export class ViewerScene {
  readonly renderer: THREE.WebGLRenderer;
  private labels: CSS2DRenderer;
  private scene = new THREE.Scene();
  private camera = new THREE.PerspectiveCamera(42, 1, 0.1, 5000);
  /** The near plane when the camera is far out; it shrinks as the camera
   *  comes in (fitNear) so close-ups are not clipped. */
  private farNear = 0.1;
  private controls: OrbitControls;
  private sun: THREE.DirectionalLight;
  private composer: EffectComposer | null = null;
  private gtao: GTAOPass | null = null;
  private aoWanted = true;
  private map: THREE.Object3D | null = null;
  private ground: THREE.Mesh;
  private grid: THREE.Object3D | null = null;
  private gridKey = "";
  private players = new Map<number, PlayerObj>();
  private shown = new Map<number, SmoothPlayer>();
  private vehicles = new Map<string, VehicleObj>();
  private shownVehicles = new Map<string, SampledVehicle>();
  private actorNames = false;
  private m4 = new THREE.Matrix4();
  private geoBox = new THREE.BoxGeometry(1, 1, 1).translate(0, 0.5, 0);
  private marks: MarkObj[] = [];
  private library: ModelLibrary;
  private models: { root: string; table: PlayerModels } | null = null;
  private shots = new ShotQueue();
  private feed = new EventFeed();
  private effects: Effects3D;
  // A campaign level's zones (`bsp_<n>` nodes) and far scenery, and which are shown.
  private zones: Array<{ obj: THREE.Object3D; name: string; cinematic: boolean }> = [];
  private bsp: number | null = null;
  private allZones = false;
  // The level's machines by node name, the frames' machines eased, and
  // stand-ins for the ones the glb has no node for.
  private machineNodes = new Map<string, MachineNode>();
  private shownMachines = new Map<string, SampledMachine>();
  private looseMachines = new Map<string, VehicleObj>();
  private features: MapFeatures | null = null;
  private featureGroup: THREE.Group | null = null;
  private featuresOn = true;
  private backdrop = false;
  private tracers = new Map<number, TracerObj>();
  private spare: TracerObj[] = [];
  // Seeing into the level: a clipping plane that drops everything above a
  // height, and a cone from the camera to the player in focus that level
  // geometry is cut out of. Both live in the map's materials (one plane and
  // one set of uniforms shared by all of them), so they cost nothing per mesh.
  private cutPlane = new THREE.Plane(new THREE.Vector3(0, -1, 0), CUT_OFF);
  private cut = { on: false, height: 0 };
  private cutShown: number | null = null;
  private seeOn = true;
  private see = { uSeeTarget: { value: new THREE.Vector3() }, uSeeRadius: { value: 0 }, uSeeFloor: { value: 0 } };
  private patched = new WeakSet<THREE.Material>();
  private v0 = new THREE.Vector3();
  private v1 = new THREE.Vector3();
  private v2 = new THREE.Vector3();
  private q0 = new THREE.Quaternion();
  private bounds: Box3 | null = null;
  private hasMap = false;
  private scale = 1;
  private mode: ViewMode = "orbit";
  private followIndex: number | null = null;
  private tween: { t0: number; ms: number; p0: THREE.Vector3; p1: THREE.Vector3; q0: THREE.Vector3; q1: THREE.Vector3 } | null = null;
  private lastNow = 0;
  private lowZ = Infinity;
  private dirty = true;
  private slow = 0;
  private w = 1; private h = 1;
  private disposed = false;
  private geoBody: THREE.BufferGeometry;
  private geoArrow: THREE.BufferGeometry;
  private geoRing: THREE.BufferGeometry;
  private geoMark: THREE.BufferGeometry;
  private geoRay: THREE.BufferGeometry;
  private geoBeam: THREE.BufferGeometry;
  private geoFlash: THREE.BufferGeometry;

  /** Throws when the browser can't give a WebGL context. */
  constructor(host: HTMLElement, private opts: SceneOptions) {
    const renderer = new THREE.WebGLRenderer({ antialias: true, powerPreference: "high-performance" });
    this.renderer = renderer;
    renderer.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    renderer.toneMapping = THREE.ACESFilmicToneMapping;
    renderer.toneMappingExposure = 1.0;
    renderer.shadowMap.enabled = true;
    renderer.shadowMap.type = THREE.PCFShadowMap;
    renderer.localClippingEnabled = true;
    renderer.domElement.className = "v3-canvas";
    host.appendChild(renderer.domElement);

    this.labels = new CSS2DRenderer();
    this.labels.domElement.className = "v3-labels";
    host.appendChild(this.labels.domElement);

    const scene = this.scene;
    scene.fog = new THREE.FogExp2(HAZE, 0.002);
    scene.background = skyTexture();
    const pmrem = new THREE.PMREMGenerator(renderer);
    scene.environment = pmrem.fromScene(new RoomEnvironment(), 0.04).texture;
    scene.environmentIntensity = 0.22;
    pmrem.dispose();

    scene.add(new THREE.HemisphereLight(0xcfe2ff, 0x6b5a48, 0.6));
    const sun = new THREE.DirectionalLight(0xffe9c8, 2.4);
    sun.castShadow = true;
    const size = Math.min(2048, renderer.capabilities.maxTextureSize);
    sun.shadow.mapSize.set(size, size);
    sun.shadow.radius = 3;
    scene.add(sun, sun.target);
    this.sun = sun;
    const fill = new THREE.DirectionalLight(0x9fc0ff, 0.25);
    fill.position.set(...gameToGl(-SUN_GAME[0], -SUN_GAME[1], 0.4));
    scene.add(fill);

    // A soft ground under everything: it catches the shadows of floating
    // floors and gives the fog something to settle on.
    this.ground = new THREE.Mesh(
      new THREE.CircleGeometry(1, 64).rotateX(-Math.PI / 2),
      new THREE.MeshStandardMaterial({ color: 0x3f505d, roughness: 1, metalness: 0 }),
    );
    this.ground.receiveShadow = true;
    scene.add(this.ground);

    this.controls = new OrbitControls(this.camera, renderer.domElement);
    this.controls.enableDamping = !opts.reducedMotion;
    this.controls.dampingFactor = 0.09;
    this.controls.zoomToCursor = true;
    this.controls.maxPolarAngle = Math.PI * 0.495;
    this.controls.addEventListener("change", () => { this.fitNear(); this.dirty = true; });
    this.controls.addEventListener("start", () => { this.tween = null; });

    this.geoBody = new THREE.CapsuleGeometry(0.22, 0.42, 6, 16).translate(0, 0.43, 0);
    this.geoArrow = arrowGeometry();
    this.geoRing = new THREE.RingGeometry(0.34, 0.44, 32).rotateX(-Math.PI / 2);
    this.geoMark = markGeometry();
    // Along +x from the origin, one unit long: a tapering ray, a tracer's
    // beam, and a spiky flash around the origin.
    this.geoRay = new THREE.CylinderGeometry(0.12, 1, 1, 6, 1, true).rotateZ(-Math.PI / 2).translate(0.5, 0, 0);
    this.geoBeam = new THREE.CylinderGeometry(0.35, 1, 1, 5, 1, true).rotateZ(-Math.PI / 2).translate(0.5, 0, 0);
    this.geoFlash = new THREE.OctahedronGeometry(1, 0).scale(1.7, 0.75, 0.75).translate(0.9, 0, 0);
    this.library = new ModelLibrary(() => { this.dirty = true; });
    this.effects = new Effects3D(scene, this.camera);

    this.setBounds({ min: [-20, -20, 0], max: [20, 20, 4] }, false);
    this.frameCamera(true);
  }

  // ---- map ----

  /** Loads a map's glb. Resolves false (and leaves the grid floor) when it
   *  can't be loaded. */
  async loadMap(url: string): Promise<boolean> {
    try {
      const gltf = await new GLTFLoader().loadAsync(url);
      if (this.disposed) return false;
      const root = gltf.scene;
      // Whatever the glb's nodes are called (`structure`, several `bsp_<n>`,
      // `machines` and its children), every mesh is found by walking it once.
      // The camera and the lights fit the playable level: not far scenery,
      // nor a zone only a cutscene uses.
      const assetExtras = (gltf.asset as { extras?: unknown } | undefined)?.extras;
      const box = new THREE.Box3();
      const walk = (o: THREE.Object3D, names: string[], aside = false) => {
        const path = o.name ? [...names, o.name] : names;
        aside ||= isBackdrop(o.name) || (bspNumber(o.name) !== null && isCinematic(o.userData, o.name, assetExtras));
        const role = meshRole(path);
        if (role === "hidden") { o.visible = false; return; }
        const mesh = o as THREE.Mesh;
        if (mesh.isMesh) {
          const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
          let solid = true;
          for (const m of mats) {
            // Only faces turned to the sun cast: a level seen from above has its
            // ceilings culled, so they shouldn't black out the rooms below either.
            m.shadowSide = THREE.FrontSide;
            if (m.transparent) solid = false;
            // The structure faces into the playable space: with its back faces
            // culled it can be seen into from outside, whatever the glb says.
            // Machines are ordinary models and keep the side they came with,
            // and so does anything cut out by alpha (a grate reads from both sides).
            if (role === "level" && !m.transparent && !m.alphaTest) m.side = THREE.FrontSide;
            this.cutaway(m);
          }
          mesh.castShadow = solid;
          mesh.receiveShadow = true;
          if (!aside) box.expandByObject(mesh);
        }
        for (const c of o.children) walk(c, path, aside);
      };
      root.updateWorldMatrix(true, true);
      walk(root, []);
      if (box.isEmpty()) box.setFromObject(root);
      if (box.isEmpty()) return false;
      this.clearMap();
      // Zones and far scenery, wherever in the tree the glb keeps them.
      root.traverse((o) => {
        if (bspNumber(o.name) === null && !isBackdrop(o.name)) return;
        this.zones.push({ obj: o, name: o.name, cinematic: isCinematic(o.userData, o.name, assetExtras) });
      });
      this.applyZones();
      // The level's machines: each child of `machines` can be moved by the frames.
      root.traverse((o) => {
        if (o.name !== "machines") return;
        for (const c of o.children) if (c.name && !this.machineNodes.has(c.name)) this.machineNodes.set(c.name, machineNode(c));
      });
      this.features = parseFeatures(assetExtras);
      this.map = root;
      this.scene.add(root);
      this.hasMap = true;
      if (this.grid) { this.scene.remove(this.grid); this.grid = null; this.gridKey = ""; }
      // glTF (x, y, z) is game (x, -z, y).
      this.setBounds({ min: [box.min.x, -box.max.z, box.min.y], max: [box.max.x, -box.min.z, box.max.y] }, true);
      this.showFeatures();
      this.frameCamera(true);
      return true;
    } catch {
      return false;
    }
  }

  /** Builds (or drops) the level's teleporters, links and lifts. */
  private showFeatures() {
    if (this.featureGroup) { this.scene.remove(this.featureGroup); disposeTree(this.featureGroup); this.featureGroup = null; }
    if (this.features && this.featuresOn) {
      this.featureGroup = buildFeatures(this.features, Math.max(1, this.scale * 0.8));
      this.scene.add(this.featureGroup);
    }
    this.dirty = true;
  }

  /** Whether the level has teleporters or lifts to show. */
  hasFeatures(): boolean { return !!this.features; }

  setFeatures(on: boolean) {
    this.featuresOn = on;
    this.showFeatures();
  }

  /** A machine the level's glb has no node for: its kind's model when the
   *  server has one, else a box. */
  private looseMachine(m: SampledMachine): VehicleObj {
    let o = this.looseMachines.get(m.id);
    if (!o) {
      const boxMat = new THREE.MeshStandardMaterial({ color: 0x9a7b4f, roughness: 0.7, metalness: 0.2 });
      const box = new THREE.Mesh(this.geoBox, boxMat);
      box.castShadow = true;
      box.scale.set(2.4, 1.2, 2.4);
      const group = new THREE.Group();
      group.add(box);
      this.scene.add(group);
      o = { group, box, boxMat, model: null, key: "", height: 1.2, kind: m.kind };
      this.looseMachines.set(m.id, o);
    }
    const mod = this.models, file = mod?.table.objects[m.kind];
    const url = mod && file ? modelUrl(mod.root, file) : null;
    const template = url ? this.library.vehicle(url) : null;
    const key = template ? url! : "box";
    if (key !== o.key) {
      if (o.model) o.group.remove(o.model);
      o.model = template ? this.library.instantiateVehicle(template, null) : null;
      if (o.model) o.group.add(o.model);
      o.box.visible = !o.model;
      o.key = key;
      this.dirty = true;
    }
    return o;
  }

  /** Moves the level's machines to where the frames have them. */
  private updateMachines(list: readonly SampledMachine[], dt: number, snap: boolean) {
    smoothVehicles(this.shownMachines, list, dt, { snap });
    for (const m of this.shownMachines.values()) {
      machineMatrix(m, this.m4);
      const node = m.node ? this.machineNodes.get(m.node) ?? this.machineNodes.get(THREE.PropertyBinding.sanitizeNodeName(m.node)) : undefined;
      if (node) {
        if (placeMachine(node, this.m4)) this.dirty = true;
        continue;
      }
      // (A node the glb doesn't have yet, while the map loads, is a stand-in for now.)
      const o = this.looseMachine(m), g = o.group;
      this.m4.decompose(this.v0, this.q0, this.v1);
      if (!g.position.equals(this.v0) || !g.quaternion.equals(this.q0)) this.dirty = true;
      g.position.copy(this.v0);
      g.quaternion.copy(this.q0);
    }
    for (const [id, o] of this.looseMachines) {
      const m = this.shownMachines.get(id);
      if (m && !(m.node && (this.machineNodes.has(m.node) || this.machineNodes.has(THREE.PropertyBinding.sanitizeNodeName(m.node))))) continue;
      this.scene.remove(o.group);
      o.boxMat.dispose();
      this.looseMachines.delete(id);
      this.dirty = true;
    }
  }

  /** Shows the zone the game has loaded (every playable one when the
   *  frames don't say, or when asked for all), and the backdrop if wanted. */
  private applyZones() {
    const show = zoneVisibility(this.zones, this.allZones ? null : this.bsp, { backdrop: this.backdrop });
    this.zones.forEach((z, i) => {
      if (z.obj.visible !== show[i]) { z.obj.visible = show[i]!; this.dirty = true; }
    });
  }

  /** What the level is split into, for the toolbar: how many zones, and whether it has a backdrop. */
  zoneInfo(): { zones: number; backdrop: boolean; active: number | null } {
    return { zones: this.zones.filter((z) => bspNumber(z.name) !== null).length, backdrop: this.zones.some((z) => isBackdrop(z.name)), active: this.bsp };
  }

  /** Every playable zone at once, instead of only the loaded one. */
  setAllZones(on: boolean) { this.allZones = on; this.applyZones(); }

  /** The far scenery around the playable space: hidden unless asked for. */
  setBackdrop(on: boolean) { this.backdrop = on; this.applyZones(); }

  /** Gives a level material the height cut and the see-through. */
  /** Near plane from the camera's distance to what it orbits: small up
   *  close, back to the map's own value further out, for depth precision. */
  private fitNear() {
    const d = this.camera.position.distanceTo(this.controls.target);
    const near = Math.min(this.farNear, Math.max(0.01, d / 150));
    if (Math.abs(near - this.camera.near) > near * 0.05) {
      this.camera.near = near;
      this.camera.updateProjectionMatrix();
    }
  }

  private cutaway(m: THREE.Material) {
    if (this.patched.has(m)) return;
    this.patched.add(m);
    m.clippingPlanes = [this.cutPlane];
    m.clipShadows = true;
    const before = m.onBeforeCompile, key = m.customProgramCacheKey;
    m.onBeforeCompile = (shader, renderer) => {
      before.call(m, shader, renderer);
      const out = patchSeeThrough(shader.vertexShader, shader.fragmentShader);
      if (!out) return;
      shader.vertexShader = out.vertex;
      shader.fragmentShader = out.fragment;
      Object.assign(shader.uniforms, this.see);
    };
    m.customProgramCacheKey = () => key.call(m) + "|see";
    m.needsUpdate = true;
  }

  /** The height cut: nothing of the level above `height` (game z) is
   *  drawn. While following a player it keeps to just above them instead. */
  setCut(on: boolean, height: number) {
    this.cut = { on, height };
    this.dirty = true;
  }

  /** The heights the cut can be put at (game z), for the slider. */
  cutRange(): [number, number] {
    const b = this.bounds;
    return b ? cutRange(b.min[2], b.max[2]) : [0, 1];
  }

  /** The see-through: level geometry between the camera and the player in
   *  focus (followed, else highlighted) is cut away. */
  setSeeThrough(on: boolean) {
    this.seeOn = on;
    this.dirty = true;
  }

  /** Where the cut is drawn now, or null: for the UI. */
  cutNow(): number | null { return this.cutShown; }

  private updateCutaway(dt: number, highlight: number | null) {
    const followed = this.followIndex !== null ? this.shown.get(this.followIndex) : undefined;
    const want = cutHeight(this.cut, this.mode === "follow" && followed?.alive ? followed.z : null);
    // Following, the cut eases after the player, so a jump doesn't flicker the ceiling.
    let h = want;
    if (h !== null && this.cutShown !== null && this.mode === "follow" && !this.opts.reducedMotion) {
      h = this.cutShown + (h - this.cutShown) * (1 - Math.exp(-5 * dt / 1000));
      if (Math.abs(h - want!) < 0.005) h = want;
    }
    if (h !== this.cutShown) this.dirty = true;
    this.cutShown = h;
    this.cutPlane.constant = h === null ? CUT_OFF : h;

    const focus = this.followIndex ?? highlight;
    const s = this.seeOn && focus !== null ? this.shown.get(focus) : undefined;
    const p = s?.alive ? this.players.get(focus!) : undefined;
    const u = this.see;
    if (s && p) {
      const k = p.turn.scale.x, tall = p.biped ? p.biped.height * k : 0.86 * k;
      const [x, y, z] = gameToGl(s.x, s.y, s.z);
      const r = SEE_RADIUS * Math.max(1, k);
      if (u.uSeeRadius.value !== r || u.uSeeTarget.value.x !== x || u.uSeeTarget.value.y !== y + tall * 0.6 || u.uSeeTarget.value.z !== z) this.dirty = true;
      u.uSeeTarget.value.set(x, y + tall * 0.6, z);
      u.uSeeRadius.value = r;
      u.uSeeFloor.value = y + 0.06 * k;
    } else {
      if (u.uSeeRadius.value !== 0) this.dirty = true;
      u.uSeeRadius.value = 0;
    }
  }

  private clearMap() {
    if (!this.map) return;
    this.scene.remove(this.map);
    this.map.traverse((o) => {
      const mesh = o as THREE.Mesh;
      if (!mesh.isMesh) return;
      mesh.geometry.dispose();
      for (const m of Array.isArray(mesh.material) ? mesh.material : [mesh.material]) m.dispose();
    });
    this.map = null;
    this.zones = [];
    this.machineNodes.clear();
    this.features = null;
    this.showFeatures();
    this.hasMap = false;
  }

  /** Sizes the lights, shadows, fog, ground and camera limits to a box
   *  (game coordinates). */
  private setBounds(b: Box3, real: boolean) {
    this.bounds = b;
    const fit = cameraFit(b, this.camera.fov, this.camera.aspect);
    const r = fit.radius;
    this.scale = markerScale(b);
    const c = new THREE.Vector3(...gameToGl(...fit.centre));
    const sunDir = new THREE.Vector3(...gameToGl(...SUN_GAME)).normalize();
    this.sun.position.copy(c).addScaledVector(sunDir, r * 2.2);
    this.sun.target.position.copy(c);
    const cam = this.sun.shadow.camera;
    cam.left = -r * 1.15; cam.right = r * 1.15; cam.top = r * 1.15; cam.bottom = -r * 1.15;
    cam.near = r * 0.1; cam.far = r * 4.5;
    cam.updateProjectionMatrix();
    const texel = (r * 2.3) / this.sun.shadow.mapSize.x;
    this.sun.shadow.bias = -0.0004;
    this.sun.shadow.normalBias = texel * 1.6;
    (this.scene.fog as THREE.FogExp2).density = 0.11 / r;
    // Without a map the floor is the players' lowest point; with one, just under it.
    const drop = real ? Math.max(0.02 * r, 0.3) : 0.02 * this.scale;
    this.ground.position.set(c.x, b.min[2] - drop, c.z);
    this.ground.scale.setScalar(r * 14);
    this.farNear = Math.max(0.05, r / 400);
    this.camera.far = r * 60;
    this.fitNear();
    // Close enough to look a player in the face, on any size of map
    // (campaign levels are hundreds of times larger than a player).
    this.controls.minDistance = Math.max(0.35, this.scale * 0.6);
    this.controls.maxDistance = r * 6;
    this.gtao?.updateGtaoMaterial({ radius: this.scale * 1.6, distanceExponent: 1.2, thickness: 1.2, scale: 1.15, samples: 12 });
    this.dirty = true;
  }

  /** With no map: a grid floor fitted to where players have been. */
  private fitGrid(seen: MapFrame["seen"]) {
    if (this.hasMap || !seen || !isFinite(seen.minX)) return;
    const z = isFinite(this.lowZ) ? this.lowZ : 0;
    const w = Math.max(30, seen.maxX - seen.minX) * 1.2, d = Math.max(30, seen.maxY - seen.minY) * 1.2;
    const side = Math.max(w, d);
    const cx = (seen.minX + seen.maxX) / 2, cy = (seen.minY + seen.maxY) / 2;
    // Only rebuild when the fit has changed by a fair step.
    const step = niceStep(side / 12);
    const half = Math.ceil(side / 2 / step) * step;
    const key = [Math.round(cx / step), Math.round(cy / step), half, Math.round(z * 2)].join("|");
    if (key === this.gridKey) return;
    const first = !this.gridKey;
    this.gridKey = key;
    if (this.grid) { this.scene.remove(this.grid); disposeTree(this.grid); }
    const g = new THREE.Group();
    const ox = Math.round(cx / step) * step, oy = Math.round(cy / step) * step;
    const lines = new THREE.GridHelper(half * 2, Math.round((half * 2) / step), 0xb9d2e4, 0x86a3b8);
    const lm = lines.material as THREE.LineBasicMaterial;
    lm.transparent = true; lm.opacity = 0.55;
    lines.position.y = 0.02;
    const floor = new THREE.Mesh(
      new THREE.PlaneGeometry(half * 2, half * 2).rotateX(-Math.PI / 2),
      new THREE.MeshStandardMaterial({ color: 0x607786, roughness: 0.95, metalness: 0 }),
    );
    floor.receiveShadow = true;
    g.add(floor, lines);
    g.position.set(...gameToGl(ox, oy, z));
    this.grid = g;
    this.scene.add(g);
    this.setBounds({ min: [ox - half, oy - half, z], max: [ox + half, oy + half, z + this.scale * 2] }, false);
    if (first) this.frameCamera(true);
  }

  // ---- camera ----

  private frameCamera(now: boolean) {
    if (!this.bounds) return;
    const fit = cameraFit(this.bounds, this.camera.fov, this.camera.aspect);
    const target = new THREE.Vector3(...gameToGl(...fit.centre));
    const pos = this.mode === "top"
      ? target.clone().add(new THREE.Vector3(0, fit.topDistance, 0.001))
      : target.clone().add(new THREE.Vector3(...gameToGl(...fit.offset)));
    this.moveCamera(pos, target, now);
  }

  private moveCamera(pos: THREE.Vector3, target: THREE.Vector3, now: boolean) {
    if (now || this.opts.reducedMotion) {
      this.tween = null;
      this.camera.position.copy(pos);
      this.controls.target.copy(target);
      this.controls.update();
    } else {
      this.tween = { t0: performance.now(), ms: 650, p0: this.camera.position.clone(), p1: pos, q0: this.controls.target.clone(), q1: target };
    }
    this.dirty = true;
  }

  setMode(mode: ViewMode, follow: number | null = null) {
    const was = this.mode;
    this.mode = mode;
    this.followIndex = mode === "follow" ? follow : null;
    const top = mode === "top";
    this.controls.enableRotate = !top;
    this.controls.screenSpacePanning = true;
    this.controls.mouseButtons.LEFT = top ? THREE.MOUSE.PAN : THREE.MOUSE.ROTATE;
    this.controls.touches.ONE = top ? THREE.TOUCH.PAN : THREE.TOUCH.ROTATE;
    this.controls.enablePan = mode !== "follow";
    if (mode === "follow") {
      const s = follow !== null ? this.shown.get(follow) : undefined;
      if (s && this.bounds) {
        // Come in close behind the orbit the viewer already chose.
        const target = new THREE.Vector3(...gameToGl(s.x, s.y, s.z + this.focusHeight(follow)));
        const dir = this.camera.position.clone().sub(this.controls.target);
        if (was === "top" || dir.y / dir.length() > 0.9) dir.set(0.6, 0.55, 0.6);
        // Close to the player, however big the level is.
        const want = followDistance(dir.length(), this.scale, !!this.players.get(follow!)?.biped, this.controls.minDistance);
        this.moveCamera(target.clone().addScaledVector(dir.normalize(), want), target, false);
      }
    } else if (top || was === "top") {
      this.frameCamera(false);
    }
    this.dirty = true;
  }

  /** How far above a player's feet the follow camera looks: the chest of
   *  their model as it is drawn now, or of a capsule. */
  private focusHeight(index: number | null): number {
    const p = index !== null ? this.players.get(index) : undefined;
    return p?.biped ? p.biped.height * p.turn.scale.x * 0.6 : this.scale * 0.5;
  }

  /** Back to the whole map, in the current mode. */
  reset() { this.frameCamera(false); }

  // ---- players ----

  /** The server's player models (the art manifest's `models` table and
   *  where its files are), or null for capsules. */
  setModels(root: string | null, table: PlayerModels | null | undefined) {
    this.models = root && table ? { root, table } : null;
    this.dirty = true;
  }

  private removePlayer(p: PlayerObj) {
    this.scene.remove(p.group);
    p.label.removeFromParent();
    p.el.remove();
    // The model's parts belong to the library; the rest is this player's.
    if (p.biped) { p.biped.animator?.dispose(); p.turn.remove(p.biped.object); }
    disposeMaterials(p.group);
  }

  /** Whether AI-controlled bipeds get name labels (their species). */
  setActorNames(on: boolean) {
    this.actorNames = on;
    this.dirty = true;
  }

  /** A vehicle's object, made (or remade) for its kind and its player's colour. */
  private vehicle(v: SampledVehicle, colour: string | null): VehicleObj {
    let o = this.vehicles.get(v.id);
    if (!o) {
      const boxMat = new THREE.MeshStandardMaterial({ color: 0x8a9199, roughness: 0.6, metalness: 0.3 });
      const box = new THREE.Mesh(this.geoBox, boxMat);
      box.castShadow = true;
      const group = new THREE.Group();
      group.add(box);
      this.scene.add(group);
      o = { group, box, boxMat, model: null, key: "", height: 1, kind: "" };
      this.vehicles.set(v.id, o);
    }
    const m = this.models;
    const file = m ? vehicleFile(m.table, v.kind) : null;
    const url = m && file ? modelUrl(m.root, file) : null;
    const template = url ? this.library.vehicle(url) : null;
    const empty = !v.occupied;
    const key = (template ? url : "box:" + v.kind) + "|" + (empty ? "empty" : colour ?? "");
    if (key !== o.key) {
      if (o.model) o.group.remove(o.model);
      o.model = template ? this.library.instantiateVehicle(template, empty ? null : colour, empty) : null;
      if (o.model) o.group.add(o.model);
      o.box.visible = !o.model;
      const [len, wid, tall] = vehicleBox(v.kind);
      o.box.scale.set(len, tall, wid);
      // A box takes a little more of the colour than a model: it has nothing else to say.
      // One nobody is in is darker: there, but not part of the fight.
      o.boxMat.color.set(empty ? 0x4d545b : 0x8a9199);
      if (colour && !empty) o.boxMat.color.lerp(new THREE.Color(colour), 0.45);
      o.height = template ? template.height : tall;
      o.key = key;
      o.kind = v.kind;
      this.dirty = true;
    }
    return o;
  }

  private player(index: number, info: RosterEntry | undefined, actor = false): PlayerObj {
    let p = this.players.get(index);
    const colour = info?.colour ?? "#999", name = info?.name ?? `Player ${index + 1}`;
    if (p && p.colour === colour && p.name === name) return p;
    if (p) this.removePlayer(p);
    const c = new THREE.Color(colour);
    const body = new THREE.Mesh(this.geoBody, new THREE.MeshStandardMaterial({ color: c, roughness: 0.5, metalness: 0.1, emissive: c, emissiveIntensity: 0.18 }));
    body.castShadow = true;
    const flat = new THREE.MeshBasicMaterial({ color: c, transparent: true, opacity: 0.95, fog: false });
    const arrow = new THREE.Mesh(this.geoArrow, flat);
    const ring = new THREE.Mesh(this.geoRing, flat);
    arrow.position.y = ring.position.y = 0.03;
    arrow.renderOrder = ring.renderOrder = 2;
    const rayMat = new THREE.MeshBasicMaterial({ color: c, transparent: true, opacity: 0.5, depthWrite: false, fog: false, side: THREE.DoubleSide });
    const ray = new THREE.Mesh(this.geoRay, rayMat);
    ray.renderOrder = 2;
    const el = document.createElement("div");
    el.className = "v3-label";
    const pip = document.createElement("i");
    pip.style.background = colour;
    const gun = document.createElement("small");
    el.append(pip, document.createTextNode(name), gun);
    if (!actor) el.addEventListener("pointerdown", (e) => { e.stopPropagation(); this.opts.onPick?.(index); });
    const label = new CSS2DObject(el);
    label.center.set(0.5, 1);
    const turn = new THREE.Group();
    turn.add(body, arrow);
    const group = new THREE.Group();
    group.add(turn, ring, ray, label);
    this.scene.add(group);
    if (actor) el.classList.add("v3-label--ai");
    p = { group, turn, body, arrow, ring, ray, rayMat, label, el, gun, colour, name, biped: null, bipedUrl: null, weapon: null, weaponUrl: null, weaponName: null, actor, anim: new BipedAnim(), meter: new SpeedMeter(), shotId: 0, diedAt: -Infinity };
    this.players.set(index, p);
    return p;
  }

  /** Gives a player the model their species and weapon call for, as far
   *  as those have loaded; a capsule until then, and when there are none. */
  private dress(p: PlayerObj, s: SmoothPlayer) {
    const m = this.models;
    const file = m ? bipedFile(m.table, s.species) : null;
    const url = m && file ? modelUrl(m.root, file) : null;
    const template = url ? this.library.biped(url) : null;
    const have = template ? url : null;
    if (have !== p.bipedUrl) {
      if (p.biped) { p.biped.animator?.dispose(); p.turn.remove(p.biped.object); }
      p.biped = template ? this.library.instantiateBiped(template, p.actor ? null : p.colour) : null;
      p.bipedUrl = have;
      p.weapon = null;
      p.weaponUrl = null;
      if (p.biped) p.turn.add(p.biped.object);
      // The model says which way it faces; the capsule needs its arrow.
      p.body.visible = p.arrow.visible = !p.biped;
      this.dirty = true;
    }
    if (p.biped) {
      const wfile = m ? weaponFile(m.table, s.weapon) : null;
      const wurl = m && wfile ? modelUrl(m.root, wfile) : null;
      const wt = wurl ? this.library.weapon(wurl) : null;
      const whave = wt ? wurl : null;
      if (whave !== p.weaponUrl) {
        if (p.weapon) p.biped.hand.remove(p.weapon.object);
        p.weapon = wt ? this.library.instantiateWeapon(wt) : null;
        p.weaponUrl = whave;
        if (p.weapon) p.biped.hand.add(p.weapon.object);
        this.dirty = true;
      }
    }
    if (s.weapon !== p.weaponName) {
      p.weaponName = s.weapon;
      p.gun.textContent = weaponLabel(s.weapon);
    }
  }

  /** The class of clips a player's weapon calls for. */
  private clipClass(s: SmoothPlayer): string {
    return weaponClass(this.models?.table.classes, s.weapon);
  }

  /** Starts a one-shot (firing, a throw, a melee, a reload, a swap) on a
   *  player: it lasts as long as their model's clip for it. */
  private oneShot(index: number, state: OneShot, now: number) {
    const p = this.players.get(index), s = this.shown.get(index);
    if (!p || !s || !s.alive) return;
    const an = p.biped?.animator;
    const clip = an ? clipFor(an.names, this.clipClass(s), state) : null;
    p.anim.trigger(state, now, an?.duration(clip) ?? undefined);
  }

  /** Moves a player's model through its clips for this moment; a capsule
   *  can only squat. Reduced motion leaves the model in its pose. */
  private animate(p: PlayerObj, s: SmoothPlayer, now: number, dt: number) {
    const speed = s.speed ?? p.meter.update(s.x, s.y, dt);
    const st = p.anim.update(now, { alive: s.alive, stance: s.stance, speed });
    const squat = st.loop === "crouch" ? 0.7 : 1;
    if (p.body.scale.y !== squat) { p.body.scale.y = squat; this.dirty = true; }
    const an = p.biped?.animator;
    if (!an || this.opts.reducedMotion) return;
    const cls = this.clipClass(s);
    an.setLoop(clipFor(an.names, cls, st.loop), paceScale(st.loop, speed), st.loop === "die");
    if (st.shotId !== p.shotId) {
      p.shotId = st.shotId;
      // Anything but landing is done with the arms: unless they stand still, the legs carry on with the loop.
      if (st.shot) an.playOnce(clipFor(an.names, cls, st.shot), st.shot !== "land" && st.loop !== "idle");
    }
    an.update(dt);
    this.dirty = true;
  }

  /** How long a dead player's model stays to play its dying clip (ms); 0 when it has none. */
  private dyingFor(p: PlayerObj, s: SmoothPlayer): number {
    const an = p.biped?.animator;
    if (!an || this.opts.reducedMotion) return 0;
    const d = an.duration(clipFor(an.names, this.clipClass(s), "die"));
    return d === null ? 0 : d + 700;
  }

  /** Where a player's shots leave from, in the scene: the weapon's muzzle,
   *  else the hand, else chest height just ahead of a capsule. */
  private muzzle(p: PlayerObj, k: number, out: THREE.Vector3): THREE.Vector3 {
    const from = p.weapon?.muzzle ?? p.biped?.hand;
    if (from) return from.getWorldPosition(out);
    return out.set(0.3 * k, 0.56 * k, 0).applyEuler(p.turn.rotation).add(p.group.position);
  }

  private tracer(): TracerObj {
    const old = this.spare.pop();
    if (old) return old;
    const beamMat = new THREE.MeshBasicMaterial({ color: 0xfff2c4, transparent: true, depthWrite: false, fog: false, blending: THREE.AdditiveBlending, side: THREE.DoubleSide });
    const flashMat = new THREE.MeshBasicMaterial({ color: 0xffd98a, transparent: true, depthWrite: false, fog: false, blending: THREE.AdditiveBlending });
    const beam = new THREE.Mesh(this.geoBeam, beamMat), flash = new THREE.Mesh(this.geoFlash, flashMat);
    beam.renderOrder = flash.renderOrder = 4;
    const group = new THREE.Group();
    group.add(beam, flash);
    this.scene.add(group);
    return { group, beam, flash, beamMat, flashMat };
  }

  /** Draws the shots on screen now: each is put at its player's muzzle
   *  along their aim when it appears, and stays there while it fades. */
  private drawShots(now: number) {
    const S = this.scale;
    const live = this.opts.reducedMotion ? [] : this.shots.live(now);
    const kept = new Set<number>();
    for (const shot of live) {
      const p = this.players.get(shot.index), s = this.shown.get(shot.index);
      let t = this.tracers.get(shot.id);
      if (!t) {
        if (!p || !s || !s.alive) continue;
        t = this.tracer();
        this.tracers.set(shot.id, t);
        const k = p.turn.scale.x;
        this.muzzle(p, k, t.group.position);
        t.group.quaternion.setFromUnitVectors(AXIS_X, this.v1.set(...gameToGl(...s.aim)).normalize());
        // Never thinner than a few pixels, however far the camera is.
        const far = this.camera.position.distanceTo(t.group.position);
        const width = Math.max(p.biped ? p.biped.height * 0.024 * k : 0.026 * k, far * 0.0022);
        t.beam.scale.set(Math.max(14, 9 * S), width, width);
        t.flash.scale.setScalar(Math.max(p.biped ? p.biped.height * 0.13 * k : 0.16 * k, far * 0.012));
        // A little roll, so a burst's flashes aren't one shape.
        t.flash.rotation.x = shot.id * 1.9;
        t.group.visible = true;
      }
      kept.add(shot.id);
      t.beamMat.opacity = shot.alpha * 0.85;
      t.flash.visible = shot.age < FLASH_MS;
      t.flashMat.opacity = flashAlpha(shot.age);
      this.dirty = true;
    }
    for (const [id, t] of this.tracers) {
      if (kept.has(id)) continue;
      t.group.visible = false;
      this.tracers.delete(id);
      this.spare.push(t);
      this.dirty = true;
    }
  }

  private mark(i: number): MarkObj {
    let m = this.marks[i];
    if (!m) {
      const mat = new THREE.MeshBasicMaterial({ transparent: true, depthWrite: false, fog: false });
      const group = new THREE.Group();
      const x = new THREE.Mesh(this.geoMark, mat);
      x.renderOrder = 3;
      group.add(x);
      this.scene.add(group);
      m = { group, mat };
      this.marks[i] = m;
    }
    return m;
  }

  /** Places players and marks for this moment, moves the camera, and draws
   *  if anything changed. */
  update(now: number, frame: MapFrame, roster: Map<number, RosterEntry>, highlight: number | null, seeking: boolean) {
    if (this.disposed) return;
    const dt = this.lastNow ? Math.min(250, now - this.lastNow) : 16;
    this.lastNow = now;

    for (const p of frame.players) if (p.alive && p.z < this.lowZ) this.lowZ = p.z;
    this.fitGrid(frame.seen);

    const snap = seeking || this.opts.reducedMotion;
    smoothPlayers(this.shown, frame.actors?.length ? [...frame.players, ...frame.actors] : frame.players, dt, { snap });
    const fired = this.shots.observe(frame.players, now, { quiet: seeking });
    if (frame.actors?.length) fired.push(...this.shots.observe(frame.actors, now, { quiet: seeking }));
    for (const index of fired) this.oneShot(index, "fire", now);
    for (const e of this.feed.observe(frame, now, { quiet: seeking })) {
      const shot = actionShot(e.action);
      if (shot) this.oneShot(e.index, shot, now);
    }
    const bsp = frame.bsp ?? null;
    if (bsp !== this.bsp) { this.bsp = bsp; this.applyZones(); }
    const S = this.scale;
    const still = this.opts.reducedMotion;

    this.updateMachines(frame.machines ?? [], dt, snap);

    // Vehicles first: whoever is in one is drawn as the vehicle.
    smoothVehicles(this.shownVehicles, frame.vehicles ?? [], dt, { snap });
    for (const [id, v] of this.shownVehicles) {
      let colour: string | null = null;
      for (const [index, s] of this.shown) if (s.vehicle === id && !s.actor && s.alive) { colour = roster.get(index)?.colour ?? null; break; }
      const o = this.vehicle(v, colour);
      const [x, y, z] = gameToGl(v.x, v.y, v.z);
      const k = o.model ? modelScale(this.camera.position.distanceTo(this.v0.set(x, y, z)), o.height, S) : S;
      const b = vehicleBasis(v.forward, v.up);
      this.m4.makeBasis(this.v0.set(...b.x), this.v1.set(...b.y), this.v2.set(...b.z));
      const g = o.group, q = g.quaternion, qx = q.x, qy = q.y, qz = q.z;
      q.setFromRotationMatrix(this.m4);
      if (g.position.x !== x || g.position.y !== y || g.position.z !== z || g.scale.x !== k || q.x !== qx || q.y !== qy || q.z !== qz) this.dirty = true;
      g.position.set(x, y, z);
      g.scale.setScalar(k);
    }
    for (const [id, o] of this.vehicles) {
      if (this.shownVehicles.has(id)) continue;
      this.scene.remove(o.group);
      o.boxMat.dispose();
      this.vehicles.delete(id);
      this.dirty = true;
    }

    for (const [index, s] of this.shown) {
      const info = s.actor ? { name: weaponLabel(s.species) || "AI", colour: actorColour(s.actor.team) } : roster.get(index);
      const p = this.player(index, info, !!s.actor);
      const ride = s.vehicle ? this.vehicles.get(s.vehicle) : undefined;
      if (ride && s.alive) {
        // In a vehicle: the vehicle stands for them, and carries their label.
        const g = p.group;
        if (p.turn.visible || !g.visible || !g.position.equals(ride.group.position)) this.dirty = true;
        g.visible = true;
        g.position.copy(ride.group.position);
        p.turn.visible = p.ring.visible = p.ray.visible = false;
        p.turn.rotation.y = yawToGl(s.yaw);
        p.turn.scale.setScalar(ride.group.scale.x);
        p.label.visible = !p.actor || this.actorNames;
        p.label.position.y = ride.height * ride.group.scale.x + 0.25 * S;
        p.el.classList.toggle("on", highlight === index || this.followIndex === index);
        if (s.weapon !== p.weaponName) { p.weaponName = s.weapon; p.gun.textContent = weaponLabel(s.weapon); }
        continue;
      }
      this.dress(p, s);
      if (s.alive) p.diedAt = null;
      else if (p.diedAt === null) p.diedAt = seeking ? -Infinity : now;
      // A model with a dying clip stays to play it; anything else is gone at once.
      const show = s.alive || now - p.diedAt! < this.dyingFor(p, s);
      p.turn.visible = true;
      p.ring.visible = s.alive;
      p.ray.visible = s.alive && !p.actor;
      const [x, y, z] = gameToGl(s.x, s.y, s.z);
      const g = p.group, turn = p.turn;
      const big = highlight === index || this.followIndex === index;
      // A capsule (and every ring) grows with the map so it can be found. A
      // model is life size up close and grows only as the camera backs away,
      // up to the same limit, so it never shrinks to a speck on a big map.
      const k = p.biped
        ? modelScale(this.camera.position.distanceTo(this.v0.set(x, y, z)), p.biped.height, S)
        : S * (big ? 1.25 : 1) * (p.actor ? 0.8 : 1);
      const yaw = yawToGl(s.yaw);
      if (g.visible !== show || g.position.x !== x || g.position.y !== y || g.position.z !== z || turn.scale.x !== k || turn.rotation.y !== yaw) this.dirty = true;
      g.visible = show;
      p.label.visible = s.alive && (!p.actor || this.actorNames);
      g.position.set(x, y, z);
      turn.rotation.y = yaw;
      turn.scale.setScalar(k);
      // The ring fits what it is under: a model's footprint, or the capsule's.
      p.ring.scale.setScalar(p.biped ? k * p.biped.height * 1.25 * (big ? 1.25 : 1) : k);
      p.label.position.y = p.biped ? p.biped.height * k * 1.08 + 0.12 * S : 1.15 * k;
      p.el.classList.toggle("on", big);
      if (show) this.animate(p, s, now, dt);
      if (!s.alive || p.actor) continue;
      // The aim ray: from the eye point, along the eased aim. Reduced motion
      // has no flashes; the ray itself says "firing" for a moment instead.
      g.updateWorldMatrix(false, true);
      const ray = p.ray;
      if (p.biped) p.biped.head.getWorldPosition(ray.position).sub(g.position);
      else ray.position.set(0, 0.74 * k, 0);
      const ax = ray.quaternion.x, ay = ray.quaternion.y, az = ray.quaternion.z;
      ray.quaternion.setFromUnitVectors(AXIS_X, this.v0.set(...gameToGl(...s.aim)).normalize());
      if (ax !== ray.quaternion.x || ay !== ray.quaternion.y || az !== ray.quaternion.z) this.dirty = true;
      const firing = still && this.shots.firing(index, now);
      const len = (p.biped ? p.biped.height * 3 * k : 2.3 * k) * (firing ? 1.6 : 1);
      const far = this.camera.position.distanceTo(g.position);
      const thick = Math.max(p.biped ? p.biped.height * 0.042 * k : 0.036 * k, far * 0.0017) * (firing ? 1.5 : 1);
      if (ray.scale.x !== len) this.dirty = true;
      ray.scale.set(len, thick, thick);
      const opacity = firing ? 0.9 : 0.5;
      if (p.rayMat.opacity !== opacity) this.dirty = true;
      p.rayMat.color.set(firing ? FIRING : p.colour);
      p.rayMat.opacity = opacity;
    }
    for (const [index, p] of this.players) {
      if (this.shown.has(index)) continue;
      this.removePlayer(p);
      this.players.delete(index);
      this.dirty = true;
    }
    this.drawShots(now);
    if (this.effects.update(this.feed.liveHits(now), still ? [] : this.feed.liveGrenades(now), S, still)) this.dirty = true;
    this.updateCutaway(dt, highlight);

    frame.markers.forEach((m, i) => {
      const o = this.mark(i);
      const a = Math.max(0, Math.min(1, m.alpha));
      if (!o.group.visible || Math.abs(o.mat.opacity - a) > 0.004) this.dirty = true;
      o.group.visible = a > 0;
      o.group.position.set(...gameToGl(m.x, m.y, m.z + 0.06 * S));
      o.group.scale.setScalar(S * 1.3);
      o.mat.color.set(m.colour);
      o.mat.opacity = a;
    });
    for (let i = frame.markers.length; i < this.marks.length; i++) {
      if (this.marks[i]!.group.visible) this.dirty = true;
      this.marks[i]!.group.visible = false;
    }

    // Camera: a running tween, then following.
    if (this.tween) {
      const tw = this.tween;
      const u = Math.min(1, (performance.now() - tw.t0) / tw.ms), e = u * u * (3 - 2 * u);
      this.camera.position.lerpVectors(tw.p0, tw.p1, e);
      this.controls.target.lerpVectors(tw.q0, tw.q1, e);
      if (u >= 1) this.tween = null;
      this.dirty = true;
    } else if (this.mode === "follow" && this.followIndex !== null) {
      const s = this.shown.get(this.followIndex);
      if (s && s.alive) {
        const want = new THREE.Vector3(...gameToGl(s.x, s.y, s.z + this.focusHeight(this.followIndex)));
        const t = this.opts.reducedMotion ? 1 : 1 - Math.exp(-6 * dt / 1000);
        const d = want.sub(this.controls.target).multiplyScalar(t);
        if (d.lengthSq() > 1e-10) {
          this.controls.target.add(d);
          this.camera.position.add(d);
          this.dirty = true;
        }
      }
    }
    this.controls.update();
    if (!this.dirty) return;
    this.dirty = false;
    this.draw(dt);
  }

  private draw(dt: number) {
    if (this.composer && this.aoWanted) this.composer.render();
    else this.renderer.render(this.scene, this.camera);
    this.labels.render(this.scene, this.camera);
    // Ambient occlusion is the one costly step: drop it on a machine that
    // can't keep up (under ~25 frames a second for a couple of seconds),
    // rather than stutter.
    if (this.composer && this.aoWanted) {
      this.slow = dt > 40 ? this.slow + 1 : Math.max(0, this.slow - 2);
      if (this.slow > 50) { this.aoWanted = false; this.dirty = true; this.onAoDropped?.(); }
    }
  }

  onAoDropped?: () => void;

  /** Screen-space ambient occlusion on or off. Returns whether it is on
   *  (false when this machine can't do it). */
  setAo(on: boolean): boolean {
    this.aoWanted = on;
    this.slow = 0;
    if (on && !this.composer) {
      try {
        const size = this.renderer.getDrawingBufferSize(new THREE.Vector2());
        const target = new THREE.WebGLRenderTarget(size.x, size.y, { type: THREE.HalfFloatType, samples: 4 });
        const composer = new EffectComposer(this.renderer, target);
        composer.addPass(new RenderPass(this.scene, this.camera));
        const gtao = new GTAOPass(this.scene, this.camera, this.w, this.h);
        gtao.blendIntensity = 0.9;
        composer.addPass(gtao);
        composer.addPass(new OutputPass());
        composer.setPixelRatio(this.renderer.getPixelRatio());
        composer.setSize(this.w, this.h);
        // The occlusion pass draws the scene again with one material of its
        // own: it takes the cuts too, or the cut-away walls would still shade.
        this.cutaway(gtao.normalMaterial);
        this.composer = composer;
        this.gtao = gtao;
        gtao.updateGtaoMaterial({ radius: this.scale * 1.6, distanceExponent: 1.2, thickness: 1.2, scale: 1.15, samples: 12 });
      } catch {
        this.composer = null; this.gtao = null; this.aoWanted = false;
      }
    }
    this.dirty = true;
    return this.aoWanted;
  }

  resize(w: number, h: number) {
    if (w < 1 || h < 1) return;
    this.w = w; this.h = h;
    this.renderer.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
    this.renderer.setSize(w, h, false);
    this.labels.setSize(w, h);
    this.composer?.setPixelRatio(this.renderer.getPixelRatio());
    this.composer?.setSize(w, h);
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
    this.dirty = true;
  }

  /** Draws on the next update even if nothing moved. */
  invalidate() { this.dirty = true; }

  dispose() {
    this.disposed = true;
    this.clearMap();
    this.controls.dispose();
    for (const p of this.players.values()) this.removePlayer(p);
    this.players.clear();
    for (const o of this.vehicles.values()) { if (o.model) o.group.remove(o.model); o.boxMat.dispose(); }
    this.vehicles.clear();
    for (const o of this.looseMachines.values()) { if (o.model) o.group.remove(o.model); o.boxMat.dispose(); }
    this.looseMachines.clear();
    this.geoBox.dispose();
    this.effects.dispose();
    this.library.dispose();
    disposeTree(this.scene);
    this.scene.environment?.dispose();
    (this.scene.background as THREE.Texture | null)?.dispose();
    this.composer?.dispose();
    this.renderer.dispose();
    this.renderer.forceContextLoss();
    this.renderer.domElement.remove();
    this.labels.domElement.remove();
  }
}

/** Whether this browser can make a WebGL context at all. */
export function hasWebGl(): boolean {
  try {
    const c = document.createElement("canvas");
    return !!(c.getContext("webgl2") || c.getContext("webgl"));
  } catch {
    return false;
  }
}

/** The sky: a hazy horizon rising to deep blue, as a small equirect picture. */
function skyTexture(): THREE.Texture {
  const c = document.createElement("canvas");
  c.width = 8; c.height = 256;
  const ctx = c.getContext("2d")!;
  const g = ctx.createLinearGradient(0, 0, 0, 256);
  const hex = (n: number) => "#" + n.toString(16).padStart(6, "0");
  g.addColorStop(0, hex(SKY_TOP));
  g.addColorStop(0.3, "#3f6890");
  g.addColorStop(0.5, hex(SKY_HORIZON));
  g.addColorStop(0.56, hex(HAZE));
  g.addColorStop(1, "#5c7385");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 8, 256);
  const t = new THREE.CanvasTexture(c);
  t.mapping = THREE.EquirectangularReflectionMapping;
  t.colorSpace = THREE.SRGBColorSpace;
  return t;
}

/** A flat arrowhead on the ground ahead of a player, pointing along +x. */
function arrowGeometry(): THREE.BufferGeometry {
  const s = new THREE.Shape();
  s.moveTo(1.15, 0); s.lineTo(0.56, 0.3); s.lineTo(0.66, 0); s.lineTo(0.56, -0.3); s.closePath();
  // A shape lies in x/y; turn it flat so its y becomes -z.
  return new THREE.ShapeGeometry(s).rotateX(-Math.PI / 2);
}

/** A flat cross: two bars lying on the ground. */
function markGeometry(): THREE.BufferGeometry {
  const s = new THREE.Shape();
  const a = 0.5, b = 0.09;
  s.moveTo(-a, -b); s.lineTo(-b, -b); s.lineTo(-b, -a); s.lineTo(b, -a); s.lineTo(b, -b); s.lineTo(a, -b);
  s.lineTo(a, b); s.lineTo(b, b); s.lineTo(b, a); s.lineTo(-b, a); s.lineTo(-b, b); s.lineTo(-a, b); s.closePath();
  return new THREE.ShapeGeometry(s).rotateX(-Math.PI / 2).rotateY(Math.PI / 4);
}

function niceStep(raw: number): number {
  const p = 10 ** Math.floor(Math.log10(Math.max(raw, 1e-6)));
  const m = raw / p;
  return (m < 1.5 ? 1 : m < 3.5 ? 2 : m < 7.5 ? 5 : 10) * p;
}

function disposeMaterials(o: THREE.Object3D) {
  o.traverse((c) => {
    const mesh = c as THREE.Mesh;
    if (!mesh.isMesh) return;
    for (const m of Array.isArray(mesh.material) ? mesh.material : [mesh.material]) m.dispose();
  });
}

function disposeTree(o: THREE.Object3D) {
  o.traverse((c) => {
    const mesh = c as THREE.Mesh;
    if (mesh.geometry) mesh.geometry.dispose();
    const mat = mesh.material;
    if (mat) for (const m of Array.isArray(mat) ? mat : [mat]) m.dispose();
  });
}
