use lsp_types::{notification::DidSaveTextDocument, Notification};
use serde_json::Value;

pub fn handle_notification(method: &str, params: Value) {
    match method {
        DidSaveTextDocument::METHOD => {
            // 1. Get the file path
            // 2. Trigger the Transpiler
            // 3. Log it for the "BBS/Cookbook"
            println!("Sentinel intercepted save: Triggering Rust-based RBS sync.");
        }
        _ => {
            // Pass-through other notifications to Steep
        }
    }
}
