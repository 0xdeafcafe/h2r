//! `h2-live-server`: the Live service on its own, for the Docker image
//! (docs/live/live-hosting.md). Flags: [`live_server::standalone::USAGE`];
//! `h2-live-server store copy ...` moves a log and blobs between backends
//! ([`live_server::copy::USAGE`]); `store check` tries a bucket
//! ([`live_server::check::USAGE`]).

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    if args.first().map(String::as_str) == Some("store") {
        let rest = args[2.min(args.len())..].to_vec();
        return match args.get(1).map(String::as_str) {
            Some("copy") => runtime.block_on(live_server::copy::run(rest, &env)),
            Some("check") => runtime.block_on(live_server::check::run(rest, &env)),
            _ => anyhow::bail!(
                "{}\n\n{}",
                live_server::copy::USAGE,
                live_server::check::USAGE
            ),
        };
    }
    let run = live_server::standalone::parse(args, env)?;
    for line in &run.notices {
        println!("h2 live: {line}");
    }
    runtime.block_on(live_server::serve(run.config))
}
