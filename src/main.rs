// main.rs
mod plugin;
mod transpiler;
mod watcher;

use anyhow::Result;
use std::path::Path;
use watcher::SentinelWatcher;

#[tokio::main]
async fn main() -> Result<()> {
    let app_path = Path::new("./app");
    let sig_path = Path::new("./sig/generated");

    // Ensure the output directory exists
    std::fs::create_dir_all(sig_path)?;

    // Initialize our async watcher
    let watcher = SentinelWatcher::new(app_path)?.with_plugins();

    // Run the event loop
    watcher.run().await;

    Ok(())
}
