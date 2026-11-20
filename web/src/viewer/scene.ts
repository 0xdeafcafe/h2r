// The 3D game viewer's scene: a map's glb (or a grid floor when there is
// none) under a sun, with players as their species' model in their team or
// own colour, holding their weapon (a capsule when the server has no
// models), an aim ray from the head, name labels, muzzle flashes and
// tracers when they fire, damage drawn from attacker to victim, projectiles
// in flight, every vehicle in the world, and fading knockout marks. A
// skinned model plays its clips (standing, walking, running, crouching, in
// the air, throwing, hitting, reloading, firing) as the frames say; a
// static one glides in the pose it was exported in. What of the level is
// shown (the zone the game has loaded or one picked, far scenery, decals and
// effects, the sky and fog) is `level3d.ts`'s. Game positions (z up) are placed at glTF
// (x, z, -y), as the map geometry contract says.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { gltfLoader } from "./gltf";
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
import { MODEL_SCALE, actorColour, bipedFile, modelUrl, seatNodeFor, seatState, vehicleBasis, vehicleBox, vehicleFile, weaponFile, weaponLabel, type PlayerModels } from "./models";
import type { SampledVehicle } from "../state/frames";
import { Projectiles, lookFor, shotEnd, type Look } from "./projectiles";
import { ModelLibrary, type BipedAnimator, type BipedInstance, type WeaponInstance } from "./players3d";
import { smoothPlayers, smoothVehicles, type SmoothPlayer } from "./sync";
import { BipedAnim, HELD, MAX_TWIST, SpeedMeter, WALK_SPEED, actionShot, aimAngles, actorAim, clipFor, clipTime, gameAnim, legTwist, paceScale, sideways, weaponClass, wrapAngle, type OneShot } from "./anim";
import { GroundCache, type Floor } from "./ground";
import { FloorGrid, type FloorSource } from "./floorgrid";
import { Bodies, bodyPlace, type Place } from "./bodies";
import { EventFeed } from "../map/events";
import { Effects3D } from "./effects3d";
import { ZoneFollower, bspNumber, isBackdrop, isCinematic, layerNode, type ZonePick } from "./zones";
import { dressWater } from "./water3d";
import { LevelLook, type LevelInfo } from "./level3d";
import { dressLayer } from "./layers3d";
import { loadPlasma, type LevelPlasma } from "./plasma3d";
import { LevelAoPass } from "./aopass";
import { machineMatrix, machineNode, placeDevice, placeMachine, type MachineNode } from "./machines3d";
import { FilmWorld3D } from "./film-world3d";
import { FrameObjects } from "./objects3d";
import { buildFeatures } from "./features3d";
import { TextureSwap, deferTextures, loadFlats, mapAnisotropy, streamTextures, texturesSharing, type TexturePair } from "./textures3d";
import { ScenarioObjects, isVehicle, objectOf } from "./scenario3d";
import { parseFeatures, type MapFeatures } from "../map/features";
import type { SampledMachine } from "../state/frames";
import { easeAngle, mountedTurrets, pickTurret, type TurretAim } from "./turrets";
import { findTurrets, poseTurret, turretWant, type TurretObj } from "./turrets3d";
import { BOOST, FLY_KEYS, MOUSE_LOOK, PAD_LOOK, TOUCH_LOOK, addMoves, anglesOf, approach, keyBoost, keyMove, lookStep, looksWith, moveIsIdle, padInput, pointerLook, speedLabel, speedStep, twoFingerMove, type FlyMove } from "./flycam";
import { cameraFit, eyeView, followDistance, markerScale, orbitDirection, type Box3 } from "./camera";
import { cutHeight, cutRange, meshRole, seeTargets, takesSeeThrough, type SeeMode } from "./cutaway";
import { easeOutExpo, playableBounds, type CameraStart } from "./start";
import { CutawayMaterials } from "./cutaway3d";

/** `eyes`: first person, through the followed player's eyes along their aim. */
export type ViewMode = "orbit" | "top" | "follow" | "eyes";

export interface SceneOptions {
  reducedMotion: boolean;
  /** Called when a player's label is clicked. */
  onPick?: (index: number) => void;
  /** Players as the game draws them, without their rings, aim rays or
   *  labels (still pictures: web/banner). */
  bare?: boolean;
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
  /** A Clanker bot: its label carries a BOT mark. */
  bot: boolean;
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
  /** How far the legs are turned from the aim to follow the movement (eased, radians). */
  legs: number;
  /** The last one-shot started on the model. */
  shotId: number;
  /** Whether they were alive at the last update (null: not seen yet). */
  wasAlive: boolean | null;
  /** Where they last stood alive, for a body a sender doesn't place. */
  lastAlive: Place | null;
}

/** A dead player's model left lying (viewer/bodies.ts). */
interface BodyObj {
  group: THREE.Group;
  biped: BipedInstance;
  anim: BipedAnim;
  /** The weapon class they died holding: which dying clip. */
  cls: string;
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
  /** Its model's turrets (viewer/turrets.ts), in the glb's order. */
  turrets: TurretObj[];
  /** Its model's `seat_` nodes, by name. */
  seats: Map<string, THREE.Object3D>;
}

/** One shot on screen: a tracer and its muzzle flash. */
interface TracerObj { group: THREE.Group; beam: THREE.Mesh; flash: THREE.Mesh; beamMat: THREE.MeshBasicMaterial; flashMat: THREE.MeshBasicMaterial }

const AXIS_X = new THREE.Vector3(1, 0, 0);
/** Level pictures fetched at once. They are small (a few to 80 KB) and
 *  the art is on HTTP/2, so the wait is round trips, not bandwidth: six at
 *  a time took 12 s for a campaign level's 197. */
