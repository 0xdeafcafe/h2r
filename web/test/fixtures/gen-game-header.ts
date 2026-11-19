// Writes the game-header fixtures: the fixture game's model as
// game-header.json and its markup as game-header.html. The web test and
// the Rust parity test (crates/live-server/src/page_header.rs) both render
// that model and hold themselves to that markup, so when the header's
// shape changes, regenerate (bun test/fixtures/gen-game-header.ts) and
// port the change to the Rust side in the same commit.

import { writeFileSync } from "node:fs";
import { gameHeaderHtml, gameHeaderJson, gameHeaderModel } from "../../src/stats/gameHeader";
import { fixtureGame } from "./game-header-game";

// Generate for the page origin, as both server-side renderers do.
const model = gameHeaderModel(fixtureGame());
writeFileSync(new URL("game-header.json", import.meta.url), gameHeaderJson(model) + "\n");
writeFileSync(new URL("game-header.html", import.meta.url), gameHeaderHtml(model) + "\n");
console.log(gameHeaderJson(model));
console.log(gameHeaderHtml(model));
