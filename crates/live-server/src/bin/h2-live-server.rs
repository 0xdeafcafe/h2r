//! `h2-live-server`: the Live service on its own, for the Docker image
//! (docs/live/live-hosting.md). Flags: [`live_server::standalone::USAGE`].

fn main() -> anyhow::Result<()> {
    let args = std::env::args().skip(1).collect();
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let run = live_server::standalone::parse(args, env)?;
    for line in &run.notices {
        println!("h2 live: {line}");
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(live_server::serve(run.config))
}
