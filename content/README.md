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
