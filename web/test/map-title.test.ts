import { describe, expect, test } from "bun:test";
import { mapStem, mapTitle, setMapTitles } from "../src/state/gameart";
import { mapName } from "../src/lib/sessions";

describe("map titles", () => {
  test("file stems become the disc's names once the catalog is in", () => {
    setMapTitles({});
    expect(mapTitle("cyclotron")).toBe("Cyclotron");
    setMapTitles({ cyclotron: "Ivory Tower", beavercreek: "Beaver Creek" });
    expect(mapTitle("cyclotron")).toBe("Ivory Tower");
    expect(mapTitle("Beavercreek")).toBe("Beaver Creek");
    expect(mapTitle("maps\\cyclotron.map")).toBe("Ivory Tower");
    expect(mapTitle("burial_mounds")).toBe("Burial Mounds");
    setMapTitles({});
  });

  test("stems drop paths, extension and case", () => {
    expect(mapStem("C:/maps/Lockout.map")).toBe("lockout");
    expect(mapStem(null)).toBe("");
  });

  test("session map names use the lookup they are given", () => {
    expect(mapName({ map: "cyclotron" }, () => "Ivory Tower")).toBe("Ivory Tower");
    expect(mapName({ map: "lighthouse" })).toBe("lighthouse");
    expect(mapName({ map: null })).toBe("unknown map");
  });
});
