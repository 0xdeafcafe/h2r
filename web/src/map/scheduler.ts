// One requestAnimationFrame loop shared by every canvas on the page, so
// several live minimaps cost one callback per frame, not one each.

type Draw = (now: number) => void;
const draws = new Set<Draw>();
let handle = 0;

function loop(now: number) {
  handle = 0;
  for (const d of draws) {
    try { d(now); } catch (e) { console.error(e); }
  }
  if (draws.size) handle = requestAnimationFrame(loop);
}

export function onAnimationFrame(d: Draw): () => void {
  draws.add(d);
  if (!handle) handle = requestAnimationFrame(loop);
  return () => {
    draws.delete(d);
    if (!draws.size && handle) { cancelAnimationFrame(handle); handle = 0; }
  };
}
