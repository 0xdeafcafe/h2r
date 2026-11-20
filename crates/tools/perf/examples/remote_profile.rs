//! `h2 profile remote` without building the whole `h2` (platform/xbox/xbox.sh
//! uses it): `cargo run --release -p perf --example remote_profile -- <dump> ...`.

fn main() {
    match perf::cli::remote(std::env::args().skip(1).collect()) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("profile remote: {e:#}");
            std::process::exit(1);
        }
    }
}
