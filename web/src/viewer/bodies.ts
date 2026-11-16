// Dead players' bodies in the 3D view. When a player dies their model
// stays where they fell: it plays the dying clip and holds its last
// frame. The frames place a dead player at their body while it lasts (an
// older sender has no body and sends the origin; then it stays where they
// last stood). Respawning makes a new model for the player; the body stays
// behind until it is old (the game clears bodies away) or there are too
// many. AI bodies need none of this: the frames carry them as actors with
// the `dead` stance until the game removes them.
//
// Pure: the scene hangs a model on each body (`obj`).

/** How long a body lies after its player died (ms). */
export const BODY_MS = 60_000;
/** At most this many bodies are kept; the oldest goes first. */
export const MAX_BODIES = 24;

export interface Place { x: number; y: number; z: number; yaw: number }

export interface Body<T> extends Place {
  /** The player it was. */
  index: number;
  bornAt: number;
  /** Still that player's body: moved to where the frames put them, until they respawn. */
  following: boolean;
  obj: T;
}

/** Where a dead player's body lies: where the frame puts them, unless that
 *  is the origin (a sender without bodies), then where they last stood. */
export function bodyPlace(at: Place, last: Place | null): Place {
  const none = at.x === 0 && at.y === 0 && at.z === 0;
  return none && last ? { ...last } : { x: at.x, y: at.y, z: at.z, yaw: at.yaw };
}

export class Bodies<T> {
  readonly list: Body<T>[] = [];

  /** A new body; returns those it pushed out (the oldest past the cap). */
  add(index: number, place: Place, now: number, obj: T): Body<T>[] {
    for (const b of this.list) if (b.index === index) b.following = false;
    this.list.push({ index, ...place, bornAt: now, following: true, obj });
    return this.list.length > MAX_BODIES ? this.list.splice(0, this.list.length - MAX_BODIES) : [];
  }

  /** The body a dead player is lying as, if any. */
  following(index: number): Body<T> | undefined {
    return this.list.find((b) => b.following && b.index === index);
  }

  /** The player is up again: their body stays where it is. */
  release(index: number): void {
    for (const b of this.list) if (b.index === index) b.following = false;
  }

  /** Takes out the bodies older than `BODY_MS` and returns them. */
  expire(now: number): Body<T>[] {
    const out: Body<T>[] = [];
    for (let i = this.list.length - 1; i >= 0; i--) {
      const b = this.list[i]!;
      if (now - b.bornAt >= BODY_MS || now < b.bornAt) out.push(...this.list.splice(i, 1));
    }
    return out;
  }

  /** Every body out (a seek, a new level). */
  clear(): Body<T>[] {
    return this.list.splice(0, this.list.length);
  }
}
