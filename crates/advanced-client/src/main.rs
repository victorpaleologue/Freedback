//! Advanced-client binary: maintain a local sync copy of a target's feedback.
//!
//! `freedback-sync --db ./feedback.redb --server http://host:port \
//!     --target https://example.com/item/1 [--reconcile | --full]`

use clap::Parser;
use freedback_advanced_client::{AdvancedClient, LocalStore};

#[derive(Parser)]
#[command(
    name = "freedback-sync",
    about = "Maintain a local Freedback sync copy"
)]
struct Cli {
    /// Path to the local redb database.
    #[arg(long, default_value = "feedback.redb")]
    db: String,
    /// Feedback server base URL.
    #[arg(long)]
    server: String,
    /// Target URI to sync.
    #[arg(long)]
    target: String,
    /// Backdated reconciliation via negentropy (NIP-77): transfers only the
    /// differing ids, falling back to a full pull if the server lacks it.
    #[arg(long)]
    reconcile: bool,
    /// Force a full reconciliation (the labeled fallback) instead of negentropy.
    #[arg(long)]
    full: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let store = LocalStore::open(&cli.db)?;
    let client = AdvancedClient::new(store);

    if cli.reconcile {
        let r = client.reconcile(&cli.server, &cli.target).await?;
        println!(
            "reconcile via {:?} · transferred {} · new {} · rounds {} · cursor {}",
            r.via, r.transferred, r.new, r.rounds, r.cursor
        );
    } else if cli.full {
        let r = client.reconcile_full(&cli.server, &cli.target).await?;
        println!(
            "full · fetched {} · new {} · cursor {}",
            r.fetched, r.new, r.cursor
        );
    } else {
        let r = client.sync(&cli.server, &cli.target).await?;
        println!(
            "sync · fetched {} · new {} · cursor {}",
            r.fetched, r.new, r.cursor
        );
    }

    let live = client.store().live_by_target(&cli.target)?;
    println!("{} live annotations for {}", live.len(), cli.target);
    Ok(())
}
