pub trait SentinelPlugin: Send + Sync {
    fn name(&self) -> &str;
    // Updated to return (MethodName, ErrorMessage) for better Ruby-side context
    fn check(&self, content: &str) -> Vec<(String, String)>;
}

pub struct VoidArgumentPlugin;

impl SentinelPlugin for VoidArgumentPlugin {
    fn name(&self) -> &str {
        "Void Argument"
    }
    fn check(&self, content: &str) -> Vec<(String, String)> {
        let mut issues = Vec::new();
        let mut current_method = String::from("unknown");

        for line in content.lines() {
            if line.trim().starts_with("def ") {
                current_method = line
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .replace("def ", "")
                    .trim()
                    .to_string();
            }

            if line.contains(": void ->") || line.contains("(void)") {
                issues.push((
                    current_method.clone(),
                    "Uses 'void' as an argument (use '()' instead)".to_string(),
                ));
            }
        }
        issues
    }
}

pub struct TypeCasePlugin;

impl SentinelPlugin for TypeCasePlugin {
    fn name(&self) -> &str {
        "Type Case"
    }
    fn check(&self, content: &str) -> Vec<(String, String)> {
        let mut issues = Vec::new();
        let mut current_method = String::from("top-level");

        // The "Wall of Shame" for lowercase primitives
        let primitives = ["string", "integer", "boolean", "array", "hash"];

        for line in content.lines() {
            // Track the method context so the user knows where to look
            if line.trim().starts_with("def ") {
                current_method = line
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .replace("def ", "")
                    .trim()
                    .to_string();
            }

            for p in primitives {
                // We use word boundaries or specific markers like '(' or ' '
                // to avoid accidental matches (like "string_helper")
                let patterns = [
                    format!("({})", p),  // e.g., (string)
                    format!(": {}", p),  // e.g., : string
                    format!("[{}]", p),  // e.g., Array[string]
                    format!("-> {}", p), // e.g., -> string
                ];

                if patterns.iter().any(|pat| line.contains(pat)) {
                    issues.push((
                        current_method.clone(),
                        format!("Found lowercase type '{}'. RBS requires 'String', 'Integer', 'Array', etc.", p)
                    ));
                    break; // Move to next line once one issue is found
                }
            }
        }
        issues
    }
}
