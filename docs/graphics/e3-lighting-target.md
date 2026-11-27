# E3 lighting: the target look and its sources

Status: Design. The look is what the pictures of each slice of [e3-lighting.md](e3-lighting.md) are judged against.
Code: none. The checks are `render.capture` pairs through `render.compare`.
Switch: none.

| Group | Items |
|---|---|
| Shadows | 1 hard edges, 2 sun shadows from characters and vehicles, 3 large vehicles, 4 several shadows per object, 9 baked sun shadows |
| Lights | 5 lamps, 6 headlights, 7 weapon fire, 12 gels |
| Shading | 8 dark shade, 10 normal-mapped light, 11 specular |
| Out of scope | 13 bloom |

Footage times are rows of the table in [pstencil-history.md](pstencil-history.md), section 12.

## Evidence labels

This page and [e3-lighting.md](e3-lighting.md) use their own scheme.

| Label | Meaning |
|---|---|
| **P** | a developer's own statement, with the source. **P\*** is a statement read through a secondary quotation (a wiki, a mirror, a search excerpt) because the original was unreachable, so the wording is unverified |
| **C** | a community finding or a secondary characterisation |
| **R** | read from the retail image or disc by this project, in the doc named |
| **S** | our inference or speculation |
| **Design** | our choice, not a fact about the game |
| **Estimate** | arithmetic on a measurement taken elsewhere, to be replaced by a measurement in a frame |

## The look

| # | Item | Source |
|---:|---|---|
| 1 | **Hard shadow edges, no visible filtering**, from the sun and from local lights, on large and small casters | Halopedia "sharp rather than softened" (**C**); Steam commenter "razor sharp" (**C**); footage 2:00, 5:24, 7:12 (**S**) |
| 2 | **Characters and vehicles cast onto the world and onto themselves under the sun**, not only under dynamic lights | Staten (**P\***), Halopedia (**C**), Beyond3D (**C**) |
| 3 | **Large vehicles cast large shadows** (the Pelican) | Jones (**P\***), footage 1:07 (**S**) |
| 4 | **Several shadows per object where several lights reach it** | Beyond3D on the Ghost (**C**) |
| 5 | **Lamps cast shadows and throw pools of light** that read as discrete shapes on the ground | Halopedia (**C**), footage 5:00, 7:12 (**S**) |
| 6 | **Vehicle headlights light the road in front** and move with the vehicle | Halopedia (**C**), footage 5:44-5:49 (**S**) |
| 7 | **Weapon fire and explosions light their surroundings**, including the firer | Perez via Halopedia (**P\***), Halopedia on the turret (**C**), footage 3:19 (**S**) |
| 8 | **Shadowed areas are very dark**: low ambient, high contrast, characters close to silhouettes in shade | Footage 2:00-2:09, 6:27 (**S**) only; no statement |
| 9 | **The level's own sun shadows are baked, and sharp.** Retail's were softened. Perez's "combine lightmaps for ambient lighting with stencil for directional sources" (**P**) is the plan the demo's level did not show. Not met: retail's bake is what the disc has ([e3-lighting.md](e3-lighting.md), "The lightmap problem") | Halopedia (**C**) |
| 10 | **Dynamic lights shade per pixel through normal maps**, so armour and walls show relief under a moving light | Freidin (**P**), normal maps everywhere (**C**), footage 6:33, 8:36 (**S**) |
| 11 | **Specular highlights from dynamic lights** on armour and visors | Staten on the visor (**P\***), footage 8:36-8:42 (**S**) |
| 12 | **Coloured, projected lights (gels)** where a light has one | Digsite (**C**) |
| 13 | **Out of scope but part of the look:** the demo's different bloom | Halopedia (**C**) |

## Related

[e3-lighting.md](e3-lighting.md), [pstencil-history.md](pstencil-history.md),
[pstencil.md](pstencil.md).
