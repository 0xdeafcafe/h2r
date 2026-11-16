//! The app-container API check (platform/xbox/build.sh): every function
//! the executable and the game module import must be one a UWP app can
//! call on every Windows 10 device family, the Xbox included, which is
//! what the Windows SDK's `OneCoreUAP.lib` umbrella library exports. An
//! import outside it may simply not exist on the console, and then the
//! process does not load at all; this finds that on the Mac.

use std::collections::BTreeSet;

/// One imported function: (DLL, function name).
pub type Import = (String, String);

/// The imports in `llvm-readobj --coff-imports` output: each `Import` or
/// `DelayImport` block's `Name:` and its `Symbol:` lines. Imports by
/// ordinal (no name) are skipped.
pub fn parse_readobj(listing: &str) -> Vec<Import> {
    let mut out = Vec::new();
    let mut dll: Option<String> = None;
    for line in listing.lines().map(str::trim) {
        if line.starts_with("Import {") || line.starts_with("DelayImport {") {
            dll = None;
        } else if let Some(n) = line.strip_prefix("Name: ") {
            dll.get_or_insert_with(|| n.trim().to_owned());
        } else if let Some(s) = line.strip_prefix("Symbol: ") {
            let name = s.split(" (").next().unwrap_or("").trim();
            if let (Some(d), false) = (&dll, name.is_empty()) {
                out.push((d.clone(), name.to_owned()));
            }
        }
    }
    out
}

/// The function names an import library exports, from
/// `llvm-nm --just-symbol-name` over it: each `__imp_<name>` gives `<name>`.
pub fn parse_nm(listing: &str) -> BTreeSet<String> {
    listing.lines().filter_map(|l| l.trim().strip_prefix("__imp_")).map(str::to_owned).collect()
}

/// The imports whose function no allowed library exports, sorted and
/// without repeats.
pub fn outside(imports: &[Import], allowed: &BTreeSet<String>) -> Vec<Import> {
    let set: BTreeSet<&Import> = imports.iter().filter(|(_, f)| !allowed.contains(f)).collect();
    set.into_iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const READOBJ: &str = "\
File: h2uwp.exe
Format: COFF-x86-64
Arch: x86_64
AddressSize: 64bit
Import {
  Name: KERNEL32.dll
  ImportLookupTableRVA: 0x1234
  ImportAddressTableRVA: 0x5678
  Symbol: GetLastError (0)
  Symbol: CreateFileW (0)
  Symbol:  (12)
}
Import {
  Name: USER32.dll
  ImportLookupTableRVA: 0x1
  ImportAddressTableRVA: 0x2
  Symbol: MessageBoxW (0)
}
DelayImport {
  Name: d3d12.dll
  Attributes: 0x1
  Symbol: D3D12CreateDevice (0)
}
";

    #[test]
    fn readobj_listings_give_each_dll_and_its_named_functions() {
        assert_eq!(
            parse_readobj(READOBJ),
            [
                ("KERNEL32.dll".into(), "GetLastError".into()),
                ("KERNEL32.dll".into(), "CreateFileW".into()),
                ("USER32.dll".into(), "MessageBoxW".into()),
                ("d3d12.dll".into(), "D3D12CreateDevice".into()),
            ]
        );
    }

    #[test]
    fn functions_outside_the_umbrella_library_are_reported() {
        let nm = "\napi-ms-win-core-errorhandling-l1-1-0.dll:\n__imp_GetLastError\nGetLastError\n__imp_CreateFileW\nCreateFileW\n__imp_D3D12CreateDevice\n";
        let allowed = parse_nm(nm);
        assert_eq!(allowed.len(), 3);
        assert_eq!(outside(&parse_readobj(READOBJ), &allowed), [("USER32.dll".to_owned(), "MessageBoxW".to_owned())]);
    }
}
