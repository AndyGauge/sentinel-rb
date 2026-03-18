mod config;
mod init;
mod plugin;
mod transpiler;
mod watcher;

use anyhow::Result;
use config::SentinelConfig;
use watcher::SentinelWatcher;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(|s| s.as_str()).unwrap_or("watch");

    match command {
        "init" => {
            let config = SentinelConfig::ensure_exists()?;
            std::fs::create_dir_all(config.output_path())?;
            for folder in config.folder_paths() {
                init::run(&folder, &config.output_path());
            }
        }
        "watch" => {
            let config = SentinelConfig::ensure_exists()?;
            std::fs::create_dir_all(config.output_path())?;
            for folder in config.folder_paths() {
                init::run(&folder, &config.output_path());
            }
            let watcher = SentinelWatcher::new(&config)?.with_plugins();
            watcher.run().await;
        }
        "add" => {
            let folder = args.get(2).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: sentinel add <folder>");
                std::process::exit(1);
            });
            let mut config = SentinelConfig::ensure_exists()?;
            if config.add_folder(folder) {
                config.save()?;
                println!("Added '{}' — watching: {:?}", folder, config.folders);
            } else {
                println!("'{}' is already in the watch list.", folder);
            }
        }
        "remove" => {
            let folder = args.get(2).map(|s| s.as_str()).unwrap_or_else(|| {
                eprintln!("Usage: sentinel remove <folder>");
                std::process::exit(1);
            });
            let mut config = SentinelConfig::load()?;
            if config.remove_folder(folder) {
                config.save()?;
                println!("Removed '{}' — watching: {:?}", folder, config.folders);
            } else {
                println!("'{}' is not in the watch list.", folder);
            }
        }
        "list" => {
            let config = SentinelConfig::load()?;
            println!("Watched folders:");
            for folder in &config.folders {
                println!("  {}", folder);
            }
            println!("Output: {}", config.output);
        }
        other => {
            eprintln!("Unknown command: {}", other);
            eprintln!("Usage: sentinel [init|watch|add|remove|list]");
            eprintln!();
            eprintln!("Commands:");
            eprintln!("  init           Generate RBS files for all watched folders");
            eprintln!("  watch          Watch for changes and regenerate (default)");
            eprintln!("  add <folder>   Add a folder to the watch list");
            eprintln!("  remove <folder> Remove a folder from the watch list");
            eprintln!("  list           Show watched folders and output path");
            std::process::exit(1);
        }
    }

    Ok(())
}