const PICTURES_AT_ONCE = 16;
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
  /** The floor under each biped, probed on the loaded level (ground.ts). */
  private floors: GroundCache | null = null;
  /** The level's triangles for floor probes, and the meshes they are from. */
  private floorGrid: { grid: FloorGrid; meshes: THREE.Object3D[] } | null = null;
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
  /** Bolts, tracers and rockets in flight (viewer/projectiles.ts). */
  private projectiles: Projectiles;
  private raycaster = new THREE.Raycaster();
  private feed = new EventFeed();
  private effects: Effects3D;
  // What of the level is shown (zones, far scenery, decals, effects) and
  // the sky and fog around it.
  private look: LevelLook;
  private gradient: THREE.Texture;
  // The level's machines by node name, the frames' machines eased, and
  // stand-ins for the ones the glb has no node for.
  private machineNodes = new Map<string, MachineNode>();
  private shownMachines = new Map<string, SampledMachine>();
  private looseMachines = new Map<string, VehicleObj>();
  // The level and its objects textured or in flat class colours (textures3d.ts).
  private textures = new TextureSwap();
  // Which map load is current (a newer one stops the older one's pictures),
  // and the current map's pictures, to free with it.
  private mapGen = 0;
  private mapTextures: THREE.Texture[] = [];
  /** Settles when the current map's pictures have all arrived (or stopped). */
  texturesReady: Promise<void> = Promise.resolve();
  /** The scenario's placed objects (scenario3d.ts): what frames move by
   *  `find(kind, index)` or `apply(objects)`. */
  readonly scenarioObjects = new ScenarioObjects(() => { this.dirty = true; });
  /** The frames' moved and gone scenario objects, applied to those. */
  private readonly frameObjects = new FrameObjects(this.scenarioObjects);
  private features: MapFeatures | null = null;
  private featureGroup: THREE.Group | null = null;
  private featuresOn = true;
  private tracers = new Map<number, TracerObj>();
  private spare: TracerObj[] = [];
  // The height plane and the player windows are shared by the level's
  // materials and the machines and scenery the frames draw separately.
  private cutPlane = new THREE.Plane(new THREE.Vector3(0, -1, 0), CUT_OFF);
  private cut = { on: false, height: 0 };
  private cutShown: number | null = null;
  private seeMode: SeeMode = "auto";
  private cutaway = new CutawayMaterials(this.cutPlane, this.camera.position);
  private filmWorld = new FilmWorld3D((node) => this.cutaway.attachModel(node));
  private v0 = new THREE.Vector3();
  private e0 = new THREE.Euler();
  private v1 = new THREE.Vector3();
  private v2 = new THREE.Vector3();
  private v3 = new THREE.Vector3();
  private q0 = new THREE.Quaternion();
  private bounds: Box3 | null = null;
  private frameBox: Box3 | null = null;
  private frameMap = false;
  private opening: CameraStart | null = null;
  private hasMap = false;
  private curtain = false;
  onFirstMapFrame?: () => void;
  onSurfaceProgress?: (done: number, total: number) => void;
  private drewMap = false;
  private scale = 1;
  private mode: ViewMode = "orbit";
  private followIndex: number | null = null;
  private tween: { t0: number; ms: number; ease: (t: number) => number; p0: THREE.Vector3; p1: THREE.Vector3; q0: THREE.Vector3; q1: THREE.Vector3 } | null = null;
  private lastNow = 0;
  private lowZ = Infinity;
  /** Dead players' bodies, left where they fell. */
  private bodies = new Bodies<BodyObj>();
  /** Which zone is in play (viewer/zones.ts). */
  private zone = new ZoneFollower();
  private dirty = true;
  /** The flycam while it is on (see flycam.ts); what to go back to is `saved`. */
  private fly: { yaw: number; pitch: number; vel: THREE.Vector3; keys: Set<string>; drag: boolean; moved: number; dist: number; saved: { pos: THREE.Vector3; target: THREE.Vector3 } } | null = null;
  private flySpeed = 1;
  private flyOff: (() => void) | null = null;
  /** Told when the flycam goes on or off (it also ends by itself), and its speed when that changes. */
  onFlyChange?: (on: boolean) => void;
  onFlySpeed?: (label: string) => void;
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
    this.gradient = skyTexture();
    scene.background = this.gradient;
    this.look = new LevelLook(scene, this.gradient, () => { this.dirty = true; });
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
    this.flyOff = this.listenFly(renderer.domElement);

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
    // Models reflect the scene's environment at their own strength (players3d.ts).
    this.library.setEnvironment(this.scene.environment);
    this.effects = new Effects3D(scene, this.camera);
    this.projectiles = new Projectiles(scene);
    scene.add(this.filmWorld.group);

    this.setBounds({ min: [-20, -20, 0], max: [20, 20, 4] }, false);
    this.frameCamera(true);
  }

  // ---- map ----

  setCurtain(on: boolean) { this.curtain = on; if (!on) this.dirty = true; }

  useGrid() {
    this.mapGen++;
    if (!this.hasMap) return;
    this.clearMap();
    this.setBounds({ min: [-20, -20, 0], max: [20, 20, 4] }, false);
    this.frameCamera(true);
  }

  /** Loads a map's glb. A null progress event starts decoding. */
  async loadMap(url: string, onProgress?: (event: ProgressEvent | null) => void): Promise<"ok" | "failed"> {
    const gen = ++this.mapGen;
    try {
      // The level comes first, drawn in its class colours; its pictures
      // follow (textures3d.ts).
      const loader = gltfLoader();
      const pending = deferTextures(loader, url);
      let decoding = false;
      const decode = () => {
        if (decoding || gen !== this.mapGen || this.disposed) return;
        decoding = true;
        onProgress?.(null);
      };
      loader.register(() => ({ name: "viewer-progress", beforeRoot: async () => { decode(); } }));
      const loading = loader.loadAsync(url, event => {
        if (gen !== this.mapGen || this.disposed) return;
        onProgress?.(event);
        if (event.lengthComputable && event.total > 0 && event.loaded >= event.total) decode();
      });
      // Player and vehicle models wait for the level (players3d.ts).
      this.library.hold(loading);
      const gltf = await loading;
      if (this.disposed || gen !== this.mapGen) return "failed";
      // Textured primitives' class materials, for the flat look.
      const [flats, plasma] = await Promise.all([loadFlats(gltf), loadPlasma(gltf)]);
      if (this.disposed || gen !== this.mapGen) return "failed";
      const pairs: TexturePair[] = [];
      const root = gltf.scene;
      // Whatever the glb's nodes are called (`structure`, several `bsp_<n>`,
      // `machines` and its children), every mesh is found by walking it once.
      // The camera and the lights fit the playable level: not far scenery,
      // nor a zone only a cutscene uses.
      const assetExtras = (gltf.asset as { extras?: unknown } | undefined)?.extras;
      const box = new THREE.Box3();
      // Machines and placed objects share the level's class materials in
      // the glb: they get copies of their own, so the level's culling
      // doesn't reach them; vehicle placements another set, which the
      // see-through leaves alone (takesSeeThrough).
      const machineMats = new Map<THREE.Material, THREE.Material>();
      const vehicleMats = new Map<THREE.Material, THREE.Material>();
      const walk = (o: THREE.Object3D, names: string[], aside = false, vehicle = false) => {
        const path = o.name ? [...names, o.name] : names;
        // A scenario placement's extras say what it is.
        if (!vehicle && names.length && /^scenario(_\d+)?$/i.test(names[names.length - 1]!)) {
          const info = objectOf(o.userData);
          vehicle = info !== null && isVehicle(info);
        }
        const pool = vehicle ? vehicleMats : machineMats;
        const own = (m: THREE.Material) => {
          let c = pool.get(m);
          if (!c) { c = m.clone(); pool.set(m, c); }
          return c;
        };
        // (Placed objects don't frame the camera either: some stand far out.)
        aside ||= isBackdrop(o.name) || o.name === "scenario" || (bspNumber(o.name) !== null && isCinematic(o.userData, o.name, assetExtras));
        const role = meshRole(path);
        if (role === "hidden") { o.visible = false; return; }
        const mesh = o as THREE.Mesh;
        // Decals and effects: their own blending, the level's cuts, no part in the framing.
        if (mesh.isMesh && path.some((n) => layerNode(n)) && dressLayer(mesh)) {
          this.cutaway.attach(mesh.material as THREE.Material);
        } else if (mesh.isMesh && !Array.isArray(mesh.material) && plasma.has(mesh.material)) {
          // Plasma is already dressed and shared across its instances.
          used.add(mesh.material);
          mesh.castShadow = mesh.receiveShadow = false;
          this.cutaway.attach(mesh.material);
        } else if (mesh.isMesh) {
          if (role === "machine") mesh.material = Array.isArray(mesh.material) ? mesh.material.map(own) : own(mesh.material);
          const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
          for (const m of mats) { used.add(m); if (dressWater(m, this.look.water)) wet = true; }
          let flat = Array.isArray(mesh.material) ? undefined : flats.get(mesh);
          if (flat && role === "machine") flat = own(flat);
          if (flat) pairs.push({ mesh, textured: mesh.material as THREE.Material, flat });
          let solid = true;
          for (const m of flat ? [...mats, flat] : mats) {
            // Only faces turned to the sun cast: a level seen from above has its
            // ceilings culled, so they shouldn't black out the rooms below either.
            m.shadowSide = THREE.FrontSide;
            if (m.transparent) solid = false;
            // The structure faces into the playable space: with its back faces
            // culled it can be seen into from outside, whatever the glb says.
            // Machines are ordinary models and keep the side they came with,
            // and so does anything cut out by alpha (a grate reads from both sides).
            if (role === "level" && !m.transparent && !m.alphaTest) m.side = THREE.FrontSide;
            this.cutaway.attach(m, takesSeeThrough(role, vehicle));
          }
          mesh.castShadow = solid;
          mesh.receiveShadow = true;
          if (!aside) box.expandByObject(mesh);
        }
        for (const c of o.children) walk(c, path, aside, vehicle);
      };
      const used = new Set<THREE.Material>();
      let wet = false;
      root.updateWorldMatrix(true, true);
      walk(root, []);
      if (box.isEmpty()) box.setFromObject(root);
      if (box.isEmpty()) throw new Error("Map has no geometry");
      this.clearMap();
      // Zones, far scenery, decals and effects, wherever in the tree the glb
      // keeps them, and the level's sky.
      this.look.adopt(root, assetExtras, url, plasma);
      this.look.wet = wet;
      this.textures.adopt(pairs);
      this.scenarioObjects.adopt(root, this.textures);
      this.frameObjects.reset();
      // Machine roots and their optional device clips are both driven by the frames.
      const deviceClips = new Map(gltf.animations.map((clip) => [clip.name, clip]));
      root.traverse((o) => {
        if (o.name !== "machines") return;
        for (const c of o.children) if (c.name && !this.machineNodes.has(c.name)) this.machineNodes.set(c.name, machineNode(c, deviceClips));
      });
      this.features = parseFeatures(assetExtras);
      this.map = root;
      this.floors = new GroundCache((x, y, z, reach) => this.floorProbe(x, y, z, reach));
      this.scene.add(root);
      this.hasMap = true;
      this.drewMap = false;
      mark("h2-viewer:map-loaded");
      if (this.grid) { this.scene.remove(this.grid); this.grid = null; this.gridKey = ""; }
      // glTF (x, y, z) is game (x, -z, y).
      this.setBounds({ min: [box.min.x, -box.max.z, box.min.y], max: [box.max.x, -box.min.z, box.max.y] }, true);
      this.showFeatures();
      this.frameMap = true;
      this.frameCamera(true);
      this.streamPictures(pending, used, gen, plasma);
      return "ok";
    } catch {
      if (gen === this.mapGen && !this.disposed) this.useGrid();
      return "failed";
    }
  }

  /** Fetches the map's pictures and puts each on its surfaces as it comes. */
  private streamPictures(pending: ReturnType<typeof deferTextures>, used: ReadonlySet<THREE.Material>, gen: number, plasma: LevelPlasma) {
    const textures = new Set(plasma.textures);
    for (const p of pending) for (const t of texturesSharing(p.texture, used)) textures.add(t);
    this.mapTextures = [...textures];
    this.onSurfaceProgress?.(0, pending.length);
    // Floors and walls seen at a glancing angle stay sharp: the GPU's most
    // anisotropic filtering (mipmaps are made when each picture arrives).
    const aniso = mapAnisotropy(this.renderer.capabilities.getMaxAnisotropy());
    for (const t of this.mapTextures) t.anisotropy = aniso;
    this.texturesReady = streamTextures(pending, (p, picture) => {
      for (const t of texturesSharing(p.texture, used)) { t.image = picture; t.needsUpdate = true; }
      this.textures.refresh();
      plasma.refresh();
      this.dirty = true;
    }, { concurrency: PICTURES_AT_ONCE, stopped: () => this.disposed || gen !== this.mapGen, progress: this.onSurfaceProgress }).then(() => { if (gen === this.mapGen) mark("h2-viewer:textured"); });
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

  /** Surface pictures on (the default) or the flat class colours. */
  setTextures(on: boolean) { if (this.textures.set(on)) this.dirty = true; }
  /** Whether the level has any pictures to turn off. */
  hasTextures(): boolean { return this.textures.has(); }
  /** The kinds of placed object the level has (scenario3d.ts's names). */
  objectKinds(): string[] { return this.scenarioObjects.kinds(); }
  setObjects(on: boolean) { this.scenarioObjects.setShown(on); this.filmWorld.group.visible = on; }
  setObjectKind(kind: string, on: boolean) { this.scenarioObjects.setKind(kind, on); }

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
      this.cutaway.attach(boxMat);
      const box = new THREE.Mesh(this.geoBox, boxMat);
      box.castShadow = true;
      box.scale.set(2.4, 1.2, 2.4);
      const group = new THREE.Group();
      group.add(box);
      this.scene.add(group);
      o = { group, box, boxMat, model: null, key: "", height: 1.2, kind: m.kind, turrets: [], seats: new Map() };
      this.looseMachines.set(m.id, o);
    }
    const mod = this.models, file = mod?.table.objects[m.kind];
    const url = mod && file ? modelUrl(mod.root, file, mod.table.stamp) : null;
    const template = url ? this.library.vehicle(url) : null;
    const key = template ? url! : "box";
    if (key !== o.key) {
      if (o.model) o.group.remove(o.model);
      o.model = template ? this.library.instantiateVehicle(template, null) : null;
      if (o.model) {
        this.cutaway.attachModel(o.model);
        o.group.add(o.model);
      }
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

  /** What the level offers the toolbar: its zones, far scenery, layers, sky and fog. */
  levelInfo(): LevelInfo { return this.look.info(); }

  /** Which zone to show: the one the game has loaded (`auto`), all, or one. */
  setZonePick(p: ZonePick) { this.look.setPick(p); }
  /** Zones only cutscenes use: listed and shown in `all` when on. */
  setCinematic(on: boolean) { this.look.setCinematic(on); }
  /** The far scenery around the playable space: hidden unless asked for. */
  setBackdrop(on: boolean) { this.look.setBackdrop(on); }
  setDecals(on: boolean) { this.look.setDecals(on); }
  setEffects(on: boolean) { this.look.setEffects(on); }
  setSky(on: boolean) { this.look.setSky(on); }
  setFog(on: boolean) { this.look.setFog(on); }
  /** The level's rain, snow or motes, where they fall. */
  setWeather(on: boolean) { this.look.setWeather(on); }

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
  setSeeThrough(on: boolean) { this.setSeeMode(on ? "auto" : "off"); }

  /** Who the see-through cuts towards: nobody, the player in focus (else
   *  everyone), or everyone. */
  setSeeMode(mode: SeeMode) {
    this.seeMode = mode;
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

    // Project around the bodies as drawn, including floor correction and seats.
    const focus = this.followIndex ?? highlight;
    const cam = this.camera.position;
    const alive = [...this.players]
      .filter(([i, p]) => p.group.visible && !p.actor && this.shown.get(i)?.alive)
      .map(([i, p]) => ({ i, d: cam.distanceToSquared(p.group.position) }))
      .sort((a, b) => a.d - b.d)
      .map((p) => p.i);
    const inFocus = focus !== null && alive.includes(focus) ? focus : null;
    // Through someone's eyes there is nothing between them and the camera.
    const targets = seeTargets(this.mode === "eyes" && !this.fly ? "off" : this.seeMode, inFocus, alive);
    const u = this.cutaway.uniforms;
    let n = 0;
    for (const index of targets) {
      const p = this.players.get(index)!;
      const { x, y, z } = p.group.position;
      const scale = p.turn.scale.x;
      const tall = p.turn.visible
        ? (p.biped ? p.biped.height : 0.86 * p.body.scale.y) * scale
        : p.label.position.y;
      const radius = Math.max(0.15, 0.5 * tall);
      const halfHeight = Math.max(0.15, 0.58 * tall);
      const centerY = y + tall * 0.5;
      const t = u.uSeeTargets.value[n]!;
      if (t.x !== x || t.y !== centerY || t.z !== z || t.w !== radius || u.uSeeHalfHeights.value[n] !== halfHeight) this.dirty = true;
      t.set(x, centerY, z, radius);
      u.uSeeHalfHeights.value[n] = halfHeight;
      n++;
    }
    if (u.uSeeCount.value !== n) this.dirty = true;
    u.uSeeCount.value = n;
  }

  private clearMap() {
    if (!this.map) return;
    for (const t of this.mapTextures) {
      t.dispose();
      const img = t.image as { close?: () => void } | null;
      img?.close?.();
    }
    this.mapTextures = [];
    this.textures.clear();
    this.scenarioObjects.clear();
    this.scene.remove(this.map);
    this.map.traverse((o) => {
      const mesh = o as THREE.SkinnedMesh;
      if (!mesh.isMesh) return;
      if (mesh.isSkinnedMesh) mesh.skeleton.dispose();
      mesh.geometry.dispose();
      for (const m of Array.isArray(mesh.material) ? mesh.material : [mesh.material]) m.dispose();
    });
    this.map = null;
    this.floors = null;
    this.floorGrid = null;
    for (const b of this.bodies.clear()) this.dropBody(b.obj);
    this.zone.reset();
    this.look.clear();
    this.machineNodes.clear();
    this.features = null;
    this.showFeatures();
    this.hasMap = false;
  }

  /** Sizes the lights, shadows, fog, ground and camera limits to a box
   *  (game coordinates). */
  private setBounds(b: Box3, real: boolean) {
    this.bounds = this.frameBox = b;
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
    this.look.setExtent(c, r);
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

  private framePose() {
    if (!this.frameBox) return null;
    const fit = cameraFit(this.frameBox, this.camera.fov, this.camera.aspect);
    const clamp = (d: number) => THREE.MathUtils.clamp(d, this.controls.minDistance * 4, this.controls.maxDistance);
    const target = new THREE.Vector3(...gameToGl(...fit.centre));
    const offset = this.mode === "top"
      ? new THREE.Vector3(0, clamp(fit.topDistance), 0.0001)
      : new THREE.Vector3(...gameToGl(...fit.offset)).setLength(clamp(Math.hypot(...fit.offset)));
    return { target, offset };
  }

  private frameCamera(now: boolean) {
    const pose = this.framePose();
    if (pose) this.moveCamera(pose.target.clone().add(pose.offset), pose.target, now);
  }

  private moveCamera(
    pos: THREE.Vector3, target: THREE.Vector3, now: boolean,
    ms = 650, ease = (t: number) => t * t * (3 - 2 * t),
  ) {
    if (now || this.opts.reducedMotion) {
      this.tween = null;
      this.camera.position.copy(pos);
      this.controls.target.copy(target);
      this.controls.update();
    } else {
      this.tween = { t0: performance.now(), ms, ease, p0: this.camera.position.clone(), p1: pos, q0: this.controls.target.clone(), q1: target };
    }
    this.dirty = true;
  }

  setStart(start: CameraStart) { this.opening = start; }

  private beginOpening() {
    const start = this.opening;
    if (!start) return;
    this.opening = null;
    const pose = this.framePose();
    if (!pose) return;
    const { target, offset } = pose;
    if (start.kind === "player") {
      const player = this.shown.get(start.index);
      if (!player) return;
      const focus = new THREE.Vector3(...gameToGl(player.x, player.y, player.z + this.focusHeight(start.index)));
      const distance = followDistance(offset.length(), this.scale, !!this.players.get(start.index)?.biped, this.controls.minDistance);
      this.moveCamera(target.clone().add(offset), target, true);
      this.setMode("follow", start.index);
      this.moveCamera(focus.clone().add(offset.setLength(distance)), focus, false, 1800, easeOutExpo);
    } else {
      const pitch = THREE.MathUtils.radToDeg(Math.asin(offset.y / offset.length()));
      const from = new THREE.Vector3(...gameToGl(...orbitDirection(pitch + 4, 14)));
      from.setLength(Math.min(offset.length() * 1.35, this.controls.maxDistance));
      this.moveCamera(target.clone().add(from), target, true);
      this.moveCamera(target.clone().add(offset), target, false, 1400, easeOutExpo);
    }
  }

  setMode(mode: ViewMode, follow: number | null = null) {
    this.endFly(false);
    const was = this.mode;
    this.mode = mode;
    this.followIndex = mode === "follow" || mode === "eyes" ? follow : null;
    const top = mode === "top";
    // Through someone's eyes the camera is theirs, not the mouse's.
    this.controls.enabled = mode !== "eyes";
    if (mode === "eyes") this.tween = null;
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
    } else if (top || was !== mode) {
      this.frameCamera(false);
    }
    this.dirty = true;
  }

  /** Whether the camera is behind `index`'s eyes: they aren't drawn then. */
  private isEyes(index: number): boolean {
    return this.mode === "eyes" && this.followIndex === index && !this.fly;
  }

  /** How far above a player's feet the follow camera looks: the chest of
   *  their model as it is drawn now, or of a capsule. */
  private focusHeight(index: number | null): number {
    const p = index !== null ? this.players.get(index) : undefined;
    return p?.biped ? p.biped.height * p.turn.scale.x * 0.6 : this.scale * 0.5;
  }

  /** Back to the whole map, in the current mode. */
  reset() { this.endFly(false); this.frameCamera(false); }

  /** A still shot: the camera at `eye` looking at `at` (game coordinates)
   *  with a vertical field of view of `fov` degrees, at once (web/banner). */
  aim(eye: [number, number, number], at: [number, number, number], fov = this.camera.fov) {
    this.endFly(false);
    this.camera.fov = fov;
    this.camera.updateProjectionMatrix();
    this.moveCamera(new THREE.Vector3(...gameToGl(...eye)), new THREE.Vector3(...gameToGl(...at)), true);
    this.fitNear();
  }

  /** The loaded level's playable bounds (game coordinates), or null before a map. */
  levelBounds(): Box3 | null { return this.hasMap ? this.bounds : null; }

  /** The height of the level's first surface below (x, y, z), game
   *  coordinates, within `reach` units; null when there is none. */
  floorBelow(x: number, y: number, z: number, reach = 20): number | null {
    return this.floorProbe(x, y, z, reach)?.z ?? null;
  }

  /** The first surface facing up below (x, y, z) within `reach`, game
   *  coordinates, with its normal: the level as it lies, without its
   *  machines (a lift is somewhere else a moment later). Probed in a grid
   *  of the level's triangles (floorgrid.ts), built on the first probe. */
  private floorProbe(x: number, y: number, z: number, reach: number): Floor | null {
    if (!this.map) return null;
    this.floorGrid ??= this.buildFloorGrid(this.map);
    const { grid, meshes } = this.floorGrid;
    const [gx, gy, gz] = gameToGl(x, y, z);
    const h = grid.probe(gx, gy, gz, reach, (id) => shownUnder(meshes[id]!, this.map!));
    return h ? { x: h.x, y: -h.z, z: h.y, nx: h.nx, ny: -h.nz, nz: h.ny } : null;
  }

  /** The level's meshes in world space for floor probes: everything but
   *  the machines (they move) and the scenario's placement nodes (hidden;
   *  their instanced batches stand in for them, as placed). */
  private buildFloorGrid(root: THREE.Object3D): { grid: FloorGrid; meshes: THREE.Object3D[] } {
    root.updateWorldMatrix(true, true);
    const meshes: THREE.Object3D[] = [];
    const sources: FloorSource[] = [];
    const v = new THREE.Vector3(), m = new THREE.Matrix4();
    const add = (mesh: THREE.Mesh, world: THREE.Matrix4) => {
      if (!Array.isArray(mesh.material) && this.look.isPlasma(mesh.material)) return;
      const pos = mesh.geometry.getAttribute("position");
      if (!pos) return;
      const out = new Float32Array(pos.count * 3);
      for (let i = 0; i < pos.count; i++) {
        v.fromBufferAttribute(pos, i).applyMatrix4(world);
        out[i * 3] = v.x; out[i * 3 + 1] = v.y; out[i * 3 + 2] = v.z;
      }
      const index = mesh.geometry.getIndex();
      sources.push({ id: meshes.length, positions: out, index: index ? index.array : null });
      meshes.push(mesh);
    };
    const walk = (o: THREE.Object3D) => {
      if (o.name === "machines") return;
      const mesh = o as THREE.Mesh;
      if ((mesh as unknown as THREE.InstancedMesh).isInstancedMesh) {
        const im = mesh as unknown as THREE.InstancedMesh;
        for (let i = 0; i < im.count; i++) { im.getMatrixAt(i, m); add(mesh, m.premultiply(im.matrixWorld)); }
      } else if (mesh.isMesh && !hasScenarioPlacement(o)) {
        add(mesh, mesh.matrixWorld);
      }
      for (const c of o.children) walk(c);
    };
    walk(root);
    return { grid: new FloorGrid(sources), meshes };
  }

  /** The level's placed objects and where they stand (scenario3d.ts). */
  spots() { return this.scenarioObjects.spots(); }

  /** Settles when the level's sky model has loaded (or failed to). */
  skyReady(): Promise<void> { return this.look.skyReady; }

  // ---- flycam ----

  get flying() { return this.fly !== null; }

  /** Flies free from where the camera is; off again goes back to the mode
   *  and view it had (the mode itself is never touched, so a followed
   *  player stays followed). */
  setFly(on: boolean) {
    if (on === (this.fly !== null) || this.disposed) return;
    if (!on) { this.endFly(true); return; }
    const saved = this.tween
      ? { pos: this.tween.p1.clone(), target: this.tween.q1.clone() }
      : { pos: this.camera.position.clone(), target: this.controls.target.clone() };
    this.tween = null;
    const dir = this.camera.getWorldDirection(this.v1);
    const a = anglesOf(dir.x, dir.y, dir.z);
    this.fly = { ...a, vel: new THREE.Vector3(), keys: new Set(), drag: false, moved: 0, dist: Math.max(1, this.camera.position.distanceTo(this.controls.target)), saved };
    this.controls.enabled = false;
    this.onFlyChange?.(true);
    this.onFlySpeed?.(speedLabel(this.flySpeed));
    this.dirty = true;
  }

  /** Leaves the flycam; `back` returns the camera to where it was before. */
  private endFly(back: boolean) {
    const f = this.fly;
    if (!f) return;
    this.fly = null;
    if (document.pointerLockElement === this.renderer.domElement) document.exitPointerLock();
    this.controls.enabled = this.mode !== "eyes";
    if (back) this.moveCamera(f.saved.pos, f.saved.target, false);
    this.onFlyChange?.(false);
    this.dirty = true;
  }

  /** Keys for the flycam: true when one is its own (the page should not scroll for it). */
  flyKey(e: KeyboardEvent, down: boolean): boolean {
    const f = this.fly;
    if (!f || !FLY_KEYS.has(e.code) || e.metaKey || e.altKey) return false;
    if (down) f.keys.add(e.code); else f.keys.delete(e.code);
    return true;
  }

  /** Lets go of every key (the view lost focus, so no key-up will come). */
  flyRelease() { this.fly?.keys.clear(); }

  private flyLook(dx: number, dy: number) {
    const f = this.fly!;
    Object.assign(f, lookStep(f.yaw, f.pitch, dx, dy));
    this.dirty = true;
  }

  /** Mouse drag (left or right button) or pointer lock to look, the wheel
   *  for speed; on a touch screen one finger looks and two fly (spread to
   *  go forward, drag to slide). */
  private listenFly(el: HTMLElement): () => void {
    // Pointers down on the view, by id: where each was last.
    const pointers = new Map<number, { x: number; y: number; touch: boolean }>();
    const down = (e: PointerEvent) => {
      const f = this.fly;
      const touch = e.pointerType === "touch";
      if (!f || (!touch && !looksWith(e.button))) return;
      pointers.set(e.pointerId, { x: e.clientX, y: e.clientY, touch });
      f.drag = true; f.moved = 0;
      el.setPointerCapture(e.pointerId);
    };
    const move = (e: PointerEvent) => {
      const f = this.fly;
      if (!f) return;
      if (document.pointerLockElement === el) {
        const l = pointerLook(e.movementX, e.movementY);
        this.flyLook(l.dx, l.dy);
        return;
      }
      const p = pointers.get(e.pointerId);
      if (!p || !f.drag) return;
      const dx = e.clientX - p.x, dy = e.clientY - p.y;
      f.moved += Math.abs(dx) + Math.abs(dy);
      const touches = [...pointers.entries()].filter(([, q]) => q.touch);
      if (p.touch && touches.length >= 2) {
        // Two fingers: this one moved, the other stayed where it was.
        const other = touches.find(([id]) => id !== e.pointerId)![1];
        const m = twoFingerMove([p.x, p.y], [other.x, other.y], [e.clientX, e.clientY], [other.x, other.y]);
        this.flyNudge(m, this.flyUnitsPerSecond(false) / 300);
      } else {
        const l = pointerLook(dx, dy, p.touch ? TOUCH_LOOK : MOUSE_LOOK);
        this.flyLook(l.dx, l.dy);
      }
      p.x = e.clientX; p.y = e.clientY;
    };
    const up = (e: PointerEvent) => {
      const f = this.fly;
      const p = pointers.get(e.pointerId);
      pointers.delete(e.pointerId);
      if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
      if (!f || !p) return;
      f.drag = pointers.size > 0;
      // A left click rather than a drag takes the mouse (Esc gives it back).
      if (!p.touch && e.button === 0 && f.moved < 4 && document.pointerLockElement !== el) void (el.requestPointerLock() as Promise<void> | undefined)?.catch(() => {});
    };
    // The right button looks, so it must not open the page's menu.
    const menu = (e: MouseEvent) => { if (this.fly) e.preventDefault(); };
    const wheel = (e: WheelEvent) => {
      if (!this.fly) return;
      e.preventDefault();
      const next = speedStep(this.flySpeed, e.deltaY, e.deltaMode);
      if (next === this.flySpeed) return;
      this.flySpeed = next;
      this.onFlySpeed?.(speedLabel(this.flySpeed));
    };
    el.addEventListener("pointerdown", down);
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
    el.addEventListener("contextmenu", menu);
    el.addEventListener("wheel", wheel, { passive: false });
    return () => {
      el.removeEventListener("pointerdown", down);
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
      el.removeEventListener("contextmenu", menu);
      el.removeEventListener("wheel", wheel);
    };
  }

  /** How fast the flycam flies, world units a second. */
  private flyUnitsPerSecond(boost: boolean) {
    return this.scale * 6 * this.flySpeed * (boost ? BOOST : 1);
  }

  /** Moves the flycam at once by `m` (camera frame) times `k` world units. */
  private flyNudge(m: FlyMove, k: number) {
    const f = this.fly;
    if (!f || moveIsIdle(m)) return;
    this.camera.quaternion.setFromEuler(this.e0.set(f.pitch, f.yaw, 0, "YXZ"));
    const fwd = this.camera.getWorldDirection(this.v1);
    const right = this.v2.set(Math.cos(f.yaw), 0, -Math.sin(f.yaw));
    this.camera.position.addScaledVector(fwd, m.forward * k).addScaledVector(right, m.right * k);
    this.camera.position.y += m.up * k;
    this.dirty = true;
  }

  /** One frame of flying: keys and the gamepad to a velocity (eased unless
   *  motion is reduced), the velocity to a position. `dt` in ms. */
  private flyStep(dt: number) {
    const f = this.fly!;
    const sec = dt / 1000;
    const pad = padInput(Array.from(navigator.getGamepads?.() ?? []).find((g) => g?.connected));
    if (pad.lookX || pad.lookY) this.flyLook(pad.lookX * PAD_LOOK * sec, pad.lookY * PAD_LOOK * sec);
    const m = addMoves(keyMove(f.keys), pad.move);
    const speed = this.flyUnitsPerSecond(keyBoost(f.keys) || pad.boost);
    this.camera.quaternion.setFromEuler(this.e0.set(f.pitch, f.yaw, 0, "YXZ"));
    const fwd = this.camera.getWorldDirection(this.v1);
    const right = this.v2.set(Math.cos(f.yaw), 0, -Math.sin(f.yaw));
    const want = this.v0.set(0, m.up, 0).addScaledVector(fwd, m.forward).addScaledVector(right, m.right).multiplyScalar(speed);
    const tau = this.opts.reducedMotion ? 0 : 0.12;
    f.vel.set(approach(f.vel.x, want.x, sec, tau), approach(f.vel.y, want.y, sec, tau), approach(f.vel.z, want.z, sec, tau));
    if (f.vel.lengthSq() > 1e-8) { this.camera.position.addScaledVector(f.vel, sec); this.dirty = true; }
    else f.vel.set(0, 0, 0);
    // The orbit target rides ahead of the camera, for the near plane and the fog.
    this.controls.target.copy(this.camera.position).addScaledVector(fwd, f.dist);
    this.fitNear();
  }

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
      o = { group, box, boxMat, model: null, key: "", height: 1, kind: "", turrets: [], seats: new Map() };
      this.vehicles.set(v.id, o);
    }
    const m = this.models;
    const file = m ? vehicleFile(m.table, v.kind) : null;
    const url = m && file ? modelUrl(m.root, file, m.table.stamp) : null;
    const template = url ? this.library.vehicle(url) : null;
    const empty = !v.occupied;
    const key = (template ? url : "box:" + v.kind) + "|" + (empty ? "empty" : colour ?? "");
    if (key !== o.key) {
      if (o.model) o.group.remove(o.model);
      o.model = template ? this.library.instantiateVehicle(template, empty ? null : colour, empty) : null;
      if (o.model) o.group.add(o.model);
      o.turrets = o.model ? findTurrets(o.model) : [];
      o.seats = new Map();
      o.model?.traverse((n) => { if (n.name.startsWith("seat_") && !o!.seats.has(n.name)) o!.seats.set(n.name, n); });
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
    const colour = info?.colour ?? "#999", name = info?.name ?? `Player ${index + 1}`, bot = info?.bot === true;
    if (p && p.colour === colour && p.name === name && p.bot === bot) return p;
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
    el.append(pip, document.createTextNode(name));
    if (bot) {
      const tag = document.createElement("b");
      tag.className = "v3-bot";
      tag.textContent = "BOT";
      tag.title = "A Clanker bot";
      el.append(tag);
    }
    el.append(gun);
    if (!actor) el.addEventListener("pointerdown", (e) => { e.stopPropagation(); this.opts.onPick?.(index); });
    const label = new CSS2DObject(el);
    label.center.set(0.5, 1);
    const turn = new THREE.Group();
    turn.add(body, arrow);
    const group = new THREE.Group();
    group.add(turn, ring, ray, label);
    this.scene.add(group);
    if (actor) el.classList.add("v3-label--ai");
    p = { group, turn, body, arrow, ring, ray, rayMat, label, el, gun, colour, name, bot, biped: null, bipedUrl: null, weapon: null, weaponUrl: null, weaponName: null, actor, anim: new BipedAnim(), meter: new SpeedMeter(), legs: 0, shotId: 0, wasAlive: null, lastAlive: null };
    this.players.set(index, p);
    return p;
  }

  /** Gives a player the model their species and weapon call for, as far
   *  as those have loaded; a capsule until then, and when there are none. */
  private dress(p: PlayerObj, s: SmoothPlayer) {
    const m = this.models;
    const file = m ? bipedFile(m.table, s.species) : null;
    const url = m && file ? modelUrl(m.root, file, m.table.stamp) : null;
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
      const wurl = m && wfile ? modelUrl(m.root, wfile, m.table.stamp) : null;
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
    const measured = p.meter.update(s.x, s.y, dt);
    // The game's velocity, when the frames carry it, is the speed and the heading.
    const speed = s.vel ? Math.hypot(s.vel[0], s.vel[1]) : s.speed ?? measured;
    const heading = s.vel ? (speed >= WALK_SPEED ? Math.atan2(s.vel[1], s.vel[0]) : null) : p.meter.heading;
    const st = p.anim.update(now, { alive: s.alive, stance: s.stance, speed, z: s.z });
    // The game's own animation, when the frames name it (version 4).
    const game = s.anim ? gameAnim(s.anim.name, false) : null;
    const squat = (game ? game.state.startsWith("crouch") : st.loop === "crouch") ? 0.7 : 1;
    if (p.body.scale.y !== squat) { p.body.scale.y = squat; this.dirty = true; }
    const an = p.biped?.animator;
    if (!an || this.opts.reducedMotion) return;
    const cls = game?.cls ?? this.clipClass(s);
    const clip = game ? clipFor(an.names, cls, game.state) : null;
    const moving = game ? !sideways(game.state) && /^(walk|run|crouch_run)$/.test(game.state) : st.loop === "walk" || st.loop === "run";
    this.aimBody(p, s, an, moving ? heading : null, dt);
    if (game && clip && s.anim) {
      // Where the game has it, at its frame: no guessing from the speed.
      an.setLoopAt(clip, clipTime(s.anim.frame, (an.duration(clip) ?? 0) / 1000, game.loop));
    } else {
      an.setLoop(clipFor(an.names, cls, st.loop), paceScale(st.loop, speed), HELD.has(st.loop));
    }
    if (st.shotId !== p.shotId) {
      p.shotId = st.shotId;
      // Anything but landing is done with the arms: unless they stand still, the legs carry on with the loop.
      const landing = st.shot === "land" || st.shot === "land_hard";
      // The game's own animation lands already, and shows what it is playing itself.
      const shown = !!clip && (landing || st.shot === game?.state);
      if (st.shot && !shown) an.playOnce(clipFor(an.names, cls, st.shot), !landing && (game ? game.state !== "idle" : st.loop !== "idle"));
    }
    an.update(dt);
    this.dirty = true;
  }

  /** Sits a rider in their seat: at the vehicle model's `seat_` node for
   *  it, turned with the vehicle, in the seat's pose (the game's animation
   *  at its frame when the frames name it). False when there is nowhere to
   *  put them or no pose to put them in: the vehicle stands for them. */
  private sit(p: PlayerObj, s: SmoothPlayer, ride: VehicleObj, turret: boolean, first: boolean, dt: number): boolean {
    const an = p.biped?.animator;
    if (!an || !ride.model || !ride.seats.size || this.opts.reducedMotion) return false;
    const mode = s.anim ? s.anim.name.split(":")[0]! : null;
    const name = seatNodeFor([...ride.seats.keys()], { seat: s.seat, mode, turret, first }, this.models?.table.seats?.[ride.kind]);
    const node = name ? ride.seats.get(name) : undefined;
    if (!node || !name) return false;
    const game = s.anim ? gameAnim(s.anim.name, true) : null;
    const state = game?.state ?? seatState(ride.kind, name);
    // Only a pose made for a seat: standing idle in one pokes out of it.
    const clip = clipFor(an.names, game?.cls ?? this.clipClass(s), state);
    if (!clip || !clip.endsWith(":" + state)) return false;
    ride.group.updateMatrixWorld(true);
    node.getWorldPosition(p.group.position);
    node.getWorldQuaternion(p.turn.quaternion);
    p.turn.scale.setScalar(MODEL_SCALE);
    an.setLoopAt(clip, s.anim ? clipTime(s.anim.frame, (an.duration(clip) ?? 0) / 1000, true) : 0);
    // The head and gun turn from the seat towards the aim, as far as a body in a seat turns.
    const { yaw, pitch } = aimAngles(s.aim);
    const fwd = this.v3.set(1, 0, 0).applyQuaternion(p.turn.quaternion);
    const twist = wrapAngle(yaw - Math.atan2(-fwd.z, fwd.x));
    an.aim(pitch, Math.max(-MAX_TWIST / 2, Math.min(MAX_TWIST / 2, twist)));
    an.update(dt);
    this.dirty = true;
    return true;
  }

  /** Points a model where its player looks: the legs follow the movement
   *  (eased, within reach of the aim), the spine, neck and head turn the
   *  rest of the way and pitch up or down to the aim, so the gun points
   *  along it. The body's turn is `turn.rotation.y`, the facing plus `legs`. */
  private aimBody(p: PlayerObj, s: SmoothPlayer, an: BipedAnimator, heading: number | null, dt: number) {
    const { yaw, pitch } = aimAngles(s.aim);
    const up = s.alive && s.stance !== "dead";
    const want = up ? legTwist(yaw, heading) : 0;
    p.legs += (want - p.legs) * Math.min(1, dt / 220);
    // An AI's body is drawn facing where it aims, as the game turns it
    // (its own facing lags behind); a player's faces as they do, which is
    // their aim.
    const body = (p.actor && up ? yaw : s.yaw) + p.legs;
    p.turn.rotation.y = yawToGl(body);
    if (!up) { an.aim(0, 0); return; }
    const twist = wrapAngle(yaw - body);
    const aim = p.actor ? actorAim(pitch, twist) : { pitch, twist };
    an.aim(aim.pitch, aim.twist);
  }

  /** Shows the zone in play: the game's loaded zone where the players are
   *  in it, else the one the followed player (or most players) stands in,
   *  held a moment against flicker (viewer/zones.ts). When it changes and
   *  the camera isn't following anyone, the camera moves to frame it. */
  private followZone(now: number, reported: number | null, highlight: number | null, seeking: boolean) {
    const players: Array<{ x: number; y: number; z: number }> = [];
    for (const s of this.shown.values()) if (!s.actor && s.alive) players.push(s);
    const fi = this.followIndex ?? highlight;
    const f = fi !== null ? this.shown.get(fi) : undefined;
    const focus = f && f.alive && !f.actor ? f : null;
    const bsp = this.zone.update(now, this.look.zoneBoxes(), { reported, focus, players }, { cinematic: this.look.cinematicOn(), snap: seeking });
    const changed = this.look.setActive(bsp);
    if (bsp === null || !this.look.following()) return;
    const box = this.look.zoneBox(bsp);
    if (!box || this.look.info().zoneCount < 2) return;
    const reframed = this.frameBox !== box;
    this.frameBox = box;
    if ((changed || reframed) && !this.opening && this.mode !== "follow" && this.mode !== "eyes" && !this.fly) this.frameCamera(seeking);
  }

  /** Leaves a player's body where they fell: a copy of their model, made
   *  when they die, lying where the frames put them until they respawn,
   *  then staying behind (viewer/bodies.ts). */
  private playerBody(index: number, p: PlayerObj, s: SmoothPlayer, now: number, seeking: boolean) {
    const place: Place = { x: s.x, y: s.y, z: s.z, yaw: s.yaw };
    if (s.alive) {
      if (p.wasAlive === false) this.bodies.release(index);
      p.lastAlive = place;
    } else if (p.wasAlive && !seeking && p.biped && this.models && p.bipedUrl) {
      const template = this.library.biped(p.bipedUrl);
      if (template?.clips.size) {
        const biped = this.library.instantiateBiped(template, p.colour);
        const group = new THREE.Group();
        group.add(biped.object);
        this.scene.add(group);
        const at = bodyPlace(place, p.lastAlive);
        const cls = weaponClass(this.models.table.classes, p.weaponName);
        for (const old of this.bodies.add(index, at, now, { group, biped, anim: new BipedAnim(true), cls })) this.dropBody(old.obj);
        this.dirty = true;
      }
    } else if (!s.alive) {
      const b = this.bodies.following(index);
      if (b) Object.assign(b, bodyPlace(place, p.lastAlive));
    }
    p.wasAlive = s.alive;
  }

  private dropBody(b: BodyObj) {
    this.scene.remove(b.group);
    b.biped.animator?.dispose();
    this.dirty = true;
  }

  /** Places and animates the bodies; old ones go, and a seek clears them all. */
  private updateBodies(now: number, dt: number, seeking: boolean) {
    for (const b of seeking ? this.bodies.clear() : this.bodies.expire(now)) this.dropBody(b.obj);
    for (const b of this.bodies.list) {
      const o = b.obj, an = o.biped.animator;
      const [x, y, z] = gameToGl(b.x, b.y, b.z);
      o.group.position.set(x, y, z);
      o.group.rotation.y = yawToGl(b.yaw);
      o.group.scale.setScalar(MODEL_SCALE);
      if (!an) continue;
      const dieMs = an.duration(clipFor(an.names, o.cls, "die")) ?? 0;
      const st = o.anim.update(now, { alive: false });
      if (this.opts.reducedMotion) { an.setLoop(clipFor(an.names, o.cls, "dead"), 1, true); an.update(0); continue; }
      an.setLoop(clipFor(an.names, o.cls, st.loop), 1, true);
      an.update(dt);
      // Still once the death clip has run (or while it is being followed).
      if (now - b.bornAt < dieMs + 1000 || b.following) this.dirty = true;
    }
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

  /** What a player's shots look like, by their weapon (viewer/projectiles.ts). */
  private shotLook(index: number): Look {
    const s = this.shown.get(index), m = this.models?.table;
    const weapon = s?.weapon ?? null;
    return lookFor(weapon, weapon ? m?.classes[weapon] ?? null : null, weapon ? m?.ballistics?.[weapon] : null);
  }

  /** How far a shot from `from` along `dir` goes: to the victim the frame
   *  names for this shooter, else the first piece of level in the way. */
  private shotDistance(index: number, from: THREE.Vector3, dir: THREE.Vector3, range: number, now: number): number {
    const s = this.shown.get(index);
    const who = s?.actor ? s.actor.id : "p" + index;
    let hit: THREE.Vector3 | null = null;
    for (const h of this.feed.liveHits(now)) {
      if (h.fromId === who && h.age < 600) hit = this.v2.set(...gameToGl(h.x, h.y, h.z + 0.35)).clone();
    }
    const map = this.map;
    const cast = map ? (o: THREE.Vector3, d: THREE.Vector3, far: number) => {
      this.raycaster.set(o, d);
      this.raycaster.far = far;
      return this.raycaster.intersectObject(map, true)[0]?.distance ?? null;
    } : null;
    return shotEnd(from, dir, range, hit, cast);
  }

  /** Draws the shots on screen now: a muzzle flash at the player's weapon,
   *  and the shot itself flying along their aim (viewer/projectiles.ts). */
  private drawShots(now: number, frame: MapFrame) {
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
        const who = s.actor?.id ?? "p" + shot.index;
        const barrel = frame.filmEffects?.find(e => e.kind === "barrel" && e.unit === who);
        if (barrel) {
          // Keep the model's muzzle offset from the shooter's recorded origin.
          const dx = barrel.pos[0] - s.x, dy = barrel.pos[1] - s.y, dz = barrel.pos[2] - s.z;
          t.group.position.add(this.v2.set(...gameToGl(dx, dy, dz)));
        }
        t.group.quaternion.setFromUnitVectors(AXIS_X, this.v1.set(...gameToGl(...(barrel?.direction ?? s.aim))).normalize());
        // Never thinner than a few pixels, however far the camera is.
        const far = this.camera.position.distanceTo(t.group.position);
        const width = Math.max(p.biped ? p.biped.height * 0.024 * k : 0.026 * k, far * 0.0022);
        t.beam.scale.set(Math.max(14, 9 * S), width, width);
        t.flash.scale.setScalar(Math.max(p.biped ? p.biped.height * 0.13 * k : 0.16 * k, far * 0.012));
        // A little roll, so a burst's flashes aren't one shape.
        t.flash.rotation.x = shot.id * 1.9;
        t.group.visible = true;
        const look = this.shotLook(shot.index);
        // Draw fast shots as muzzle tracers. Slower projectiles come from
        // the frame census or the film object stream.
        const drawShot = look.kind === "bullet" || look.kind === "beam";
        t.beam.visible = !p.biped && drawShot;
        if (look.kind === "melee") t.flash.visible = false;
        else if (p.biped && drawShot) {
          const dir = this.v1.clone();
          this.projectiles.fire(look, t.group.position, dir, this.shotDistance(shot.index, t.group.position, dir, look.range, now), now);
        }
      }
      kept.add(shot.id);
      t.beamMat.opacity = shot.alpha * 0.85;
      t.flash.visible = shot.age < FLASH_MS && this.shotLook(shot.index).kind !== "melee";
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

  /** Points a vehicle model's turrets along their aims (each eased, unless
   *  `snap`); one without an aim this frame holds where it was. */
  private aimTurrets(o: VehicleObj, aims: readonly TurretAim[], dt: number, snap: boolean) {
    const kinds = o.turrets.map((t) => t.kind);
    for (const a of aims) {
      const t = o.turrets[pickTurret(kinds, a.kind)];
      if (!t) continue;
      const want = turretWant(t, a.aim);
      const tau = snap || this.opts.reducedMotion ? 0 : 0.08;
      const yaw = easeAngle(t.now.yaw, want.yaw, dt / 1000, tau), pitch = easeAngle(t.now.pitch, want.pitch, dt / 1000, tau);
      if (Math.abs(yaw - t.now.yaw) + Math.abs(pitch - t.now.pitch) > 1e-5) this.dirty = true;
      t.now.yaw = yaw;
      t.now.pitch = pitch;
    }
    for (const t of o.turrets) poseTurret(t);
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
    if (this.frameMap && this.bounds) {
      this.frameMap = false;
      const features = this.features ? [...this.features.teleporters, ...this.features.lifts] : [];
      this.frameBox = this.look.info().zoneCount > 1
        ? this.bounds : playableBounds(this.bounds, this.spots().values(), frame.seen, features);
      if (!this.opening && this.mode !== "follow" && this.mode !== "eyes" && !this.fly) this.frameCamera(true);
    }

    const snap = seeking || this.opts.reducedMotion;
    smoothPlayers(this.shown, frame.actors?.length ? [...frame.players, ...frame.actors] : frame.players, dt, { snap });
    const fired = this.shots.observe(frame.players, now, { quiet: seeking });
    if (frame.actors?.length) fired.push(...this.shots.observe(frame.actors, now, { quiet: seeking }));
    // A sword (or anything that melees) swings rather than shoots.
    for (const index of fired) this.oneShot(index, this.shotLook(index).kind === "melee" ? "melee" : "fire", now);
    for (const e of this.feed.observe(frame, now, { quiet: seeking })) {
      const shot = actionShot(e.action);
      if (shot) this.oneShot(e.index, shot, now);
    }
    this.followZone(now, frame.bsp ?? null, highlight, seeking);
    const S = this.scale;
    const still = this.opts.reducedMotion;

    this.updateMachines(frame.machines ?? [], dt, snap);
    const livingMachines = frame.filmWorld ? new Set((frame.machines ?? []).flatMap(m =>
      m.node ? [m.node, THREE.PropertyBinding.sanitizeNodeName(m.node)] : [])) : null;
    for (const [name, node] of this.machineNodes) {
      const visible = !livingMachines || livingMachines.has(name);
      if (node.obj.visible !== visible) { node.obj.visible = visible; this.dirty = true; }
    }
    for (const node of this.machineNodes.values()) {
      if (placeDevice(node, frame.machinePositions)) this.dirty = true;
    }
    this.scenarioObjects.setCompleteCensus(!!frame.filmWorld);
    let world = frame.filmWorld;
    if (world && frame.objects) {
      // Mod maps and incomplete art exports may omit a placement mesh.
      const missing = frame.objects.flatMap(o => o.source && o.p && !o.gone && !this.scenarioObjects.hasPlacement(o.k, o.i)
        ? [{ ...o.source, pos: o.p, forward: o.f ?? [1, 0, 0] as [number, number, number], up: o.u ?? [0, 0, 1] as [number, number, number] }] : []);
      if (missing.length) world = { ...world, props: world.props.concat(missing) };
    }
    if (this.filmWorld.update(world, p => {
      const mapModel = this.scenarioObjects.template(p.class, p.kind);
      if (mapModel) return mapModel;
      const m = this.models;
      if (!m) return null;
      const file = m.table.objects[p.kind] ?? (p.class === "weap" ? weaponFile(m.table, p.kind) : null);
      return file ? this.library.vehicle(modelUrl(m.root, file, m.table.stamp))?.root ?? null : null;
    })) this.dirty = true;
    this.frameObjects.update(frame.objects, (frame.vehicles?.length ?? 0) > 0);

    // Vehicles first: whoever is in one is drawn as the vehicle. A turret
    // is drawn as part of the vehicle it is mounted on, aimed.
    smoothVehicles(this.shownVehicles, frame.vehicles ?? [], dt, { snap, exact: true });
    const mounts = mountedTurrets(this.shownVehicles.values(), this.shown.values());
    const hullOf = (id: string | null) => (id ? mounts.hullOf.get(id) ?? id : null);
    for (const [id, v] of this.shownVehicles) {
      if (v.parent) continue;
      let colour: string | null = null;
      for (const [index, s] of this.shown) if (hullOf(s.vehicle) === id && !s.actor && s.alive) { colour = roster.get(index)?.colour ?? null; break; }
      const o = this.vehicle(v, colour);
      const [x, y, z] = gameToGl(v.x, v.y, v.z);
      const k = o.model ? MODEL_SCALE : S;
      const b = vehicleBasis(v.forward, v.up);
      this.m4.makeBasis(this.v0.set(...b.x), this.v1.set(...b.y), this.v2.set(...b.z));
      const g = o.group, q = g.quaternion, qx = q.x, qy = q.y, qz = q.z;
      q.setFromRotationMatrix(this.m4);
      if (g.position.x !== x || g.position.y !== y || g.position.z !== z || g.scale.x !== k || q.x !== qx || q.y !== qy || q.z !== qz) this.dirty = true;
      g.position.set(x, y, z);
      g.scale.setScalar(k);
      if (o.turrets.length) this.aimTurrets(o, mounts.aims.get(id) ?? [], dt, snap);
    }
    for (const [id, o] of this.vehicles) {
      if (this.shownVehicles.has(id) && !this.shownVehicles.get(id)!.parent) continue;
      this.scene.remove(o.group);
      o.boxMat.dispose();
      this.vehicles.delete(id);
      this.dirty = true;
    }

    this.floors?.begin();
    this.floors?.keep(this.shown);
    // The first rider seen in each vehicle: its driver, when the frames don't say.
    const firstRider = new Map<string, number>();
    for (const [index, s] of this.shown) if (s.vehicle && s.alive && !firstRider.has(hullOf(s.vehicle)!)) firstRider.set(hullOf(s.vehicle)!, index);
    for (const [index, s] of this.shown) {
      const info = s.actor ? { name: weaponLabel(s.species) || "AI", colour: actorColour(s.actor.team) } : roster.get(index);
      const p = this.player(index, info, !!s.actor);
      const hull = s.vehicle ? hullOf(s.vehicle)! : null;
      const ride = hull ? this.vehicles.get(hull) : undefined;
      if (ride && s.alive) {
        this.dress(p, s);
        const g = p.group;
        const seated = this.sit(p, s, ride, s.vehicle !== hull, firstRider.get(hull!) === index, dt);
        if (seated) {
          // In their seat, drawn; the label above the vehicle.
          g.visible = !this.isEyes(index);
          p.turn.visible = true;
          p.ring.visible = p.ray.visible = false;
          p.label.visible = !this.opts.bare && (!p.actor || this.actorNames);
          p.label.position.y = ride.height * ride.group.scale.x + 0.25 * S - (g.position.y - ride.group.position.y);
          p.el.classList.toggle("on", highlight === index || this.followIndex === index);
          if (s.weapon !== p.weaponName) { p.weaponName = s.weapon; p.gun.textContent = weaponLabel(s.weapon); }
          continue;
        }
        // In a vehicle with no seat to show them in: the vehicle stands for them, and carries their label.
        if (p.turn.visible || !g.visible || !g.position.equals(ride.group.position)) this.dirty = true;
        g.visible = !this.isEyes(index);
        g.position.copy(ride.group.position);
        p.turn.visible = p.ring.visible = p.ray.visible = false;
        p.turn.rotation.set(0, yawToGl(s.yaw), 0);
        p.turn.scale.setScalar(ride.group.scale.x);
        p.label.visible = !this.opts.bare && (!p.actor || this.actorNames);
        p.label.position.y = ride.height * ride.group.scale.x + 0.25 * S;
        p.el.classList.toggle("on", highlight === index || this.followIndex === index);
        if (s.weapon !== p.weaponName) { p.weaponName = s.weapon; p.gun.textContent = weaponLabel(s.weapon); }
        continue;
      }
      // An AI's body comes in the frames (the `dead` stance) and is drawn
      // as it lies; a player's is left behind as a body of its own, made
      // from the model they had alive (the dead have no species or weapon).
      const dead = !s.alive || s.stance === "dead";
      if (!p.actor) this.playerBody(index, p, s, now, seeking);
      this.dress(p, s);
      const show = s.alive || (p.actor && dead);
      p.turn.visible = true;
      p.ring.visible = !dead && !this.opts.bare;
      p.ray.visible = !dead && !p.actor && !this.opts.bare;
      // On their feet, on the floor (ground.ts); in the air or dead, where the frames have them.
      const feet = this.floors ? this.floors.place(index, s) : s.z;
      const [x, y, z] = gameToGl(s.x, s.y, feet);
      const g = p.group, turn = p.turn;
      const big = highlight === index || this.followIndex === index;
      // A capsule (and every ring) grows with the map so it can be found. A
      // model is always the game's own size, beside the map's geometry.
      const k = p.biped
        ? MODEL_SCALE
        : S * (big ? 1.25 : 1) * (p.actor ? 0.8 : 1);
      const yaw = yawToGl(s.yaw);
      if (g.visible !== show || g.position.x !== x || g.position.y !== y || g.position.z !== z || turn.scale.x !== k || turn.rotation.y !== yaw) this.dirty = true;
      g.visible = show && !this.isEyes(index);
      p.label.visible = !dead && !this.opts.bare && (!p.actor || this.actorNames);
      g.position.set(x, y, z);
      // Level again after a seat that leaned with its vehicle.
      turn.rotation.set(0, yaw, 0);
      turn.scale.setScalar(k);
      // The ring fits what it is under: a model's footprint, or the capsule's.
      p.ring.scale.setScalar(p.biped ? k * p.biped.height * 1.25 * (big ? 1.25 : 1) : k);
      p.label.position.y = p.biped ? p.biped.height * k * 1.08 + 0.12 * S : 1.15 * k;
      p.el.classList.toggle("on", big);
      if (show) this.animate(p, s, now, dt);
      if (dead || p.actor) continue;
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
    this.updateBodies(now, dt, seeking);
    this.drawShots(now, frame);
    if (this.effects.film(frame.filmEffects ?? [], still)) this.dirty = true;
    if (this.projectiles.update(now)) this.dirty = true;
    if (this.effects.update(this.feed.liveHits(now), still ? [] : frame.projectileTrails ?? [], S, still)) this.dirty = true;
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

    this.beginOpening();
    // Camera: flying, or a running tween, then following.
    if (this.fly) {
      this.flyStep(dt);
    } else if (this.tween) {
      const tw = this.tween;
      const u = Math.min(1, (performance.now() - tw.t0) / tw.ms), e = tw.ease(u);
      this.camera.position.lerpVectors(tw.p0, tw.p1, e);
      this.controls.target.lerpVectors(tw.q0, tw.q1, e);
      if (u >= 1) this.tween = null;
      this.dirty = true;
    } else if (this.mode === "eyes" && this.followIndex !== null) {
      // First person: at their eyes, along their (eased) aim. The dead
      // leave the camera where it was.
      const s = this.shown.get(this.followIndex);
      if (s && s.alive) {
        const p = this.players.get(this.followIndex);
        const height = p?.biped ? p.biped.height * p.turn.scale.x : null;
        const v = eyeView(s, height);
        const eye = this.v0.set(...gameToGl(...v.eye)), look = this.v1.set(...gameToGl(...v.look));
        if (!this.camera.position.equals(eye) || !this.controls.target.equals(look)) {
          this.camera.position.copy(eye);
          this.controls.target.copy(look);
          this.dirty = true;
        }
        this.camera.lookAt(look);
      }
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
    // Through someone's eyes the orbit controls would pull the camera back to their least distance.
    if (!this.fly && this.mode !== "eyes") this.controls.update();
    this.updateCutaway(dt, highlight);
    // The water's ripples and the weather move by themselves.
    // (Held still when motion is reduced, but the weather still follows the view.)
    const moving = this.look.animate(this.opts.reducedMotion ? 0 : now / 1000, this.controls.target, this.camera.position.distanceTo(this.controls.target));
    if (moving && !this.opts.reducedMotion) this.dirty = true;
    if (!this.dirty || this.curtain) return;
    this.dirty = false;
    this.draw(dt);
  }

  private draw(dt: number) {
    // Marks for the perf harness (web/bench/viewer.bench.ts): the first
    // frame with the level in it.
    this.look.beforeDraw(this.camera, this.camera.position.distanceTo(this.controls.target));
    this.scenarioObjects.beforeDraw();
    // The soft ground stands in for a sky's horizon only when there is none.
    this.ground.visible = !this.look.skyShown();
    // The world matrices once a frame: the passes and the labels would
    // each walk the whole tree (every bone of every biped) again.
    this.scene.updateMatrixWorld();
    this.scene.matrixWorldAutoUpdate = false;
    try {
      if (this.composer && this.aoWanted) this.composer.render();
      else this.renderer.render(this.scene, this.camera);
      this.labels.render(this.scene, this.camera);
    } finally {
      this.scene.matrixWorldAutoUpdate = true;
    }
    if (this.hasMap && !this.drewMap) {
      this.drewMap = true;
      mark("h2-viewer:first-map-frame");
      this.onFirstMapFrame?.();
    }
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
        // With a depth texture, which the occlusion reads (aopass.ts): the
        // composer's second target is a copy, with its own.
        const target = new THREE.WebGLRenderTarget(size.x, size.y, { type: THREE.HalfFloatType, samples: 4, depthTexture: new THREE.DepthTexture(size.x, size.y) });
        const composer = new EffectComposer(this.renderer, target);
        composer.addPass(new RenderPass(this.scene, this.camera));
        const gtao = new LevelAoPass(this.scene, this.camera, this.w, this.h, target.depthTexture!);
        gtao.blendIntensity = 0.9;
        composer.addPass(gtao);
        composer.addPass(new OutputPass());
        composer.setPixelRatio(this.renderer.getPixelRatio());
        composer.setSize(this.w, this.h);
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
    this.endFly(false);
    this.flyOff?.();
    this.clearMap();
    this.controls.dispose();
    for (const p of this.players.values()) this.removePlayer(p);
    this.players.clear();
    for (const b of this.bodies.clear()) this.dropBody(b.obj);
    for (const o of this.vehicles.values()) { if (o.model) o.group.remove(o.model); o.boxMat.dispose(); }
    this.vehicles.clear();
    for (const o of this.looseMachines.values()) { if (o.model) o.group.remove(o.model); o.boxMat.dispose(); }
    this.looseMachines.clear();
    this.geoBox.dispose();
    this.effects.dispose();
    this.projectiles.dispose();
    this.filmWorld.dispose();
    this.cutaway.dispose();
    this.library.dispose();
    this.look.dispose();
    disposeTree(this.scene);
    this.scene.environment?.dispose();
    this.gradient.dispose();
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

/** A performance mark, for the perf harness; never throws. */
function mark(name: string) {
  try { performance.mark(name); } catch { /* no User Timing */ }
}

/** Whether `o` and everything above it up to `root` is shown. */
function shownUnder(o: THREE.Object3D, root: THREE.Object3D): boolean {
  for (let p: THREE.Object3D | null = o; p && p !== root; p = p.parent) if (!p.visible) return false;
  return true;
}

/** Whether `o` is in a scenario placement's node (a child of `scenario`
 *  that is not one of its instanced batches). */
function hasScenarioPlacement(o: THREE.Object3D): boolean {
  for (let p: THREE.Object3D | null = o; p; p = p.parent) if (p.parent?.name === "scenario") return !(p as THREE.InstancedMesh).isInstancedMesh;
  return false;
}
