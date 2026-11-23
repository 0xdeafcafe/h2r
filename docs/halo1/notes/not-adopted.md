# Halo 1: not adopted

Evidence for the design pages under [docs/halo1](../). Experiments and
designs that were built and removed, one row each, so that the next lane
does not build them again.

| What it was | Why it went | Removed by |
|---|---|---|
| The health meter as a copy of the shield meter's own bitmap raised 10 pixels (`h1objects::health_hud`, converter v33) | A second shield bar over the first, hiding the shield emblem; the sword-battery strip above the shield bar replaced it ([health.md](../health.md), "The health meter in the HUD") | `db72d0610` (converter v37) |
