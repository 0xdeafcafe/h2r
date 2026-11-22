# Dedicated host image

The Linux/amd64 image runs dedicated matches with a headless `h2`. It holds
public code only: the game module compiled from your own disc comes with
the private host-data bundle, which the host downloads and verifies at
start. With `H2_PACK=1` it runs `h2 host pack`,
which takes several leases' matches over a private control port; Live on
Cloudflare starts it that way.
[Packed hosts](../../docs/live/dedicated-host.md#packed-hosts) Without it,
it runs a single lease, as below. CI builds and pushes this image on
every push to master ([Deploy from CI](../../docs/live/live-worker.md#deploy-from-ci)).

## Build the Linux h2

Install [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild), Zig
and the Rust `x86_64-unknown-linux-gnu` target, and `llvm-strip` (on Linux,
binutils' `strip` also works). From the repository root, on macOS or
Linux, stage the headless `h2` in `deploy/host/stage/h2`.

```sh
deploy/host/build.sh --h2
```

It links against bookworm's glibc 2.36 and needs no disc. Pack it into a
local image.

```sh
docker buildx build --platform linux/amd64 --load -t h2host:local deploy/host
```

The scratch image copies the ELF runtime dependencies of `h2` and of the
module, certificates, the entrypoint shell and `h2` in two layers, least
often changed first: the base files, then `h2` with the entrypoint. A
push uploads only the layers that changed.

## Build the module for the bundle

The module is the owner's: build it on macOS and put it in the private
host-data bundle, never in an image. Install LLVM's `clang`, `ld.lld`,
`llvm-ar` and `llvm-strip`, put them on `PATH`, and use an amd64 Debian
bookworm sysroot with glibc development files, Linux headers and GCC
support files. The script checks these prerequisites and prints
installation hints.
[Linux module build](../../docs/runtime/linux.md)

Set `H2_DISC` to the owner's ISO or `default.xbe` and `H2_LINUX_SYSROOT`
to the absolute sysroot path, check out the revision CI deployed, and
stage the module in `deploy/host/stage/image/`.

```sh
deploy/host/build.sh --module "$H2_DISC"
h2 host bundle --disc "$H2_DISC" --image deploy/host/stage/image --out "$bundle_dir"
```

The script uses a Mac build of `h2` from the same checkout to emit
`libh2game.so` and its stamp; set `H2_BIN` to reuse a Mac binary from that
revision. It stages a slim module, native code only for the functions a
host runs, when it has a coverage list: `--coverage FILE`,
`H2_HOST_COVERAGE`, or `deploy/host/host-coverage.txt` when that file
exists. Git ignores that file; keep it private with the build. With no
list it stages the full module.
[Record and check a slim host](../../docs/live/dedicated-host-build.md#slim-host)

The module's stamp must match the deployed `h2`: same generator revision
and code-generation settings, such as `H2_OPT` (the container sets none).
A host refuses a bundle whose module does not match, with a named error.
[The game module](../../docs/live/dedicated-host-build.md#the-game-module)

With neither `--h2` nor `--module`, the script stages both.

For a registry push by hand, export the layers with gzip level 9.
Set `H2_HOST_IMAGE` to the image reference.

```sh
docker buildx build --platform linux/amd64 \
  --output "type=image,name=$H2_HOST_IMAGE,push=true,compression=gzip,compression-level=9,force-compression=true" \
  deploy/host
```

## Run one lease

Prepare the owner's private game-data bundle with `h2 host bundle
--image deploy/host/stage/image`. It carries the matching `default.xbe`,
the maps, the game module and each multiplayer map's bot nav mesh and
visibility grid, so the host builds none. Before starting the container, export the lease ID, Live
URL, lease token, signed bundle URL and bundle SHA-256. Set
`H2_HOST_LEASE_JSON` to the complete lease supplied by Live. Its `id`
must match `H2_LEASE`.

For a UDP lease through Docker, set `H2_PUBLIC_ADDR` to the Docker host's
IPv4 address reachable by clients. If you change `H2_BASE_PORT`, publish
the matching seven-port UDP range.

To get `H2_LEASE`, `H2_HOST_LEASE_JSON`, `H2_TOKEN` and `H2_LIVE` in one
step, reserve the lease on the server and eval what it prints:

```sh
eval "$(h2 live hosts start --map lockout --no-launch --live http://127.0.0.1:8074)"
```

Then run the container:

```sh
docker run --rm --name h2host-smoke --platform linux/amd64 \
  -p 31000-31006:31000-31006/udp \
  -e H2_LEASE -e H2_HOST_LEASE_JSON -e H2_LIVE -e H2_TOKEN \
  -e H2_DATA_URL -e H2_DATA_SHA \
  -e H2_REGION -e H2_OPT -e H2_BASE_PORT=31000 -e H2_PUBLIC_ADDR \
  h2host:local
```

The entrypoint passes those lease and data values to `h2 host run`.
`H2_REGION` is optional. The bundle cache defaults to `/var/cache/h2`,
and the host runs the module from the verified bundle there; set
`H2_IMAGE` and mount a module directory to run another.
For reuse across runs, mount a volume there writable by UID 1000. The
image exposes TCP 31100 for the Cloudflare datagram tunnel.
[Dedicated host design](../../docs/live/dedicated-host.md#container-image)
