# Our content packages

Each folder here that holds a `package.toml` is one of our packages
(docs/editor/studio-redesign.md, "Packages"). `boot`'s build script bakes
every file of each into `h2` (`crates/apps/boot/build.rs`,
`boot::packages`), and the game loads them before the user's own under
`<data>/packages/`.

`h2 package check content/` reports what is wrong with them;
`h2 package build content/ --base <map> -o <out.map>` writes the map a game
would read.

A package holds our own sources and data, never bytes from a disc.

`core` holds the scripts of the seven retail game types (`rules` items) and
`modes` holds our built-in list entries (`variant` items), the example and
matchmaking scripts (`rules` items) and the default hopper's playlists
(`playlist`, `playlists/default.toml`). The `.rules` files are also what
`rules::builtin` compiles in, so editing one changes the constants and the
package together.

`halo1` and `map-pack` hold recipes (`convert` items), not maps: the
converters read the user's own Halo 1 and Map Pack discs at prepare time
and write under the data root (`h2 package prepare`;
docs/editor/studio-redesign.md, "Conversions").
