# sim-script

HaloScript on the native simulation's world. The `hs` VM's persistent state (threads and their frames, the thread pool's allocation cursor, global cells, host strings) is the world's scripts section; `ScriptRuntime` updates it each tick; `Natives` gives engine builtins Rust bodies where a Rust system exists, and the rest go to the guest engine adapter.

[hs_vm.md](../../../docs/engine/hs_vm.md) · [sim-world.md](../../../docs/engine/sim-world.md)
