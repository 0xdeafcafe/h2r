# xbe

A parser for original Xbox executables. `Xbe::parse` borrows the file
bytes and resolves everything against the virtual address space the image
loads into: sections, certificate, libraries, the TLS directory and the
kernel import table.

## Where it sits

Domain `formats`; no workspace dependencies. It is used by the whole
recompiler (`analyze`, `lift`, `emit`, `clif`, `symbols`), the runtime
(`runtime`, `kernel`, `lifted`, `recomp`), the platform crates that pin
retail addresses, and `boot`, `cli` and `game`.

## Layout

- `src/lib.rs`: the whole crate: `Xbe`, `Section`, `SectionFlags`,
  `Certificate`, `Library`, `KernelImport`, `TlsDirectory`, and the
  address-space reads (`bytes_at`, `read_u32`, `section_containing`).

## Tests

`cargo nextest run -p xbe`. The tests use invented images.

## Docs

- [docs/runtime/xbox.md](../../../docs/runtime/xbox.md)
- [docs/runtime/BOOT.md](../../../docs/runtime/BOOT.md)
