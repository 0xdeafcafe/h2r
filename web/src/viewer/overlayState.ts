// The map uplink's load and progress states. See docs/live/viewer-overlay.md, "The overlay".
export type OverlayState = "boot" | "loading" | "decoding" | "reveal" | "ready" | "none" | "error" | "unsupported";

export interface OverlayFacts {
  sceneReady: boolean;
  unavailable?: boolean;
  glb?: string | null;
  manifestReady?: boolean;
  waitExpired?: boolean;
  phase?: "fetching" | "decoding" | "ok" | "failed";
  reveal?: "started" | "done";
}

export function overlayState(facts: OverlayFacts): OverlayState {
  if (facts.unavailable) return "unsupported";
  if (!facts.sceneReady) return "boot";
  if (!facts.glb) return facts.manifestReady || facts.waitExpired ? "none" : "loading";
  if (facts.phase === "failed") return "error";
  if (facts.reveal === "done") return "ready";
  if (facts.reveal === "started") return "reveal";
  if (facts.phase === "decoding" || facts.phase === "ok") return "decoding";
  return "loading";
}

export const LOAD_SEGMENTS = 24;

export interface LoadProgress {
  lit: number;
  text: string;
  percent: number | null;
  size: string | null;
  chasing: boolean;
}

function formatSize(bytes: number): string {
  if (bytes < 1000) return `${Math.floor(bytes)} B`;
  if (bytes < 1000000) return `${Number((bytes / 1000).toFixed(1))} KB`;
  return `${Number((bytes / 1000000).toFixed(1))} MB`;
}

export function loadProgress(state: OverlayState, loaded = 0, total?: number, previousLit = 0): LoadProgress {
  if (state === "boot") return { lit: previousLit, text: "Loading viewer", percent: null, size: null, chasing: true };
  if (state === "decoding") return { lit: Math.max(23, previousLit), text: "Decoding geometry", percent: null, size: null, chasing: false };
  if (state === "reveal" || state === "ready") {
    return { lit: LOAD_SEGMENTS, text: "Map geometry ready", percent: 100, size: null, chasing: false };
  }
  if (state !== "loading") return { lit: previousLit, text: "", percent: null, size: null, chasing: false };
  if (total === undefined || total <= 0) {
    return { lit: previousLit, text: "Loading map geometry", percent: null, size: loaded > 0 ? formatSize(loaded) : null, chasing: true };
  }
  const ratio = Math.min(1, Math.max(0, loaded / total));
  return {
    lit: Math.max(1, Math.floor(22 * ratio), previousLit),
    text: "Loading map geometry",
    percent: Math.floor(ratio * 100),
    size: null,
    chasing: false,
  };
}
