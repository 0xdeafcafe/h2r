# App icon: Earthlight Chrome

The h2recomp app icon on every platform: a chrome "2" over a lit Earth at
sunrise, with an orbital station, its elevator and a fleet. It is a
hand-drawn homage. No game pixels, extracted meshes, traced glyphs,
proprietary fonts, Bungie, Microsoft or Xbox marks, or HALO wordmark are in
it; the station and ship silhouettes were built independently.

[proof-index.png](proof-index.png) shows every export at a glance (a
reduced copy of the full proof, which stays out of the repository).

## Credit

**Earth albedo: NASA Earth Observatory, Blue Marble: Next Generation,
January 2004, with topography and bathymetry.** Public domain; credited
as NASA asks.

- Catalog: <https://science.nasa.gov/earth/earth-observatory/blue-marble-next-generation/base-topography-bathymetry/>
- Source image: <https://assets.science.nasa.gov/content/dam/science/esd/eo/images/bmng/bmng-topography-bathymetry/january/world.topo.bathy.200401.3x5400x2700.jpg>

Everything else (sphere projection, lighting, clouds and their shadows,
atmosphere, stars, station, elevator, ships, numeral, effects) was authored
for this icon. The clouds are procedural, not NASA cloud imagery.

## Palette

| Role | Hex |
| --- | --- |
| Space; UWP tile and splash background | `#061323` |
| Deep chrome reflection | `#244A7B` |
| Blue metal | `#467FB2` |
| Sky reflection | `#ADC3D8` |
| Chrome highlight | `#EDF7FF` |
| Atmospheric blue | `#6CBFFF` |
| Restrained amber rim | `#E9AC68` |
| Warm sunrise | `#FFD6A0` |
| Running-light red | `#EF665F` |

These are anchors; the Earth, lighting, bloom and grain add the colours in
between.

## Where each export lives

| Platform | Files | Used by |
| --- | --- | --- |
| iOS, iPadOS | `platform/ios/H2iOS/Assets.xcassets/AppIcon.appiconset`: one 1024 universal image, plus dark (transparent space) and tinted (grayscale) appearances | `ASSETCATALOG_COMPILER_APPICON_NAME: AppIcon` in `platform/ios/project.yml` |
| tvOS | `platform/tvos/H2TV/Assets.xcassets/App Icon & Top Shelf Image.brandassets`: the App Icon stack (400x240 @1x/@2x) and App Store stack (1280x768), each Front, Middle and Back; Top Shelf Image (1920x720 @1x/@2x) and Top Shelf Image Wide (2320x720 @1x/@2x) | `ASSETCATALOG_COMPILER_APPICON_NAME: "App Icon & Top Shelf Image"` in `platform/tvos/project.yml` |
| macOS windows | `crates/apps/app-icon/assets/AppIcon-512.png`, the plated Mac icon | `app_icon::set_dock_icon()`: the Dock icon of `h2 play` (`crates/apps/app`) and `h2-editor` (`crates/apps/studio`), which have no bundle |
| Xbox (UWP) | `platform/xbox/Assets/` on the `xbox` branch | the AppxManifest that `uwp-host`'s `package` example writes (docs/runtime/xbox.md) |

The tvOS layers sit Front over Middle over Back on one registered canvas:
Back is space, Earth and clouds (opaque); Middle the atmosphere, orbital
arc, station, ships, elevator and sunrise; Front the chrome numeral with
its amber rim and shadow. Parallax has not been checked on an Apple TV.

The Xbox images are the scale-200 exports under plain names
(`Square150x150Logo.png` and so on): makemsix on a Mac makes no
`resources.pri`, and without one Windows cannot pick between qualified
names, so the manifest names each file directly.

The macOS `.icns` and iconset (ten sizes, with simplified 16 and 32 point
renditions) are not committed: nothing here builds a Mac `.app` yet.
Regenerate them from the sources below when something does.

## Sources

`sources/` holds the vector sources:

- `masters/icon.svg`, `tvos.svg`, `topshelf.svg`, `topshelfwide.svg`,
  `uwp-wide.svg`: the scene compositions, with named Back, Middle and Front
  groups. Each links raster layers beside it (`icon-back.png` and so on),
  rendered from the NASA albedo and the lit models. Those rasters are not
  committed (about 300 MB), so these files open with the layers missing.
- `masters/*-numeral.svg`: the chrome numeral at each canvas size, true
  vector.
- `unplated-numeral.svg`, `macos-optical-micro.svg`, `status-glyph.svg`,
  `lockscreen-glyph.svg`, `StatusIconTemplate.svg`: the small adaptations
  (the isolated numeral, the 16 and 32 point Mac icon, the status and
  lock-screen glyphs).

## Regenerating

The exports were made by a Python pipeline (NumPy, Pillow, CairoSVG and
Cairo, plus macOS `iconutil`) that renders the Earth from the NASA texture,
lights the station and ship models, composes each scene from the masters
and writes every platform's files, then checks every dimension, alpha
channel and ICNS entry and builds a light and dark proof at native size
and 4x. That pipeline, its render caches, the NASA image and the full
proof are kept outside the repository with the artwork's working files.
To change the icon, re-run it there and copy the exports into the places
in the table above, keeping each file's name; then check the catalogs
compile:

```sh
xcrun actool platform/ios/H2iOS/Assets.xcassets --compile /tmp/ios-icon \
  --platform iphoneos --minimum-deployment-target 17.0 \
  --target-device iphone --target-device ipad --app-icon AppIcon \
  --output-partial-info-plist /tmp/ios-icon/partial.plist --warnings --errors
xcrun actool platform/tvos/H2TV/Assets.xcassets --compile /tmp/tv-icon \
  --platform appletvos --minimum-deployment-target 17.0 --target-device tv \
  --app-icon "App Icon & Top Shelf Image" \
  --output-partial-info-plist /tmp/tv-icon/partial.plist --warnings --errors
```

and run `cargo test -p app-icon` (and `-p uwp-host` for
the Xbox images).
