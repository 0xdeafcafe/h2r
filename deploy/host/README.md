# Dedicated host image

The Linux/amd64 image runs dedicated matches with a headless `h2` and the
module compiled from your own disc. With `H2_PACK=1` it runs `h2 host pack`,
which takes several leases' matches over a private control port; Live on
Cloudflare starts it that way.
[Packed hosts](../../docs/live/dedicated-host.md#packed-hosts) Without it,
it runs a single lease, as below. Use a private registry for this image.

## Build on macOS

Install [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild), Zig,
the Rust `x86_64-unknown-linux-gnu` target and LLVM's `clang`, `ld.lld`,
`llvm-ar` and `llvm-strip`. Put the LLVM tools on `PATH` and use an amd64 Debian bookworm
sysroot with glibc development files, Linux headers and GCC support files.
The script checks these prerequisites and prints installation hints.
[Linux module build](../../docs/runtime/linux.md)

From the repository root, set `H2_DISC` to the owner's ISO or `default.xbe`
and `H2_LINUX_SYSROOT` to the absolute sysroot path. Stage the host and module.

```sh
deploy/host/build.sh "$H2_DISC"
```

Pass `--coverage FILE` to stage a slim module built from the recorded owner
list. [Record and check a slim host](../../docs/live/dedicated-host-build.md#slim-host)

Pack the staged files into a local image.

```sh
docker buildx build --platform linux/amd64 --load -t h2host:local deploy/host
```

The script builds the Linux host against bookworm's glibc 2.36 and uses a
Mac build of `h2` from the same checkout to emit `libh2game.so` and its
stamp. Set `H2_BIN` to reuse a Mac binary from that revision. The script
strips the staged copies. The scratch image copies the ELF runtime
dependencies, certificates, entrypoint shell and staged files. Supply the
matching code-generation settings at runtime if you override defaults
such as `H2_OPT`.

For a private registry push, export the rootfs layer with gzip level 9.
Set `H2_HOST_IMAGE` to the private image reference.

```sh
docker buildx build --platform linux/amd64 \
  --output "type=image,name=$H2_HOST_IMAGE,push=true,compression=gzip,compression-level=9,force-compression=true" \
  deploy/host
```

## Run one lease

Prepare the owner's private game-data bundle with `h2 host bundle`. The
host needs the matching `default.xbe` at boot, supplied with the private
runtime data. Before starting the container, export the lease ID, Live
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
`H2_REGION` is optional. The bundle cache defaults to `/var/cache/h2`.
For reuse across runs, mount a volume there writable by UID 1000. The
image exposes TCP 31100 for the Cloudflare datagram tunnel.
[Dedicated host design](../../docs/live/dedicated-host.md#container-image)
