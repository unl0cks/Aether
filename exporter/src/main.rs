use anyhow::Result;
use clap::Parser;
use exporter::{cli::Opt, run_main};

fn main() -> Result<()> {
    // The engine reports through `tracing`, and without a subscriber every warning it emits is
    // dropped -- including the diagnostics that exist to explain a rendering fault. Quiet by
    // default; `RUST_LOG=warn` turns it on.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    // Aether's cache policies are runtime switches that only the AQW preset turns on, so a plain
    // export renders as upstream Ruffle would. `AETHER_CACHE_POLICIES=1` turns them on here instead,
    // which is what lets a rendering fault that only appears in Aether be reproduced headlessly
    // rather than guessed at from a screenshot.
    if std::env::var_os("AETHER_CACHE_POLICIES").is_some() {
        ruffle_core::aether_performance::set_adaptive_avatar_cache_enabled(true);
        ruffle_core::aether_performance::set_cache_texture_grid_enabled(true);
    }

    let opt: Opt = Opt::parse();
    run_main(opt)
}
