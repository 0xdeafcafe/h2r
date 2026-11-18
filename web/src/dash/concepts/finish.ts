// Only carry a comparison into another dashboard screen. Its URL can be
// shared or reloaded without storing a preference on a player's account.
export function readFinish(value: unknown) {
  return value === "original" || value === "refined" ? value : undefined;
}

export function finishHref(href: string, finish: ReturnType<typeof readFinish>) {
  if (!finish || !(href === "/" || href.startsWith("/dash/"))) return href;
  const url = new URL(href, "https://preview.invalid");
  url.searchParams.set("finish", finish);
  return url.pathname + url.search + url.hash;
}
