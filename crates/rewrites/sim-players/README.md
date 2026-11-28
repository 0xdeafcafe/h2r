# sim-players

The players datum array (16 salted `0x21c`-byte records), the players' globals and the four local users' first-person state, as one `sim_world::Pool` and two byte blocks held by the `World` in its players section. Import reads the guest's arrays, scatter writes them back, and `PlayersMem` shows the state to the typed views as guest memory.

[sim-world.md](../../../docs/engine/sim-world.md)
