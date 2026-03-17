mod init;
mod plugin;
mod transpiler;
mod watcher;

use anyhow::Result;
use std::path::Path;
use watcher::SentinelWatcher;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("watch");

    let app_path = Path::new("./app");
    let sig_path = Path::new("./sig/generated");
    std::fs::create_dir_all(sig_path)?;

    match command {
        "init" => init::run(app_path),
        "watch" => {
            init::run(app_path);
            let watcher = SentinelWatcher::new(app_path)?.with_plugins();
            watcher.run().await;
        }
        other => {
            eprintln!("Unknown command: {}", other);
            eprintln!("Usage: sentinel [init|watch]");
            std::process::exit(1);
        }
    }

    Ok(())
}
