#[path = "../tests/render_churn.rs"]
mod benchmark;

fn main() {
    benchmark::bench();
}
