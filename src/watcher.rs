use crate::plugin::{SentinelPlugin, TypeCasePlugin, VoidArgumentPlugin}; // Import your plugins
use crate::transpiler::SentinelTranspiler;
use anyhow::Result;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};

pub struct SentinelWatcher {
    #[allow(dead_code)]
    watcher: RecommendedWatcher,
    receiver: mpsc::Receiver<notify::Result<notify::Event>>,
    app_root: PathBuf,
    plugins: Vec<Box<dyn SentinelPlugin>>,
}

impl SentinelWatcher {
    /// Initializes the watcher on the specified Ruby directory (e.g., ./app)
    pub fn new(path: &Path) -> Result<Self> {
        let (tx, rx) = mpsc::channel(100);
        let app_root = path.to_path_buf().canonicalize()?;

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
            plugins: Vec::new(), // Initialize empty
        })
    }

    /// Register default plugins for RBS linting
    pub fn with_plugins(mut self) -> Self {
        self.plugins.push(Box::new(VoidArgumentPlugin));
        self.plugins.push(Box::new(TypeCasePlugin));
        self
    }

    /// The main event loop that processes file changes and triggers transpilation
    pub async fn run(mut self) {
        let mut transpiler = SentinelTranspiler::new();
        println!("🚀 Sentinel standing guard over {:?}...", self.app_root);

        while let Some(res) = self.receiver.recv().await {
            match res {
                Ok(event) => {
                    if event.kind.is_modify() || event.kind.is_create() {
                        // 1. Drain duplicate events (debouncing)
                        while self.receiver.try_recv().is_ok() {}

                        for path in event.paths {
                            if path.extension().is_some_and(|ext| ext == "rb") {
                                // 2. Tiny delay to ensure file lock is released
                                sleep(Duration::from_millis(50)).await;
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
        println!(
            "🔍 Change detected: {:?}",
            path.file_name().unwrap_or_default()
        );

        match transpiler.transpile_file(path) {
            Ok(rbs_content) => {
                // 1. RUN PLUGINS (The new linter system)
                for plugin in &self.plugins {
                    let issues = plugin.check(&rbs_content);
                    if !issues.is_empty() {
                        eprintln!(
                            "⚠️  [{}] issues in {:?}:",
                            plugin.name(),
                            path.file_name().unwrap_or_default()
                        );
                        for (method, msg) in issues {
                            eprintln!("   - Method `{}`: {}", method, msg);
                        }
                    }
                }

                let target_path = self.derive_sig_path(path);

                // 2. Ensure directory exists
                if let Some(parent) = target_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }

                // 3. Write file
                if let Err(e) = std::fs::write(&target_path, &rbs_content) {
                    eprintln!("❌ Failed to write RBS: {}", e);
                } else {
                    println!(
                        "✅ RBS Synced -> {:?}",
                        target_path
                            .strip_prefix(&self.app_root)
                            .unwrap_or(&target_path)
                    );
                }
            }
            Err(e) => eprintln!("❌ Transpiler error: {}", e),
        }
    }

    fn derive_sig_path(&self, rb_path: &Path) -> PathBuf {
        let mut p = PathBuf::from("./sig/generated");
        if let Ok(relative) = rb_path.strip_prefix(&self.app_root) {
            p.push(relative);
        } else if let Some(name) = rb_path.file_name() {
            p.push(name);
        }
        p.set_extension("rbs");
        p
    }
}
