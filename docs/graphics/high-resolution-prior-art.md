# High resolution: prior art

Status: Reference. Read for behaviour and cited only; no code or data was copied (xemu is GPL).
Code: none. The design these inform is [high-resolution.md](high-resolution.md).

| Project | Resolution method | What we take |
|---|---|---|
| xemu | one integer `surface_scale_factor` for the whole NV2A | the surface policy (scale colour and depth together, leave guest sizes alone) and its issue list as a test list |
| Halo-2-HD | patches the retail game's own back-buffer size on Xbox hardware | which targets must track the back buffer, and what breaks when one does not |

The xemu facts are from a summary of the files on its master branch, not a line-by-line reading. The Halo-2-HD blog was read through a mirror because the site refused direct fetches.

## xemu

<https://github.com/xemu-project/xemu>: `hw/xbox/nv2a/pgraph/pgraph.h`, with the work in `pgraph/gl/surface.c`, `draw.c`, `texture.c` and `glsl/vsh.c`, and the Vulkan backend alongside.

- Colour and zeta surfaces are allocated at the guest size times the factor. The guest's antialiasing mode multiplies the size first.
- Viewport, scissor and clear rectangles are multiplied by the factor. Line width is set to the factor, and point size is multiplied in the generated vertex shader.
- A texture whose address matches a surface renders from the scaled surface on the GPU. Zeta surfaces take a slower CPU path.
- Download to guest RAM reads the scaled surface and keeps one pixel per block. Upload replicates pixels. Changing the scale at run time downloads dirty surfaces and rebuilds them all.
- Anisotropy follows the guest's register and does not interact with the scale.

Known problems above 1x, by issue:

| Issue | Problem |
|---|---|
| [1771](https://github.com/xemu-project/xemu/issues/1771) | a post-process quad with a half-pixel offset samples the neighbouring texel |
| [57](https://github.com/xemu-project/xemu/issues/57), [508](https://github.com/xemu-project/xemu/issues/508) | artefacts along the screen margins; the 1x case was fixed by 4-bit coordinate rounding ([PR 2183](https://github.com/xemu-project/xemu/pull/2183)), which our fixup does |
| [PR 3065](https://github.com/xemu-project/xemu/pull/3065) (open) | corrects the subpixel offset when upscaling; cannot fix low-resolution intermediate targets, copies through the blit path ([417](https://github.com/xemu-project/xemu/issues/417)) and unscaled history buffers ([993](https://github.com/xemu-project/xemu/issues/993)) |
| [2728](https://github.com/xemu-project/xemu/issues/2728), [2747](https://github.com/xemu-project/xemu/issues/2747), [1926](https://github.com/xemu-project/xemu/issues/1926) | bloom ghosts or shrinks |
| [1996](https://github.com/xemu-project/xemu/issues/1996) | shadows flicker and do not scale |
| [1054](https://github.com/xemu-project/xemu/issues/1054), [3119](https://github.com/xemu-project/xemu/issues/3119) | clip rectangles and a download assertion at scale |
| [PR 2984](https://github.com/xemu-project/xemu/pull/2984) | surfaces read back as vertex data must stay at 1x |
| [865](https://github.com/xemu-project/xemu/issues/865), [602](https://github.com/xemu-project/xemu/issues/602) | Halo: Combat Evolved water and texture seams |

No scale-specific Halo 2 issue was found in xemu's tracker.

We classify intermediate targets by guest size. In exact mode readbacks come from a 1x reference. Single-copy mode samples the scaled surface on demand.

## Halo-2-HD

<https://github.com/grimdoomer/Halo-2-HD>, with the author's write-up at <https://icode4.coffee/?p=738>. It patches the retail game to render at 720x480, 720p and 1080i on Xbox hardware. The higher modes need a 128 MB RAM upgrade.

What it changed:

- The stock game always renders 640x480; its 480p mode is an upscale and its widescreen is anamorphic. The patch raises the real back-buffer size.
- The textures the game builds over the back buffers and over the depth buffer are hard-coded to 640x480, and so is the allocation of the texaccum target. Each had to be patched. Left alone, they gave repeating lines and cut-off water on the menu. A size check had to change before the menu's blue filter came back.
- Of about 25 render targets (four or five allocations, from 512x512 down to 64x64), only texaccum was resized. The write-up does not say the bloom, blur and shadow targets were touched; that they stayed at stock size is our inference.
- The swap chain moved from two buffers to three, with a hook that re-points the game's back-buffer textures at each swap.
- Frame buffers moved into tiled memory with matching pitch (a wrong pitch gave striping). Caches were enlarged, and the kernel's allocator limit and the GPU clock were patched.
- Field of view, split-screen layout and fog options were added.

Its README lists a shrunken HUD at higher resolutions, loading, sign-in and lobby screens at 640x480, and fog that scrolls and bleeds between split-screen views. Those come from changing a size the game can see.

We keep the guest's size, so the HUD and menus keep their layout. Xbox performance is GPU-bound: 720p runs about 19 fps stock and 27-28 with triple buffering, tiled texaccum and the overclock, and 1080i is single digits. Fill is the cost here too.

What carries over to the renderer:

- The back-buffer and depth aliases must track the buffer exactly, or lines and cut-off water appear. This matches our "frame buffers as textures" path.
- One intermediate target follows the back-buffer size and must scale with it.
- The game looks right with every other target at stock size, which supports leaving the square targets alone.
