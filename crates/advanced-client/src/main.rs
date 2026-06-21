//! Advanced-client binary: maintain a local sync copy of a target's feedback.
//!
//! `freedback-sync --db ./feedback.redb --server http://host:port \
//!     --target https://example.com/item/1 [--full]`

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
    /// Do a full reconciliation (catches backdated items) instead of a cursor pull.
    #[arg(long)]
    full: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let store = LocalStore::open(&cli.db)?;
    let client = AdvancedClient::new(store);

    let report = if cli.full {
        client.reconcile_full(&cli.server, &cli.target).await?
    } else {
        client.sync(&cli.server, &cli.target).await?
    };

    println!(
        "fetched {} · new {} · cursor {}",
        report.fetched, report.new, report.cursor
    );
    let live = client.store().live_by_target(&cli.target)?;
    println!("{} live annotations for {}", live.len(), cli.target);
    Ok(())
}
