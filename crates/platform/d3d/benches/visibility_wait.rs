#[path = "../tests/visibility_async.rs"]
mod benchmark;

fn main() {
    benchmark::bench();
}
