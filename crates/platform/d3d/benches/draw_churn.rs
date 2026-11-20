#[path = "../tests/draw_churn.rs"]
mod benchmark;

fn main() {
    benchmark::bench();
}
