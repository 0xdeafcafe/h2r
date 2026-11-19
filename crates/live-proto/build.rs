fn main() {
    let source = "src/clanker_appearance.c";
    println!("cargo:rerun-if-changed={source}");
    cc::Build::new().file(source).compile("clanker_appearance");
}
