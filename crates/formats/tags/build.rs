mod schema;
use schema::{Kind, Record};
use std::{env, fmt::Write, fs, path::PathBuf};

fn ty(k: Kind) -> String {
    match k {
        Kind::U8 | Kind::Flags8 => "u8".into(),
        Kind::U16 => "u16".into(),
        Kind::I16 => "i16".into(),
        Kind::U32 => "u32".into(),
        Kind::F32 => "f32".into(),
        Kind::Enum16 => "Enum16".into(),
        Kind::Enum32 => "Enum32".into(),
        Kind::Flags16 => "Flags16".into(),
        Kind::StringId => "StringId".into(),
        Kind::TagRef => "TagReference".into(),
        Kind::DataRef => "DataReference".into(),
        Kind::Block(_) | Kind::EmptyBlock => "TagBlock".into(),
        Kind::Bytes(n) => format!("[u8; {n}]"),
        Kind::U32s(n) => format!("[u32; {n}]"),
        Kind::F32s(n) => format!("[f32; {n}]"),
    }
}
fn validate(r: &Record) {
    assert!(r.size > 0, "empty record");
    let mut occupied = vec![false; r.size];
    let mut names = std::collections::HashSet::new();
    for f in &r.fields {
        assert!(names.insert(f.name), "duplicate {}.{}", r.name, f.name);
        assert!(
            f.kind.width() > 0 && f.offset + f.kind.width() <= r.size,
            "field outside {}.{}",
            r.name,
            f.name
        );
        if let Kind::Block(stride) = f.kind {
            assert!(stride > 0, "zero block stride");
        }
        for slot in &mut occupied[f.offset..f.offset + f.kind.width()] {
            assert!(!*slot, "overlap in {}.{}", r.name, f.name);
            *slot = true;
        }
    }
}
fn main() {
    println!("cargo:rerun-if-changed=schema.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let mut out = String::from(
        "// Generated from this crate's reviewed schema.rs. Do not edit.\nuse crate::wire::*;\n",
    );
    let records = schema::records();
    for r in &records {
        for field in &r.fields {
            if let Some(name) = field.element {
                let target = records
                    .iter()
                    .find(|r| r.name == name)
                    .expect("unknown element record");
                let Kind::Block(stride) = field.kind else {
                    panic!("element on non-block field")
                };
                assert_eq!(
                    stride as usize, target.size,
                    "block stride differs from record size"
                );
            }
        }
    }
    for mut r in records {
        validate(&r);
        r.fields.sort_by_key(|f| f.offset);
        let name = r.name;
        let size = r.size;
        writeln!(
            out,
            "/// Xbox retail wire record. Encoders preserve uninterpreted bytes."
        )
        .unwrap();
        writeln!(out, "#[derive(Clone, PartialEq)]\npub struct {name} {{").unwrap();
        writeln!(out, "    original: [u8; {size}],").unwrap();
        for f in &r.fields {
            writeln!(
                out,
                "    /// Field at offset {:#x}, {} bytes.",
                f.offset,
                f.kind.width()
            )
            .unwrap();
            writeln!(out, "    pub {}: {},", f.name, ty(f.kind)).unwrap();
        }
        writeln!(out, "}}\n\nimpl std::fmt::Debug for {name} {{").unwrap();
        writeln!(
            out,
            "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{"
        )
        .unwrap();
        writeln!(out, "        debug_record(self, f)\n    }}\n}}\n").unwrap();
        writeln!(
            out,
            "impl BinaryRecord for {name} {{\n    const SIZE: usize = {size};"
        )
        .unwrap();
        writeln!(out, "    const META: RecordMeta = RecordMeta {{\n        name: {name:?},\n        size: {size},\n        fields: &[").unwrap();
        for f in &r.fields {
            writeln!(out, "            FieldMeta {{").unwrap();
            writeln!(out, "                name: {:?},\n                offset: {},\n                size: {},", f.name, f.offset, f.kind.width()).unwrap();
            writeln!(out, "                kind: FieldKind::{:?},", f.kind).unwrap();
            writeln!(out, "                choices: &[").unwrap();
            for (value, label) in f.choices {
                writeln!(out, "                    ({value}, {label:?}),").unwrap();
            }
            writeln!(
                out,
                "                ],\n                element: {:?},",
                f.element
            )
            .unwrap();
            writeln!(out, "            }},").unwrap();
        }
        writeln!(out, "        ],\n    }};\n").unwrap();
        writeln!(out, "    fn decode(bytes: &[u8]) -> crate::Result<Self> {{").unwrap();
        writeln!(
            out,
            "        exact(bytes.len(), Self::SIZE)?;\n        Ok(Self {{"
        )
        .unwrap();
        writeln!(
            out,
            "            original: bytes.try_into().expect(\"checked record size\"),"
        )
        .unwrap();
        for f in &r.fields {
            writeln!(
                out,
                "            {}: <{} as Wire>::read(&bytes[{}..{}])?,",
                f.name,
                ty(f.kind),
                f.offset,
                f.offset + f.kind.width()
            )
            .unwrap();
        }
        writeln!(out, "        }})\n    }}\n").unwrap();
        writeln!(
            out,
            "    fn encode_into(&self, bytes: &mut [u8]) -> crate::Result<()> {{"
        )
        .unwrap();
        writeln!(out, "        exact(bytes.len(), Self::SIZE)?;\n        bytes.copy_from_slice(&self.original);").unwrap();
        for f in &r.fields {
            writeln!(
                out,
                "        self.{}.write(&mut bytes[{}..{}])?;",
                f.name,
                f.offset,
                f.offset + f.kind.width()
            )
            .unwrap();
        }
        writeln!(out, "        Ok(())\n    }}\n}}\n").unwrap();
        // Keep runtime wire-width changes from silently diverging from the schema.
        for f in &r.fields {
            writeln!(
                out,
                "const _: [(); {}] = [(); <{} as Wire>::SIZE];",
                f.kind.width(),
                ty(f.kind)
            )
            .unwrap();
        }
    }
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("records.rs"),
        out,
    )
    .unwrap();
}
