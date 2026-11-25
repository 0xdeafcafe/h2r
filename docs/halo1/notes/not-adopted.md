# Halo 1: not adopted

Evidence for the design pages under [docs/halo1](../). Experiments and
designs that were built and removed, one row each, so that the next lane
does not build them again.

| What it was | Why it went | Removed by |
|---|---|---|
| The health meter as a copy of the shield meter's own bitmap raised 10 pixels (`h1objects::health_hud`, converter v33) | A second shield bar over the first, hiding the shield emblem; the sword-battery strip above the shield bar replaced it ([health.md](../health.md), "The health meter in the HUD") | `db72d0610` (converter v37) |
| A `phmo` built from a scenery object's Halo 1 collision (`h1objects`, converter versions 85 to 91) | The engine never gives scenery a Havok body (`0x001c35f0` returns unless the object type is in mask `0x1883`), so the built `phmo` was inert and the boulders were walked through ([rebuild.md](../rebuild.md), "Scenery from definitions"; [notes/rebuild.md](rebuild.md)); each placement's collision is merged into the level's instead (`h1convert::scenery_collision`) | `31156269f` |
