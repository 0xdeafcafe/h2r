# editor-glass (experiment)

A UI-only SwiftUI mock of the h2recomp editor in macOS Liquid Glass style. It
has no game data, no map loading and no connection to a game. Every field,
byte and placement is generated from a seeded RNG. It is not the editor: the
editor stays native Rust on Dear ImGui (`crates/apps/studio`). This is an
experiment to look at the style.

Screens: the Scenario editor (viewport, glass toolbar, inspector cards,
paired-game bar), the Tags editor (field list, hex view) and the HUD editor,
in light and dark.

```sh
cd prototypes/editor-glass
swift build
swift run EditorGlass
```
