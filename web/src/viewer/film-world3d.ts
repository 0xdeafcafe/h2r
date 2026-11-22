import * as THREE from "three";
import type { FilmProp, FilmVector, FilmWorld } from "../api/types";
import { gameToGl } from "../map/views";
import { vehicleBasis } from "./models";

const levelProps = new Set(["scen", "bloc", "mach", "ctrl", "lifi", "ssce", "weap", "eqip"]);

const proxy: Record<string, { size: FilmVector; colour: number }> = {
  proj: { size: [.045, .026, .026], colour: 0xee9962 },
  weap: { size: [.26, .07, .08], colour: 0x536064 },
  eqip: { size: [.12, .08, .10], colour: 0x6795a0 },
  crea: { size: [.22, .20, .09], colour: 0x708066 },
};

/** Persistent runtime objects outside the exported level placements.
 * Render the map's asset when available, with a class-sized stand-in
 * while loading or when the art pack omits this tag. */
export class FilmWorld3D {
  readonly group = new THREE.Group();
  private props = new Map<string, { node: THREE.Object3D; asset: THREE.Object3D | null }>();
  private box = new THREE.BoxGeometry();
  private ball = new THREE.SphereGeometry(.5, 8, 6);
  private materials = new Map<string, THREE.MeshStandardMaterial>();
  private outline = new THREE.LineLoop(new THREE.BufferGeometry(),
    new THREE.LineBasicMaterial({ color: 0xeac257, transparent: true, opacity: .9 }));
  private lastHill: FilmWorld["hill"] = null;
  private basis = new THREE.Matrix4();
  private q = new THREE.Quaternion();
  private x = new THREE.Vector3();
  private y = new THREE.Vector3();
  private z = new THREE.Vector3();

  constructor(private readonly cutaway: (object: THREE.Object3D) => void) {
    this.outline.visible = false;
    this.group.add(this.outline);
  }

  private standIn(p: FilmProp) {
    const style = proxy[p.class] ?? { size: [.38, .30, .30] as FilmVector, colour: 0x827b69 };
    let material = this.materials.get(p.class);
    if (!material) { material = new THREE.MeshStandardMaterial({ color: style.colour }); this.materials.set(p.class, material); }
    const mesh = new THREE.Mesh(p.class === "proj" ? this.ball : this.box, material);
    mesh.scale.set(...style.size);
    return mesh;
  }

  update(world: FilmWorld | undefined, template: (p: FilmProp) => THREE.Object3D | null,
    visible?: (at: THREE.Vector3) => boolean,
  ) {
    let changed = false;
    const keep = new Set<string>();
    for (const p of world?.props ?? []) {
      keep.add(p.id);
      const asset = template(p);
      let entry = this.props.get(p.id);
      if (!entry || entry.asset !== asset) {
        entry?.node.removeFromParent();
        const node = asset ? asset.clone(true) : this.standIn(p);
        if (levelProps.has(p.class)) this.cutaway(node);
        node.visible = true;
        node.matrixAutoUpdate = true;
        this.group.add(node);
        entry = { node, asset };
        this.props.set(p.id, entry);
        changed = true;
      }
      const node = entry.node;
      const b = vehicleBasis(p.forward, p.up);
      this.basis.makeBasis(this.x.set(...b.x), this.y.set(...b.y), this.z.set(...b.z));
      this.q.setFromRotationMatrix(this.basis);
      this.x.set(...gameToGl(...p.pos));
      const show = levelProps.has(p.class) || (visible?.(this.x) ?? true);
      if (node.visible !== show) { node.visible = show; changed = true; }
      if (!node.position.equals(this.x) || !node.quaternion.equals(this.q)) {
        node.position.copy(this.x);
        node.quaternion.copy(this.q);
        changed = true;
      }
    }
    for (const [id, value] of this.props) if (!keep.has(id)) {
      value.node.removeFromParent(); this.props.delete(id); changed = true;
    }
    const hill = world?.hill ?? null;
    if (hill !== this.lastHill) {
      this.outline.geometry.dispose();
      this.outline.geometry = new THREE.BufferGeometry().setFromPoints((hill?.corners ?? []).map(p =>
        new THREE.Vector3(...gameToGl(...p))));
      this.outline.visible = !!hill && hill.corners.length >= 2;
      this.lastHill = hill;
      changed = true;
    }
    return changed;
  }

  dispose() {
    this.group.removeFromParent();
    this.group.clear();
    this.props.clear();
    this.box.dispose();
    this.ball.dispose();
    this.outline.geometry.dispose();
    this.outline.material.dispose();
    for (const m of this.materials.values()) m.dispose();
  }
}
