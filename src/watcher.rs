use crate::transpiler::SentinelTranspiler;
use anyhow::{Context, Result};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

pub struct SentinelWatcher {
    #[allow(dead_code)]
    watcher: RecommendedWatcher,
    receiver: mpsc::Receiver<notify::Result<notify::Event>>,
    app_root: PathBuf,
}

impl SentinelWatcher {
    /// Initializes the watcher on the specified Ruby directory (e.g., ./app)
    pub fn new(path: &Path) -> Result<Self> {
        let (tx, rx) = mpsc::channel(100);
        let app_root = path.to_path_buf().canonicalize()?;

        // Setup the native watcher with a closure that sends events into our async channel
        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.blocking_send(res);
            },
            Config::default(),
        )?;

        watcher.watch(&app_root, RecursiveMode::Recursive)?;

        Ok(Self {
            watcher,
            receiver: rx,
            app_root,
        })
    }

    /// The main event loop that processes file changes and triggers transpilation
    pub async fn run(mut self) {
        let mut transpiler = SentinelTranspiler::new();
        println!("🚀 Sentinel standing guard over {:?}...", self.app_root);

        while let Some(res) = self.receiver.recv().await {
            match res {
                Ok(event) => {
                    // We only care about file modification and creation
                    if event.kind.is_modify() || event.kind.is_create() {
                        for path in event.paths {
                            if path.extension().map_or(false, |ext| ext == "rb") {
                                self.handle_change(&mut transpiler, &path).await;
                            }
                        }
                    }
                }
                Err(e) => eprintln!("Watcher internal error: {:?}", e),
            }
        }
    }

    async fn handle_change(&self, transpiler: &mut SentinelTranspiler, path: &Path) {
        println!("🔍 Change detected: {:?}", path.file_name().unwrap_or_default());

        match transpiler.transpile_file(path) {
            Ok(rbs_content) => {
                let target_path = self.derive_sig_path(path);

                // Ensure the subdirectory structure exists in sig/generated
                if let Some(parent) = target_path.parent() {
                    if let Err(e) = fs::create_dir_all(parent) {
                        eprintln!("❌ Failed to create directory {:?}: {}", parent, e);
                        return;
                    }
                }

                if let Err(e) = fs::write(&target_path, rbs_content) {
                    eprintln!("❌ Failed to write RBS for {:?}: {}", path, e);
                } else {
                    println!("✅ RBS Synced -> {:?}", target_path.strip_prefix(&self.app_root).unwrap_or(&target_path));
                }
            }
            Err(e) => eprintln!("❌ Transpiler error on {:?}: {}", path, e),
        }
    }

    /// Maps an app file (e.g., ./app/models/user.rb) to a sig file (e.g., ./sig/generated/models/user.rbs)
    fn derive_sig_path(&self, rb_path: &Path) -> PathBuf {
        let mut p = PathBuf::from("./sig/generated");

        // Strip the base path to maintain the internal directory structure
        if let Ok(relative) = rb_path.strip_prefix(&self.app_root) {
            p.push(relative);
        } else {
            // Fallback if the path is outside the root (shouldn't happen with the current setup)
            if let Some(name) = rb_path.file_name() {
                p.push(name);
            }
        }

        p.set_extension("rbs");
        p
    }
}
