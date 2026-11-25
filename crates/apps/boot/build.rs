//! `$OUT_DIR/packages_baked.rs`: every package under `content/` (a folder
//! with a `package.toml`), each file `include_bytes!`d by its path in the
//! package, so the game carries our packages with no file read
//! (`boot::packages`, docs/editor/studio-redesign.md, "The model").

use std::path::{Path, PathBuf};

fn files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let hidden = p
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'));
        if hidden {
            continue;
        }
        if p.is_dir() {
            files(root, &p, out);
        } else {
            let rel = p
                .strip_prefix(root)
                .expect("under the package")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, p));
        }
    }
}

fn main() {
    let content = Path::new("../../../content");
    println!("cargo:rerun-if-changed={}", content.display());
    let mut packages: Vec<PathBuf> = std::fs::read_dir(content)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("package.toml").is_file())
        .collect();
    packages.sort();
    let mut out = String::from("pub const BAKED: &[(&str, &[(&str, &[u8])])] = &[\n");
    for dir in &packages {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .expect("a package folder's name is text");
        let mut list = Vec::new();
        files(dir, dir, &mut list);
        out.push_str(&format!("    ({name:?}, &[\n"));
        for (rel, path) in list {
            let abs =
                std::fs::canonicalize(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            println!("cargo:rerun-if-changed={}", abs.display());
            out.push_str(&format!(
                "        ({rel:?}, include_bytes!({:?}).as_slice()),\n",
                abs.display().to_string()
            ));
        }
        out.push_str("    ]),\n");
    }
    out.push_str("];\n");
    let dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(dir.join("packages_baked.rs"), out).expect("packages_baked.rs");
}
