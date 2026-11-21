import { expect, test } from "bun:test";
import { openFilmHref } from "../src/theater/open";

test("film and clip links preserve the selected Live home and viewing options", () => {
  const server = "https://live.example:8443/hub";
  expect(openFilmHref(server, "game_1")).toBe("h2recomp://theater/game_1?server=https%3A%2F%2Flive.example%3A8443%2Fhub");
  expect(openFilmHref(server, "game_1", "clip-2", "?t=42&seat=0&camera=orbit&token=private"))
    .toBe("h2recomp://theater/game_1?server=https%3A%2F%2Flive.example%3A8443%2Fhub&clip=clip-2&t=42&seat=0&camera=orbit");
  expect(openFilmHref(server, "../game")).toBeNull();
  expect(openFilmHref(server, "game", "a/b")).toBeNull();
  expect(openFilmHref("https://user@live.example", "game")).toBeNull();
  for (const search of ["?seat=16", "?t=-1", "?t=4294967296", "?t=0&t=1", "?camera=no"]) {
    expect(openFilmHref(server, "game", undefined, search)).toBeNull();
  }
});
