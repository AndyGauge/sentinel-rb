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

pub struct AngleBracketPlugin;

impl SentinelPlugin for AngleBracketPlugin {
    fn name(&self) -> &str {
        "Angle Bracket"
    }
    fn check(&self, content: &str) -> Vec<(String, String)> {
        let mut issues = Vec::new();
        let mut current_method = String::from("top-level");

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

            // Skip comment lines
            if line.trim().starts_with('#') {
                continue;
            }

            // Match patterns like Array<X>, Hash<X>, Set<X>, etc.
            // Look for a capitalized identifier followed by <
            let bytes = line.as_bytes();
            for (i, &b) in bytes.iter().enumerate() {
                if b == b'<' && i > 0 {
                    // Check if preceded by an identifier char (letter/digit/underscore)
                    let prev = bytes[i - 1];
                    if prev.is_ascii_alphanumeric() || prev == b'_' {
                        // Walk back to find the start of the identifier
                        let mut start = i - 1;
                        while start > 0
                            && (bytes[start - 1].is_ascii_alphanumeric()
                                || bytes[start - 1] == b'_'
                                || bytes[start - 1] == b':')
                        {
                            start -= 1;
                        }
                        let ident = &line[start..i];
                        // Only flag if the identifier starts with uppercase (a type name)
                        if ident
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_uppercase() || c == ':')
                        {
                            issues.push((
                                current_method.clone(),
                                format!(
                                    "'{}<...>' uses angle brackets. RBS uses square brackets: '{}[...]'",
                                    ident, ident
                                ),
                            ));
                            break; // One issue per line is enough
                        }
                    }
                }
            }
        }
        issues
    }
}

#[cfg(test)]
mod angle_bracket_tests {
    use super::*;

    fn check(input: &str) -> Vec<(String, String)> {
        AngleBracketPlugin.check(input)
    }

    #[test]
    fn catches_array_angle() {
        let issues = check("  def foo: () -> Array<Hash>");
        assert_eq!(issues.len(), 1);
        assert!(issues[0].1.contains("Array"));
    }

    #[test]
    fn catches_hash_angle() {
        let issues = check("  def bar: (Hash<String, Integer>) -> void");
        assert_eq!(issues.len(), 1);
        assert!(issues[0].1.contains("Hash"));
    }

    #[test]
    fn ignores_square_brackets() {
        let issues = check("  def foo: () -> Array[Hash[untyped, untyped]]");
        assert!(issues.is_empty());
    }

    #[test]
    fn ignores_class_inheritance() {
        // class Foo < Bar should not trigger
        let issues = check("class Foo < ApplicationRecord");
        assert!(issues.is_empty());
    }

    #[test]
    fn ignores_comments() {
        let issues = check("# @return Array<Hash>");
        assert!(issues.is_empty());
    }

    #[test]
    fn tracks_method_name() {
        let input = "  def my_method: () -> Array<String>";
        let issues = check(input);
        assert_eq!(issues[0].0, "my_method");
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
