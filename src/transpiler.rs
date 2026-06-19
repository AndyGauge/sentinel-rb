use anyhow::Context;
use std::fs;
use std::path::Path;
use tree_sitter::{Node, Parser};

/// Record types and method-parameter lists with more than this many top-level entries
/// are emitted on multiple lines in the generated `.rbs` output.
const MULTILINE_THRESHOLD: usize = 3;

struct SubModuleInfo {
    name: String,
    methods: Vec<(String, String)>,
    self_methods: Vec<(String, String)>,
    type_aliases: Vec<String>,
    attributes: Vec<(String, String, String)>,
}

struct ClassInfo {
    modules: Vec<String>,
    class_name: String,
    is_module: bool,
    methods: Vec<(String, String)>,
    self_methods: Vec<(String, String)>,
    type_aliases: Vec<String>,
    attributes: Vec<(String, String, String)>, // (attr_kind, attr_name, type)
    sub_modules: Vec<SubModuleInfo>,
}

pub struct SentinelTranspiler {
    parser: Parser,
    shared_paths: Vec<std::path::PathBuf>,
}

impl SentinelTranspiler {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        let lang = tree_sitter_ruby::language();
        parser
            .set_language(lang)
            .expect("Error loading Ruby grammar");

        Self {
            parser,
            shared_paths: Vec::new(),
        }
    }

    /// Set the directories to search for shared `.rbs` type files (used by `# @rbs import`).
    pub fn set_shared_paths(&mut self, paths: Vec<std::path::PathBuf>) {
        self.shared_paths = paths;
    }

    /// Resolve `# @rbs import <name>` by searching shared_paths for `<name>.rbs`,
    /// reading its contents, and returning the type aliases found inside.
    /// Transitively resolves nested type references with cycle detection: if an
    /// imported type references another type defined in `sig/shared/`, that type
    /// is also imported. Dependencies are emitted before dependents (reverse-topological order).
    fn resolve_import(shared_paths: &[std::path::PathBuf], name: &str) -> Vec<String> {
        let mut resolved: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        // Build the index of all available type names once, then pass it through
        // the recursion so we don't re-read every .rbs file at each level of DFS.
        let available: std::collections::BTreeSet<String> = Self::index_shared_types(shared_paths);
        Self::resolve_import_recursive(shared_paths, name, &mut resolved, &mut seen, &available);
        resolved
    }

    fn resolve_import_recursive(
        shared_paths: &[std::path::PathBuf],
        name: &str,
        resolved: &mut Vec<String>,
        seen: &mut std::collections::HashSet<String>,
        available: &std::collections::BTreeSet<String>,
    ) {
        // Insert before resolution for cycle detection and to suppress
        // repeated "Could not resolve" warnings for the same name.
        if !seen.insert(name.to_string()) {
            return; // already resolved or in progress — avoid cycles
        }

        // Find and parse the requested type file
        let aliases = Self::resolve_single_import(shared_paths, name);
        if aliases.is_empty() {
            return;
        }

        // Scan the aliases for references to other shared types.
        // Co-located types (same file) are NOT pre-marked in seen, so if alias A
        // references co-located alias B, the recursion resolves and emits B first,
        // guaranteeing topological order regardless of file order.
        for alias in &aliases {
            let defined_name = alias
                .strip_prefix("type ")
                .and_then(|rest| rest.split_whitespace().next())
                .unwrap_or("");

            for available_name in available {
                if available_name != defined_name && !seen.contains(available_name) {
                    if Self::references_type(alias, available_name) {
                        Self::resolve_import_recursive(shared_paths, available_name, resolved, seen, available);
                    }
                }
            }
        }

        // Emit aliases, skipping co-located types already emitted by recursive calls.
        // The primary type (name) was inserted into seen at entry — use defined == name
        // to ensure it's always emitted. For co-located types, seen.insert returns false
        // if already resolved above, preventing duplicates.
        for alias in aliases {
            let defined = alias
                .strip_prefix("type ")
                .and_then(|rest| rest.split_whitespace().next())
                .unwrap_or("");
            if defined == name || seen.insert(defined.to_string()) {
                resolved.push(alias);
            }
        }
    }

    /// Resolve a single import by name. First tries `<name>.rbs` (exact file match),
    /// then scans all `.rbs` files for a `type <name> = ...` definition.
    fn resolve_single_import(shared_paths: &[std::path::PathBuf], name: &str) -> Vec<String> {
        // 1. Try exact file match: sig/shared/<name>.rbs
        for dir in shared_paths {
            let file = dir.join(format!("{}.rbs", name));
            if let Ok(contents) = fs::read_to_string(&file) {
                return Self::parse_shared_rbs(&contents);
            }
        }

        // 2. Scan all .rbs files for a type definition matching the name.
        //    Parse each file once and filter — avoids reading the same file twice
        //    (once for the line scan, once for parse_shared_rbs).
        let target_prefix = format!("{} =", name);
        for dir in shared_paths {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |e| e == "rbs") {
                        if let Ok(contents) = fs::read_to_string(&path) {
                            let all_aliases = Self::parse_shared_rbs(&contents);
                            let matching: Vec<String> = all_aliases
                                .into_iter()
                                .filter(|a| {
                                    a.strip_prefix("type ")
                                        .map_or(false, |rest| rest.starts_with(&target_prefix))
                                })
                                .collect();
                            if !matching.is_empty() {
                                return matching;
                            }
                        }
                    }
                }
            }
        }

        eprintln!("  [import] Could not resolve '{}' from shared paths", name);
        Vec::new()
    }

    /// Collect all type names defined across all `.rbs` files in shared_paths.
    fn index_shared_types(shared_paths: &[std::path::PathBuf]) -> std::collections::BTreeSet<String> {
        let mut names = std::collections::BTreeSet::new();
        for dir in shared_paths {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |e| e == "rbs") {
                        if let Ok(contents) = fs::read_to_string(&path) {
                            for line in contents.lines() {
                                let trimmed = line.trim();
                                if let Some(rest) = trimmed.strip_prefix("type ") {
                                    if let Some(name) = rest.split_whitespace().next() {
                                        names.insert(name.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        names
    }

    /// Check whether an alias body references a given type name.
    /// Matches the name as a whole word (not as a substring of another identifier).
    fn references_type(alias: &str, type_name: &str) -> bool {
        let name_bytes = type_name.as_bytes();
        let alias_bytes = alias.as_bytes();
        let name_len = name_bytes.len();

        let mut i = 0;
        while i + name_len <= alias_bytes.len() {
            if &alias_bytes[i..i + name_len] == name_bytes {
                // Check word boundary before
                let before_ok = i == 0 || (!alias_bytes[i - 1].is_ascii_alphanumeric() && alias_bytes[i - 1] != b'_');
                // Check word boundary after
                let after_pos = i + name_len;
                let after_ok = after_pos >= alias_bytes.len()
                    || (!alias_bytes[after_pos].is_ascii_alphanumeric() && alias_bytes[after_pos] != b'_');
                if before_ok && after_ok {
                    return true;
                }
            }
            i += 1;
        }
        false
    }

    /// Parse a bare `.rbs` file from `sig/shared/` and extract type alias definitions.
    /// Expected format: `type foo = { ... }` (may span multiple lines).
    /// Strips `@desc(...)` and similar annotations before returning.
    fn parse_shared_rbs(contents: &str) -> Vec<String> {
        let mut aliases = Vec::new();
        let mut current: Option<String> = None;

        let finalize = |raw: String, out: &mut Vec<String>| {
            if Self::is_balanced(&raw) {
                let stripped = Self::strip_annotations(&raw);
                out.push(format!("type {}", stripped));
            }
        };

        for line in contents.lines() {
            let trimmed = line.trim();

            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            if let Some(rest) = trimmed.strip_prefix("type ") {
                if let Some(prev) = current.take() {
                    finalize(prev, &mut aliases);
                }
                current = Some(rest.to_string());
            } else if let Some(ref mut cur) = current {
                cur.push(' ');
                cur.push_str(trimmed);
            }
        }

        if let Some(prev) = current {
            finalize(prev, &mut aliases);
        }

        aliases
    }

    /// Extract the text of a node from source
    fn node_text<'a>(source: &'a str, node: &Node) -> &'a str {
        &source[node.start_byte()..node.end_byte()]
    }

    /// Strip MCP-style annotations (`@desc(...)`, `@example(...)`, `@min(...)`,
    /// `@format(...)`, `@requires(...)`, etc.) from a `#:` signature. These are
    /// valid in Ruby comments but not in RBS — Steep refuses to parse them.
    /// Matches `@<word>(...)` where the parens are balanced.
    fn strip_annotations(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = String::with_capacity(s.len());
        let mut last_copy = 0;
        let mut i = 0;
        let mut stripped = false;
        while i < bytes.len() {
            if bytes[i] == b'@' {
                let mut j = i + 1;
                while j < bytes.len()
                    && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
                {
                    j += 1;
                }
                if j > i + 1 && j < bytes.len() && bytes[j] == b'(' {
                    let mut depth = 1usize;
                    let mut k = j + 1;
                    while k < bytes.len() && depth > 0 {
                        match bytes[k] {
                            b'(' => depth += 1,
                            b')' => depth -= 1,
                            _ => {}
                        }
                        k += 1;
                    }
                    if depth == 0 {
                        out.push_str(&s[last_copy..i]);
                        i = k;
                        last_copy = k;
                        stripped = true;
                        continue;
                    }
                }
            }
            i += 1;
        }

        if !stripped {
            return s.to_string();
        }

        out.push_str(&s[last_copy..]);

        // Cleanup pass: collapse whitespace runs, drop spaces sitting next to
        // brackets or commas, and remove trailing commas left dangling against
        // a closing bracket once the annotation that followed them is gone.
        let chars: Vec<char> = out.chars().collect();
        let mut cleaned = String::with_capacity(out.len());
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c.is_whitespace() {
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                let next = chars.get(i).copied();
                let last = cleaned.chars().last();
                let drop_after_open = matches!(last, Some('(') | Some('{') | Some('['));
                let drop_before_close =
                    matches!(next, Some(',') | Some(')') | Some('}') | Some(']'));
                if next.is_some() && !drop_after_open && !drop_before_close {
                    cleaned.push(' ');
                }
            } else if c == ',' {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if matches!(chars.get(j), Some(')') | Some('}') | Some(']')) {
                    i = j;
                    continue;
                }
                cleaned.push(c);
                i += 1;
            } else {
                cleaned.push(c);
                i += 1;
            }
        }

        cleaned
    }

    /// Check if braces/brackets/parens are balanced and correctly matched
    fn is_balanced(s: &str) -> bool {
        let mut stack: Vec<char> = Vec::new();

        for ch in s.chars() {
            match ch {
                '{' | '(' | '[' => stack.push(ch),
                '}' | ')' | ']' => {
                    let Some(open) = stack.pop() else {
                        return false;
                    };
                    if !matches!(
                        (open, ch),
                        ('{', '}') | ('[', ']') | ('(', ')')
                    ) {
                        return false;
                    }
                }
                _ => {}
            }
        }

        stack.is_empty()
    }

    /// Collect structure from the AST: module nesting, class name, and annotated methods.
    fn collect_structure(source: &str, root: Node, shared_paths: &[std::path::PathBuf]) -> ClassInfo {
        let mut module_stack = Vec::new();
        let mut info = ClassInfo {
            modules: Vec::new(),
            class_name: "UnknownClass".to_string(),
            is_module: false,
            methods: Vec::new(),
            self_methods: Vec::new(),
            type_aliases: Vec::new(),
            attributes: Vec::new(),
            sub_modules: Vec::new(),
        };
        Self::walk(source, root, &mut module_stack, &mut info, shared_paths);
        info
    }

    /// Flatten a node's children, inlining body_statement children
    fn flatten_children<'a>(node: Node<'a>) -> Vec<Node<'a>> {
        let mut result = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "body_statement" {
                let mut bs_cursor = child.walk();
                for bs_child in child.children(&mut bs_cursor) {
                    result.push(bs_child);
                }
            } else {
                result.push(child);
            }
        }
        result
    }

    /// Scan a sequence of sibling nodes for annotated methods, type aliases, and attributes
    fn scan_body(
        source: &str,
        children: &[Node],
        info: &mut ClassInfo,
        singleton_context: bool,
        shared_paths: &[std::path::PathBuf],
    ) {
        let mut pending_annotation: Option<String> = None;
        let mut pending_type_alias: Option<String> = None;

        for &child in children {
            // Finalize pending type alias if this node doesn't continue it
            if pending_type_alias.is_some() {
                let continues = child.kind() == "comment" && {
                    let text = Self::node_text(source, &child);
                    !text.starts_with("# @rbs type ")
                        && !text.starts_with("#: ")
                        && text
                            .strip_prefix('#')
                            .map(|c| {
                                let t = c.trim();
                                t.starts_with('|')
                                    || !Self::is_balanced(
                                        pending_type_alias.as_ref().unwrap(),
                                    )
                                    || pending_type_alias
                                        .as_ref()
                                        .unwrap()
                                        .ends_with('|')
                            })
                            .unwrap_or(false)
                };
                if !continues {
                    let alias = pending_type_alias.take().unwrap();
                    if Self::is_balanced(&alias) && !alias.ends_with('|') {
                        let alias = Self::strip_annotations(&alias);
                        info.type_aliases.push(format!("type {}", alias));
                    }
                }
            }

            match child.kind() {
                "comment" => {
                    let text = Self::node_text(source, &child);

                    // Check for @rbs type alias
                    if let Some(rest) = text.strip_prefix("# @rbs type ") {
                        pending_type_alias = Some(rest.trim().to_string());
                        pending_annotation = None;
                    } else if let Some(name) = text.strip_prefix("# @rbs import ") {
                        // Import shared type(s) from sig/shared/<name>.rbs
                        let name = name.trim();
                        let imported = Self::resolve_import(shared_paths, name);
                        for alias in imported {
                            info.type_aliases.push(alias);
                        }
                        pending_annotation = None;
                    } else if let Some(ref mut alias) = pending_type_alias {
                        // Continue multi-line type alias
                        if let Some(cont) = text.strip_prefix('#') {
                            let trimmed = cont.trim();
                            if !trimmed.is_empty() {
                                alias.push(' ');
                                alias.push_str(trimmed);
                            }
                        } else {
                            pending_type_alias = None;
                        }
                    } else if let Some(sig) = text.strip_prefix("#: ") {
                        let trimmed = sig.trim();
                        if let Some(ref mut ann) = pending_annotation {
                            if !Self::is_balanced(ann) {
                                // Continue multi-line #: annotation
                                ann.push(' ');
                                ann.push_str(trimmed);
                            } else {
                                // Previous annotation was complete; start fresh
                                pending_annotation = Some(trimmed.to_string());
                            }
                        } else {
                            pending_annotation = Some(trimmed.to_string());
                        }
                    } else {
                        pending_annotation = None;
                    }
                }
                "method" => {
                    pending_type_alias = None;
                    if let Some(sig) = pending_annotation.take() {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let method_name =
                                Self::node_text(source, &name_node).to_string();
                            let sig = Self::strip_annotations(&sig);
                            if singleton_context {
                                info.self_methods.push((method_name, sig));
                            } else {
                                info.methods.push((method_name, sig));
                            }
                        }
                    }
                }
                "singleton_method" => {
                    pending_type_alias = None;
                    if let Some(sig) = pending_annotation.take() {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let method_name =
                                Self::node_text(source, &name_node).to_string();
                            let sig = Self::strip_annotations(&sig);
                            info.self_methods.push((method_name, sig));
                        }
                    }
                }
                "singleton_class" => {
                    // class << self — methods inside are class methods
                    let inner_children = Self::flatten_children(child);
                    Self::scan_body(source, &inner_children, info, true, shared_paths);
                    pending_annotation = None;
                    pending_type_alias = None;
                }
                "call" => {
                    pending_type_alias = None;
                    // Check for attr_reader, attr_writer, attr_accessor
                    if let Some(method_node) = child.child_by_field_name("method") {
                        let method_name = Self::node_text(source, &method_node);
                        if matches!(
                            method_name,
                            "attr_reader" | "attr_writer" | "attr_accessor"
                        ) {
                            if let Some(type_sig) = pending_annotation.take() {
                                let type_sig = Self::strip_annotations(&type_sig);
                                if let Some(args_node) =
                                    child.child_by_field_name("arguments")
                                {
                                    let mut args_cursor = args_node.walk();
                                    for arg in args_node.children(&mut args_cursor) {
                                        if arg.kind() == "simple_symbol" {
                                            let sym_text =
                                                Self::node_text(source, &arg);
                                            let attr_name =
                                                sym_text.trim_start_matches(':');
                                            info.attributes.push((
                                                method_name.to_string(),
                                                attr_name.to_string(),
                                                type_sig.clone(),
                                            ));
                                        }
                                    }
                                }
                            }
                        } else {
                            pending_annotation = None;
                        }
                    } else {
                        pending_annotation = None;
                    }
                }
                _ => {
                    if child.kind() != "superclass" {
                        pending_annotation = None;
                        pending_type_alias = None;
                    }
                }
            }
        }

        // Finalize any remaining pending type alias
        if let Some(alias) = pending_type_alias {
            if Self::is_balanced(&alias) && !alias.ends_with('|') {
                let alias = Self::strip_annotations(&alias);
                info.type_aliases.push(format!("type {}", alias));
            }
        }
    }

    fn walk(
        source: &str,
        node: Node,
        module_stack: &mut Vec<String>,
        info: &mut ClassInfo,
        shared_paths: &[std::path::PathBuf],
    ) {
        match node.kind() {
            "module" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::node_text(source, &name_node);
                    let segments: Vec<&str> = name.split("::").collect();
                    let pushed = segments.len();
                    for seg in &segments {
                        module_stack.push(seg.to_string());
                    }

                    let mut cursor = node.walk();
                    for child in node.children(&mut cursor) {
                        Self::walk(source, child, module_stack, info, shared_paths);
                    }

                    // If no class was found inside, check if this module
                    // itself contains annotated content (e.g. concerns)
                    if info.class_name == "UnknownClass" {
                        let children = Self::flatten_children(node);
                        Self::scan_body(source, &children, info, false, shared_paths);
                        if !info.methods.is_empty()
                            || !info.self_methods.is_empty()
                            || !info.type_aliases.is_empty()
                            || !info.attributes.is_empty()
                        {
                            for _ in 0..pushed {
                                module_stack.pop();
                            }
                            info.modules = module_stack.clone();
                            info.class_name = name.to_string();
                            info.is_module = true;
                            return;
                        }
                    } else if info.is_module {
                        // A nested module (e.g. ClassMethods) was already captured.
                        // Check if the parent module also has its own annotated methods.
                        // If so, demote the nested module to a sub_module and promote
                        // the parent as the primary module.
                        let mut parent_info = ClassInfo {
                            modules: Vec::new(),
                            class_name: "UnknownClass".to_string(),
                            is_module: false,
                            methods: Vec::new(),
                            self_methods: Vec::new(),
                            type_aliases: Vec::new(),
                            attributes: Vec::new(),
                            sub_modules: Vec::new(),
                        };
                        let children = Self::flatten_children(node);
                        Self::scan_body(source, &children, &mut parent_info, false, shared_paths);
                        if !parent_info.methods.is_empty()
                            || !parent_info.self_methods.is_empty()
                            || !parent_info.type_aliases.is_empty()
                            || !parent_info.attributes.is_empty()
                        {
                            // Move the previously captured nested module into sub_modules
                            let nested = SubModuleInfo {
                                name: info.class_name.clone(),
                                methods: info.methods.drain(..).collect(),
                                self_methods: info.self_methods.drain(..).collect(),
                                type_aliases: info.type_aliases.drain(..).collect(),
                                attributes: info.attributes.drain(..).collect(),
                            };
                            info.sub_modules.push(nested);
                            // Replace with parent module's content
                            info.methods = parent_info.methods;
                            info.self_methods = parent_info.self_methods;
                            info.type_aliases = parent_info.type_aliases;
                            info.attributes = parent_info.attributes;
                            for _ in 0..pushed {
                                module_stack.pop();
                            }
                            info.modules = module_stack.clone();
                            info.class_name = name.to_string();
                            info.is_module = true;
                            return;
                        }
                    }

                    for _ in 0..pushed {
                        module_stack.pop();
                    }
                    return;
                }
            }
            "class" => {
                // Snapshot the current module stack — this is the nesting at class definition
                info.modules = module_stack.clone();
                info.is_module = false;

                if let Some(name_node) = node.child_by_field_name("name") {
                    info.class_name = Self::node_text(source, &name_node).to_string();
                }

                // Clear any data from a previously scanned module container
                info.methods.clear();
                info.self_methods.clear();
                info.type_aliases.clear();
                info.attributes.clear();

                // Flatten direct children + body_statement children, then scan
                let children = Self::flatten_children(node);
                Self::scan_body(source, &children, info, false, shared_paths);

                return;
            }
            _ => {}
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            Self::walk(source, child, module_stack, info, shared_paths);
        }
    }

    /// Returns true if the generated RBS has meaningful content worth writing
    pub fn has_content(rbs: &str) -> bool {
        rbs.contains("def ") || rbs.contains("type ") || rbs.contains("attr_")
    }

    /// Split `s` at top-level commas (ignoring commas nested inside `{}`, `()`, `[]`).
    /// Leading/trailing whitespace is trimmed from each segment; empty segments are dropped.
    fn split_top_level_commas(s: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut depth = 0usize;
        let mut current = String::new();
        for ch in s.chars() {
            match ch {
                '{' | '(' | '[' => {
                    depth += 1;
                    current.push(ch);
                }
                '}' | ')' | ']' => {
                    depth = depth.saturating_sub(1);
                    current.push(ch);
                }
                ',' if depth == 0 => {
                    let trimmed = current.trim().to_string();
                    current.clear();
                    if !trimmed.is_empty() {
                        result.push(trimmed);
                    }
                }
                _ => current.push(ch),
            }
        }
        let last = current.trim().to_string();
        if !last.is_empty() {
            result.push(last);
        }
        result
    }

    /// If `s` looks like `{ k1: T1, k2: T2, ... }` and has more than
    /// [`MULTILINE_THRESHOLD`] top-level entries, return a multi-line version;
    /// otherwise return `s` unchanged.
    ///
    /// * `field_indent`  – indentation prepended to each field line.
    /// * `close_indent`  – indentation prepended to the closing `}`.
    fn maybe_format_record(s: &str, field_indent: &str, close_indent: &str) -> String {
        let trimmed = s.trim();
        let inner = match trimmed
            .strip_prefix('{')
            .and_then(|t| t.strip_suffix('}'))
        {
            Some(i) => i.trim(),
            None => return s.to_string(),
        };
        let entries = Self::split_top_level_commas(inner);
        if entries.len() <= MULTILINE_THRESHOLD {
            return s.to_string();
        }
        let mut out = String::from("{\n");
        for entry in &entries {
            out.push_str(field_indent);
            out.push_str(entry);
            out.push_str(",\n");
        }
        out.push_str(close_indent);
        out.push('}');
        out
    }

    /// If `sig` starts with `(...)` and has more than [`MULTILINE_THRESHOLD`]
    /// top-level parameters, split the parameter list onto separate lines;
    /// otherwise return `sig` unchanged.
    ///
    /// * `param_indent`  – indentation prepended to each parameter line.
    /// * `close_indent`  – indentation prepended to the closing `)`.
    fn maybe_format_sig(sig: &str, param_indent: &str, close_indent: &str) -> String {
        let trimmed = sig.trim();
        if !trimmed.starts_with('(') {
            return sig.to_string();
        }

        // Find the matching closing parenthesis.
        let mut depth = 0usize;
        let mut close_pos = None;
        for (i, ch) in trimmed.char_indices() {
            match ch {
                '(' | '{' | '[' => depth += 1,
                ')' | '}' | ']' => {
                    depth -= 1;
                    if depth == 0 {
                        close_pos = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }

        let close_pos = match close_pos {
            Some(p) => p,
            None => return sig.to_string(),
        };

        let inner = trimmed[1..close_pos].trim();
        let after = trimmed[close_pos + 1..].trim(); // e.g. " -> ReturnType"

        let entries = Self::split_top_level_commas(inner);
        if entries.len() <= MULTILINE_THRESHOLD {
            return sig.to_string();
        }

        let mut out = String::from("(\n");
        for entry in &entries {
            out.push_str(param_indent);
            out.push_str(entry);
            out.push_str(",\n");
        }
        out.push_str(close_indent);
        out.push(')');
        if !after.is_empty() {
            out.push(' ');
            out.push_str(after);
        }
        out
    }

    pub fn transpile_file(
        &mut self,
        rb_path: &Path,
    ) -> anyhow::Result<String> {
        let source = fs::read_to_string(rb_path)?;
        let tree = self
            .parser
            .parse(&source, None)
            .context("Failed to parse")?;

        let info = Self::collect_structure(&source, tree.root_node(), &self.shared_paths);

        let mut rbs_output = String::new();
        rbs_output.push_str("# Generated by Sentinel - Do not edit manually\n\n");

        // Emit nested module/class structure
        let depth = info.modules.len();
        for (i, module_name) in info.modules.iter().enumerate() {
            let indent = "  ".repeat(i);
            rbs_output.push_str(&format!("{}module {}\n", indent, module_name));
        }

        let class_indent = "  ".repeat(depth);
        let member_indent = "  ".repeat(depth + 1);
        let keyword = if info.is_module { "module" } else { "class" };
        rbs_output.push_str(&format!("{}{} {}\n", class_indent, keyword, info.class_name));

        // Type aliases
        for alias in &info.type_aliases {
            // If the RHS of the alias is a record type with many entries, format it
            // on multiple lines so that the generated file stays readable.
            let formatted = if let Some(eq_pos) = alias.find(" = ") {
                let lhs = &alias[..eq_pos + 3]; // "type foo = "
                let rhs = alias[eq_pos + 3..].trim();
                let field_indent = format!("{}  ", member_indent);
                let formatted_rhs =
                    Self::maybe_format_record(rhs, &field_indent, &member_indent);
                format!("{}{}", lhs, formatted_rhs)
            } else {
                alias.clone()
            };
            rbs_output.push_str(&format!("{}{}\n", member_indent, formatted));
        }

        // Attributes
        for (kind, name, type_sig) in &info.attributes {
            rbs_output.push_str(&format!(
                "{}{} {}: {}\n",
                member_indent, kind, name, type_sig
            ));
        }

        // Class methods
        for (name, sig) in &info.self_methods {
            let param_indent = format!("{}  ", member_indent);
            let formatted_sig = Self::maybe_format_sig(sig, &param_indent, &member_indent);
            rbs_output.push_str(&format!(
                "{}def self.{}: {}\n",
                member_indent, name, formatted_sig
            ));
        }

        // Instance methods
        for (name, sig) in &info.methods {
            let param_indent = format!("{}  ", member_indent);
            let formatted_sig = Self::maybe_format_sig(sig, &param_indent, &member_indent);
            rbs_output.push_str(&format!("{}def {}: {}\n", member_indent, name, formatted_sig));
        }

        // Sub-modules (e.g. ClassMethods inside a concern)
        for sub in &info.sub_modules {
            let sub_indent = "  ".repeat(depth + 1);
            let sub_member_indent = "  ".repeat(depth + 2);
            rbs_output.push_str(&format!("{}module {}\n", sub_indent, sub.name));
            for alias in &sub.type_aliases {
                let formatted = if let Some(eq_pos) = alias.find(" = ") {
                    let lhs = &alias[..eq_pos + 3];
                    let rhs = alias[eq_pos + 3..].trim();
                    let field_indent = format!("{}  ", sub_member_indent);
                    let formatted_rhs =
                        Self::maybe_format_record(rhs, &field_indent, &sub_member_indent);
                    format!("{}{}", lhs, formatted_rhs)
                } else {
                    alias.clone()
                };
                rbs_output.push_str(&format!("{}{}\n", sub_member_indent, formatted));
            }
            for (kind, name, type_sig) in &sub.attributes {
                rbs_output.push_str(&format!(
                    "{}{} {}: {}\n",
                    sub_member_indent, kind, name, type_sig
                ));
            }
            for (name, sig) in &sub.self_methods {
                let param_indent = format!("{}  ", sub_member_indent);
                let formatted_sig =
                    Self::maybe_format_sig(sig, &param_indent, &sub_member_indent);
                rbs_output.push_str(&format!(
                    "{}def self.{}: {}\n",
                    sub_member_indent, name, formatted_sig
                ));
            }
            for (name, sig) in &sub.methods {
                let param_indent = format!("{}  ", sub_member_indent);
                let formatted_sig =
                    Self::maybe_format_sig(sig, &param_indent, &sub_member_indent);
                rbs_output.push_str(&format!(
                    "{}def {}: {}\n",
                    sub_member_indent, name, formatted_sig
                ));
            }
            rbs_output.push_str(&format!("{}end\n", sub_indent));
        }

        rbs_output.push_str(&format!("{}end\n", class_indent));

        for i in (0..depth).rev() {
            let indent = "  ".repeat(i);
            rbs_output.push_str(&format!("{}end\n", indent));
        }

        Ok(rbs_output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_simple_class_name() {
        let test_file = Path::new("/tmp/test_simple_class.rb");
        fs::write(test_file, "class User < ApplicationRecord\n  #: () -> String\n  def name\n  end\nend\n").unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        // No modules, just class
        assert!(result.contains("class User\n"), "Got: {}", result);
        assert!(result.contains("  def name: () -> String"), "Got: {}", result);
    }

    #[test]
    fn test_scope_resolution_class_name() {
        let test_file = Path::new("/tmp/test_scope_resolution.rb");
        fs::write(test_file, "class ApplicantFilter::Set < ApplicationRecord\n  #: (String) -> String\n  def combinator_for(key)\n  end\nend\n").unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        // Compact syntax: no enclosing modules, class name keeps ::
        assert!(result.contains("class ApplicantFilter::Set\n"), "Got: {}", result);
    }

    #[test]
    fn test_nested_modules() {
        let test_file = Path::new("/tmp/test_nested_modules.rb");
        fs::write(test_file, "module Tool\n  module IdleRuleHandlers\n    class Set < Tool::WorkflowBase\n      #: () -> void\n      def perform\n      end\n    end\n  end\nend\n").unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        // Must emit nested modules, not flat qualified name
        assert!(result.contains("module Tool\n"), "Expected module Tool, got: {}", result);
        assert!(result.contains("  module IdleRuleHandlers\n"), "Expected module IdleRuleHandlers, got: {}", result);
        assert!(result.contains("    class Set\n"), "Expected class Set, got: {}", result);
        assert!(result.contains("      def perform: () -> void"), "Expected indented method, got: {}", result);
        // Verify closing ends
        assert!(result.contains("    end\n  end\nend\n"), "Expected nested ends, got: {}", result);
    }

    #[test]
    fn test_single_module_wrap() {
        let test_file = Path::new("/tmp/test_single_module.rb");
        fs::write(test_file, "module Admin\n  class UsersController\n    #: (Integer) -> User\n    def show(id)\n    end\n  end\nend\n").unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(result.contains("module Admin\n"), "Got: {}", result);
        assert!(result.contains("  class UsersController\n"), "Got: {}", result);
        assert!(result.contains("    def show: (Integer) -> User"), "Got: {}", result);
    }

    #[test]
    fn test_multiple_methods() {
        let test_file = Path::new("/tmp/test_multi_methods.rb");
        fs::write(test_file, "module Tool\n  class Base < Tool::WorkflowBase\n    #: (current_user: User, current_account: Account, params: ActionController::Parameters) -> void\n    def initialize(current_user:, current_account:, params:)\n      @current_user = current_user\n    end\n\n    #: () -> Hash[Symbol, untyped]\n    def call\n      raise NotImplementedError\n    end\n\n    #: () -> Hash[Symbol, untyped]\n    def usage_info\n      {}\n    end\n  end\nend\n").unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(result.contains("module Tool\n"), "Got: {}", result);
        assert!(result.contains("  class Base\n"), "Got: {}", result);
        assert!(result.contains("def initialize: (current_user: User, current_account: Account, params: ActionController::Parameters) -> void"), "Missing initialize, got: {}", result);
        assert!(result.contains("def call: () -> Hash[Symbol, untyped]"), "Missing call, got: {}", result);
        assert!(result.contains("def usage_info: () -> Hash[Symbol, untyped]"), "Missing usage_info, got: {}", result);
    }

    #[test]
    fn test_module_with_scope_resolution_class() {
        let test_file = Path::new("/tmp/test_module_scope.rb");
        fs::write(test_file, "module Api\n  class V2::UsersController\n    #: () -> void\n    def index\n    end\n  end\nend\n").unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        // Module Api wraps, class keeps V2:: compact form
        assert!(result.contains("module Api\n"), "Got: {}", result);
        assert!(result.contains("  class V2::UsersController\n"), "Got: {}", result);
    }

    // --- New tests for issue #1 features ---

    #[test]
    fn test_singleton_method() {
        let test_file = Path::new("/tmp/test_singleton_method.rb");
        fs::write(
            test_file,
            "class Foo\n  #: (String) -> String\n  def self.call(name)\n    name.upcase\n  end\nend\n",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("def self.call: (String) -> String"),
            "Expected self.call, got: {}",
            result
        );
    }

    #[test]
    fn test_singleton_class_block() {
        let test_file = Path::new("/tmp/test_singleton_class.rb");
        fs::write(
            test_file,
            "class Foo\n  class << self\n    #: (String) -> String\n    def call(name)\n      name.upcase\n    end\n  end\nend\n",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("def self.call: (String) -> String"),
            "Expected self.call from class << self, got: {}",
            result
        );
    }

    #[test]
    fn test_singleton_class_multiple_methods() {
        let test_file = Path::new("/tmp/test_singleton_class_multi.rb");
        fs::write(
            test_file,
            "class Service\n  class << self\n    #: (String) -> String\n    def description\n    end\n\n    #: () -> Hash[Symbol, untyped]\n    def input_schema\n    end\n  end\nend\n",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("def self.description: (String) -> String"),
            "Missing self.description, got: {}",
            result
        );
        assert!(
            result.contains("def self.input_schema: () -> Hash[Symbol, untyped]"),
            "Missing self.input_schema, got: {}",
            result
        );
    }

    #[test]
    fn test_type_alias_single_line() {
        let test_file = Path::new("/tmp/test_type_alias.rb");
        fs::write(
            test_file,
            "class Foo\n  # @rbs type error_code = \"not_found\" | \"invalid\" | \"denied\"\nend\n",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("type error_code = \"not_found\" | \"invalid\" | \"denied\""),
            "Expected type alias, got: {}",
            result
        );
    }

    #[test]
    fn test_type_alias_multiline() {
        let test_file = Path::new("/tmp/test_type_alias_multi.rb");
        fs::write(
            test_file,
            "class Foo\n  # @rbs type error = {\n  #   success: false,\n  #   error: { code: String, message: String }\n  # }\nend\n",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("type error = {"),
            "Expected multiline type alias, got: {}",
            result
        );
        assert!(
            result.contains("success: false"),
            "Expected multiline type alias content, got: {}",
            result
        );
    }

    #[test]
    fn test_attr_reader() {
        let test_file = Path::new("/tmp/test_attr_reader.rb");
        fs::write(
            test_file,
            "class Foo\n  #: String\n  attr_reader :name\nend\n",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("attr_reader name: String"),
            "Expected attr_reader, got: {}",
            result
        );
    }

    #[test]
    fn test_attr_accessor() {
        let test_file = Path::new("/tmp/test_attr_accessor.rb");
        fs::write(
            test_file,
            "class Foo\n  #: Integer\n  attr_accessor :id\nend\n",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("attr_accessor id: Integer"),
            "Expected attr_accessor, got: {}",
            result
        );
    }

    #[test]
    fn test_attr_writer() {
        let test_file = Path::new("/tmp/test_attr_writer.rb");
        fs::write(
            test_file,
            "class Foo\n  #: String\n  attr_writer :email\nend\n",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("attr_writer email: String"),
            "Expected attr_writer, got: {}",
            result
        );
    }

    #[test]
    fn test_attr_multiple_symbols() {
        let test_file = Path::new("/tmp/test_attr_multi.rb");
        fs::write(
            test_file,
            "class Foo\n  #: String\n  attr_reader :name, :email\nend\n",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("attr_reader name: String"),
            "Expected attr_reader name, got: {}",
            result
        );
        assert!(
            result.contains("attr_reader email: String"),
            "Expected attr_reader email, got: {}",
            result
        );
    }

    #[test]
    fn test_mixed_self_and_instance_methods() {
        let test_file = Path::new("/tmp/test_mixed_methods.rb");
        fs::write(
            test_file,
            "class Service\n  #: (String) -> Service\n  def self.call(name)\n  end\n\n  #: () -> void\n  def perform\n  end\nend\n",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("def self.call: (String) -> Service"),
            "Missing self.call, got: {}",
            result
        );
        assert!(
            result.contains("def perform: () -> void"),
            "Missing perform, got: {}",
            result
        );
    }

    #[test]
    fn test_all_features_combined() {
        let test_file = Path::new("/tmp/test_all_features.rb");
        fs::write(
            test_file,
            "\
class MCP::Tool
  # @rbs type result = { success: bool, data: untyped }

  #: String
  attr_reader :name

  #: (Hash[Symbol, untyped]) -> result
  def self.call(params)
  end

  #: () -> void
  def validate
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(result.contains("type result = { success: bool, data: untyped }"), "Missing type alias, got: {}", result);
        assert!(result.contains("attr_reader name: String"), "Missing attr_reader, got: {}", result);
        assert!(result.contains("def self.call: (Hash[Symbol, untyped]) -> result"), "Missing self.call, got: {}", result);
        assert!(result.contains("def validate: () -> void"), "Missing validate, got: {}", result);
    }

    #[test]
    fn test_multiline_annotation() {
        let test_file = Path::new("/tmp/test_multiline_annotation.rb");
        fs::write(
            test_file,
            "\
class MultilineTest
  #: (
  #:   name: String,
  #:   age: Integer,
  #:   ?email: String?
  #: ) -> Hash[Symbol, untyped]
  def self.call(name:, age:, email: nil)
    { name: name, age: age }
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("def self.call: ( name: String, age: Integer, ?email: String? ) -> Hash[Symbol, untyped]"),
            "Expected joined multiline annotation, got: {}",
            result
        );
    }

    #[test]
    fn test_multiline_annotation_instance_method() {
        let test_file = Path::new("/tmp/test_multiline_instance.rb");
        fs::write(
            test_file,
            "\
class Processor
  #: (
  #:   Array[String],
  #:   Integer
  #: ) -> bool
  def run(items, limit)
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("def run: ( Array[String], Integer ) -> bool"),
            "Expected joined multiline annotation, got: {}",
            result
        );
    }

    #[test]
    fn test_multiline_annotation_does_not_merge_separate() {
        let test_file = Path::new("/tmp/test_no_merge_separate.rb");
        fs::write(
            test_file,
            "\
class Separate
  #: () -> String
  #: (Integer) -> void
  def overloaded
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        // The second balanced annotation should overwrite the first
        assert!(
            result.contains("def overloaded: (Integer) -> void"),
            "Expected second annotation to win, got: {}",
            result
        );
    }

    #[test]
    fn test_multiline_annotation_class_self_block() {
        let test_file = Path::new("/tmp/test_multiline_class_self.rb");
        fs::write(
            test_file,
            "\
class Builder
  class << self
    #: (
    #:   String,
    #:   ?config: Hash[Symbol, untyped]
    #: ) -> Builder
    def create(name, config: {})
    end
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("def self.create: ( String, ?config: Hash[Symbol, untyped] ) -> Builder"),
            "Expected multiline annotation in class << self, got: {}",
            result
        );
    }

    #[test]
    fn test_type_alias_multiline_union() {
        let test_file = Path::new("/tmp/test_type_alias_union.rb");
        fs::write(
            test_file,
            "\
class Applicant
  # @rbs type error_code = \"applicant_not_found\"
  #                      | \"stage_transition_invalid\"
  #                      | \"permission_denied\"
  #                      | \"applicant_already_at_stage\"
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("type error_code = \"applicant_not_found\" | \"stage_transition_invalid\" | \"permission_denied\" | \"applicant_already_at_stage\""),
            "Expected multiline union type alias, got: {}",
            result
        );
    }

    #[test]
    fn test_type_alias_multiline_union_trailing_pipe() {
        let test_file = Path::new("/tmp/test_type_alias_union_trail.rb");
        fs::write(
            test_file,
            "\
class Applicant
  # @rbs type status = \"active\" |
  #   \"inactive\" |
  #   \"pending\"
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("type status = \"active\" | \"inactive\" | \"pending\""),
            "Expected multiline union type alias with trailing pipe, got: {}",
            result
        );
    }

    #[test]
    fn test_type_alias_multiline_union_then_method() {
        let test_file = Path::new("/tmp/test_type_alias_union_method.rb");
        fs::write(
            test_file,
            "\
class Foo
  # @rbs type error_code = \"not_found\"
  #                      | \"denied\"

  #: () -> error_code
  def check
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("type error_code = \"not_found\" | \"denied\""),
            "Expected union type alias, got: {}",
            result
        );
        assert!(
            result.contains("def check: () -> error_code"),
            "Expected method after type alias, got: {}",
            result
        );
    }

    #[test]
    fn test_type_alias_trailing_pipe_then_annotation() {
        let test_file = Path::new("/tmp/test_type_alias_trail_ann.rb");
        fs::write(
            test_file,
            "\
class Foo
  # @rbs type status = \"active\" |
  #   \"inactive\"
  #: () -> status
  def check
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("type status = \"active\" | \"inactive\""),
            "Expected trailing-pipe union type alias, got: {}",
            result
        );
        assert!(
            result.contains("def check: () -> status"),
            "Expected method annotation not swallowed by type alias, got: {}",
            result
        );
    }

    #[test]
    fn test_module_with_annotations() {
        let test_file = Path::new("/tmp/test_module_annotations.rb");
        fs::write(
            test_file,
            "\
module Tool
  module Concerns
    module ConceptResolvable
      extend ActiveSupport::Concern

      private

      #: (String concept_id) -> Hash[Symbol, untyped]
      def resolve_concept_id(concept_id)
      end
    end
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("module Tool\n"),
            "Expected module Tool, got: {}",
            result
        );
        assert!(
            result.contains("  module Concerns\n"),
            "Expected module Concerns, got: {}",
            result
        );
        assert!(
            result.contains("    module ConceptResolvable\n"),
            "Expected module ConceptResolvable (not class), got: {}",
            result
        );
        assert!(
            result.contains("def resolve_concept_id: (String concept_id) -> Hash[Symbol, untyped]"),
            "Expected method signature, got: {}",
            result
        );
    }

    #[test]
    fn test_module_single_level() {
        let test_file = Path::new("/tmp/test_module_single.rb");
        fs::write(
            test_file,
            "\
module Serializable
  #: () -> Hash[Symbol, untyped]
  def to_h
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("module Serializable\n"),
            "Expected module declaration, got: {}",
            result
        );
        assert!(
            !result.contains("class "),
            "Should not contain class keyword, got: {}",
            result
        );
        assert!(
            result.contains("def to_h: () -> Hash[Symbol, untyped]"),
            "Expected method, got: {}",
            result
        );
    }

    #[test]
    fn test_module_without_annotations_skipped() {
        let test_file = Path::new("/tmp/test_module_no_ann.rb");
        fs::write(
            test_file,
            "\
module Concerns
  module Loggable
    extend ActiveSupport::Concern
    def log(message)
    end
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            !SentinelTranspiler::has_content(&result),
            "Module without annotations should have no content, got: {}",
            result
        );
    }

    #[test]
    fn test_module_with_class_inside_uses_class() {
        let test_file = Path::new("/tmp/test_module_with_class.rb");
        fs::write(
            test_file,
            "\
module Admin
  class UsersController
    #: () -> void
    def index
    end
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("module Admin\n"),
            "Expected wrapper module, got: {}",
            result
        );
        assert!(
            result.contains("class UsersController\n"),
            "Expected class (not module) for inner class, got: {}",
            result
        );
    }

    #[test]
    fn test_annotated_module_then_class_uses_class() {
        let test_file = Path::new("/tmp/test_module_then_class.rb");
        fs::write(
            test_file,
            "\
module Helpers
  #: () -> String
  def helper_method
  end
end

class Service
  #: () -> void
  def perform
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("class Service\n"),
            "Expected class keyword for Service, got: {}",
            result
        );
        assert!(
            !result.contains("module Service"),
            "Service should not be emitted as module, got: {}",
            result
        );
        assert!(
            result.contains("def perform: () -> void"),
            "Expected Service's method, got: {}",
            result
        );
    }

    #[test]
    fn test_module_with_sub_module_class_methods() {
        let test_file = Path::new("/tmp/test_module_sub_module.rb");
        fs::write(
            test_file,
            "\
module Tool
  module Concerns
    module McpMigrated
      #: (server_context: untyped) -> void
      def initialize(server_context:)
      end

      module ClassMethods
        #: (current_user: untyped, current_account: untyped, params: untyped) -> instance
        def from_legacy(current_user:, current_account:, params:)
        end
      end
    end
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("module Tool\n"),
            "Expected module Tool, got: {}",
            result
        );
        assert!(
            result.contains("  module Concerns\n"),
            "Expected module Concerns, got: {}",
            result
        );
        assert!(
            result.contains("    module McpMigrated\n"),
            "Expected module McpMigrated, got: {}",
            result
        );
        assert!(
            result.contains("def initialize: (server_context: untyped) -> void"),
            "Expected initialize method on parent module, got: {}",
            result
        );
        assert!(
            result.contains("module ClassMethods\n"),
            "Expected ClassMethods sub-module, got: {}",
            result
        );
        assert!(
            result.contains("def from_legacy: (current_user: untyped, current_account: untyped, params: untyped) -> instance"),
            "Expected from_legacy in ClassMethods, got: {}",
            result
        );
    }

    #[test]
    fn test_has_content() {
        assert!(SentinelTranspiler::has_content("  def foo: () -> void\n"));
        assert!(SentinelTranspiler::has_content("  type error_code = String\n"));
        assert!(SentinelTranspiler::has_content("  attr_reader name: String\n"));
        assert!(!SentinelTranspiler::has_content("class Foo\nend\n"));
    }

    // --- Tests for multiline pretty-printing (> MULTILINE_THRESHOLD entries) ---

    #[test]
    fn test_record_type_alias_multiline_when_many_keys() {
        let test_file = Path::new("/tmp/test_record_multiline.rb");
        fs::write(
            test_file,
            "\
class Applicant
  # @rbs type applicant_local = {
  #   external_id: String,
  #   name: String,
  #   email: String,
  #   status: String,
  # }
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        // Should be on multiple lines since there are 4 keys (> 3)
        assert!(
            result.contains("type applicant_local = {\n"),
            "Expected opening brace on its own line, got: {}",
            result
        );
        assert!(
            result.contains("    external_id: String,\n"),
            "Expected external_id on its own indented line, got: {}",
            result
        );
        assert!(
            result.contains("    name: String,\n"),
            "Expected name on its own indented line, got: {}",
            result
        );
        assert!(
            result.contains("    email: String,\n"),
            "Expected email on its own indented line, got: {}",
            result
        );
        assert!(
            result.contains("    status: String,\n"),
            "Expected status on its own indented line, got: {}",
            result
        );
        assert!(
            result.contains("  }"),
            "Expected closing brace at member_indent, got: {}",
            result
        );
    }

    #[test]
    fn test_record_type_alias_single_line_when_few_keys() {
        let test_file = Path::new("/tmp/test_record_singleline.rb");
        fs::write(
            test_file,
            "\
class Foo
  # @rbs type pair = { key: String, value: Integer }
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        // Only 2 keys — should stay on one line
        assert!(
            result.contains("type pair = { key: String, value: Integer }"),
            "Expected single-line record with 2 keys, got: {}",
            result
        );
    }

    #[test]
    fn test_method_sig_multiline_when_many_params() {
        let test_file = Path::new("/tmp/test_sig_multiline.rb");
        fs::write(
            test_file,
            "\
class Handler
  #: (
  #:   external_id: String,
  #:   name: String,
  #:   email: String,
  #:   status: String
  #: ) -> void
  def call(external_id:, name:, email:, status:)
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        // 4 keyword params (> 3) — should be multi-line
        assert!(
            result.contains("def call: (\n"),
            "Expected opening paren on its own line, got: {}",
            result
        );
        assert!(
            result.contains("    external_id: String,\n"),
            "Expected external_id on its own indented line, got: {}",
            result
        );
        assert!(
            result.contains("    name: String,\n"),
            "Expected name on its own indented line, got: {}",
            result
        );
        assert!(
            result.contains("    email: String,\n"),
            "Expected email on its own indented line, got: {}",
            result
        );
        assert!(
            result.contains("    status: String,\n"),
            "Expected status on its own indented line, got: {}",
            result
        );
        assert!(
            result.contains("  ) -> void\n"),
            "Expected closing paren with return type, got: {}",
            result
        );
    }

    #[test]
    fn test_method_sig_single_line_when_few_params() {
        let test_file = Path::new("/tmp/test_sig_singleline.rb");
        fs::write(
            test_file,
            "\
class Foo
  #: (name: String, age: Integer, active: bool) -> void
  def update(name:, age:, active:)
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        // Exactly 3 params — should stay on one line
        assert!(
            result.contains("def update: (name: String, age: Integer, active: bool) -> void"),
            "Expected single-line sig with 3 params, got: {}",
            result
        );
    }

    #[test]
    fn test_self_method_sig_multiline_when_many_params() {
        let test_file = Path::new("/tmp/test_self_sig_multiline.rb");
        fs::write(
            test_file,
            "\
class Builder
  #: (name: String, age: Integer, email: String, role: Symbol) -> Builder
  def self.create(name:, age:, email:, role:)
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        // 4 params — should be multi-line
        assert!(
            result.contains("def self.create: (\n"),
            "Expected opening paren on its own line, got: {}",
            result
        );
        assert!(
            result.contains("  ) -> Builder\n"),
            "Expected closing paren with return type, got: {}",
            result
        );
    }

    // --- Tests for issue #12: stripping @desc/@example/etc. annotations ---

    #[test]
    fn test_strip_annotations_issue_12_example() {
        let test_file = Path::new("/tmp/test_strip_ann_issue12.rb");
        fs::write(
            test_file,
            "\
class MoveApplicant
  #: (
  #:   applicant_id: String
  #:     @desc(External_id of the applicant to move)
  #:     @example(app_abc123),
  #: ) -> output
  def call(**params)
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            !result.contains("@desc"),
            "Expected @desc to be stripped, got: {}",
            result
        );
        assert!(
            !result.contains("@example"),
            "Expected @example to be stripped, got: {}",
            result
        );
        assert!(
            result.contains("def call: (applicant_id: String) -> output"),
            "Expected clean signature, got: {}",
            result
        );
    }

    #[test]
    fn test_strip_multiple_annotation_kinds() {
        let test_file = Path::new("/tmp/test_strip_multi_kinds.rb");
        fs::write(
            test_file,
            "\
class Validator
  #: (
  #:   age: Integer @min(0) @max(120) @desc(Age in years),
  #:   email: String @format(email) @requires(domain),
  #: ) -> bool
  def call(age:, email:)
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        for tag in ["@min", "@max", "@desc", "@format", "@requires"] {
            assert!(
                !result.contains(tag),
                "Expected {} to be stripped, got: {}",
                tag,
                result
            );
        }
        assert!(
            result.contains("def call: (age: Integer, email: String) -> bool"),
            "Expected clean two-param signature, got: {}",
            result
        );
    }

    #[test]
    fn test_strip_annotations_on_attr() {
        let test_file = Path::new("/tmp/test_strip_attr.rb");
        fs::write(
            test_file,
            "class Foo\n  #: String @desc(The display name)\n  attr_reader :name\nend\n",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            !result.contains("@desc"),
            "Expected @desc to be stripped from attr, got: {}",
            result
        );
        assert!(
            result.contains("attr_reader name: String"),
            "Expected clean attr_reader, got: {}",
            result
        );
    }

    #[test]
    fn test_strip_annotations_on_type_alias_record() {
        // Regression: 0.3.6 stripped @desc/@example/etc. from method signatures
        // but left them inside `# @rbs type` aliases, where Steep also rejects
        // them with `RBS::SyntaxError: cannot start a declaration, token=@desc`
        // when they appear inside a record-type's `{ ... }`.
        let test_file = Path::new("/tmp/test_strip_type_alias_record.rb");
        fs::write(
            test_file,
            "\
class Applicant
  # @rbs type applicant_local = {
  #   external_id: String @desc(Stable applicant external_id) @example(app_123) @read_only(),
  #   email: String @format(email) @desc(Primary email) @example(jane@example.com),
  # }
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        for tag in ["@desc", "@example", "@read_only", "@format"] {
            assert!(
                !result.contains(tag),
                "Expected {} to be stripped from type alias, got: {}",
                tag,
                result
            );
        }
        assert!(
            result.contains("external_id: String"),
            "Expected external_id field preserved, got: {}",
            result
        );
        assert!(
            result.contains("email: String"),
            "Expected email field preserved, got: {}",
            result
        );
    }

    #[test]
    fn test_strip_annotations_on_single_line_type_alias() {
        let test_file = Path::new("/tmp/test_strip_type_alias_single.rb");
        fs::write(
            test_file,
            "\
class Funnel
  # @rbs type funnel_ref = { external_id: String @desc(Opening external_id) @example(funnel_abc), title: String @desc(Title shown in UI) }
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            !result.contains("@desc"),
            "Expected @desc to be stripped from single-line alias, got: {}",
            result
        );
        assert!(
            !result.contains("@example"),
            "Expected @example to be stripped from single-line alias, got: {}",
            result
        );
        assert!(
            result.contains("type funnel_ref"),
            "Expected type alias preserved, got: {}",
            result
        );
    }

    #[test]
    fn test_strip_annotations_on_singleton_method() {
        let test_file = Path::new("/tmp/test_strip_singleton.rb");
        fs::write(
            test_file,
            "class Tool\n  #: (id: String @desc(External id)) -> Tool\n  def self.call(id:)\n  end\nend\n",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("def self.call: (id: String) -> Tool"),
            "Expected clean self.call, got: {}",
            result
        );
    }

    #[test]
    fn test_strip_does_not_touch_signatures_without_annotations() {
        // Same input as `test_multiline_annotation` — must produce identical
        // output now that strip_annotations short-circuits when nothing matches.
        let test_file = Path::new("/tmp/test_strip_noop.rb");
        fs::write(
            test_file,
            "\
class MultilineTest
  #: (
  #:   name: String,
  #:   age: Integer,
  #:   ?email: String?
  #: ) -> Hash[Symbol, untyped]
  def self.call(name:, age:, email: nil)
  end
end
",
        )
        .unwrap();

        let mut transpiler = SentinelTranspiler::new();
        let result = transpiler.transpile_file(test_file).unwrap();
        assert!(
            result.contains("def self.call: ( name: String, age: Integer, ?email: String? ) -> Hash[Symbol, untyped]"),
            "Annotation-free signature should be unchanged, got: {}",
            result
        );
    }

    #[test]
    fn test_strip_annotations_helper_unit() {
        assert_eq!(
            SentinelTranspiler::strip_annotations(
                "(applicant_id: String @desc(External id) @example(app_abc123)) -> output"
            ),
            "(applicant_id: String) -> output"
        );
        // Mid-string trailing comma between params
        assert_eq!(
            SentinelTranspiler::strip_annotations(
                "(a: Integer @min(0), b: String @desc(name)) -> void"
            ),
            "(a: Integer, b: String) -> void"
        );
        // No annotation: returned unchanged
        assert_eq!(
            SentinelTranspiler::strip_annotations("(a: Integer, b: String) -> void"),
            "(a: Integer, b: String) -> void"
        );
        // Stray @ that is not an annotation: left alone
        assert_eq!(
            SentinelTranspiler::strip_annotations("(email: String) -> bool"),
            "(email: String) -> bool"
        );
    }

    #[test]
    fn test_split_top_level_commas_respects_nesting() {
        // Ensure commas inside nested brackets are not treated as separators.
        let entries = SentinelTranspiler::split_top_level_commas(
            "key: Hash[String, Integer], other: { a: String, b: Integer }",
        );
        assert_eq!(
            entries,
            vec![
                "key: Hash[String, Integer]",
                "other: { a: String, b: Integer }",
            ]
        );
    }

    #[test]
    fn test_recursive_import_resolves_nested_types() {
        let tmp = TempDir::new().unwrap();
        let shared_dir = tmp.path().join("shared");
        fs::create_dir_all(&shared_dir).unwrap();

        fs::write(
            shared_dir.join("stage_ref.rbs"),
            "type stage_ref = {\n  id: String @desc(Stage ID),\n  title: String\n}\n",
        ).unwrap();
        fs::write(
            shared_dir.join("owner_ref.rbs"),
            "type owner_ref = {\n  name: String,\n  email: String @format(email)\n}\n",
        ).unwrap();
        fs::write(
            shared_dir.join("funnel_result.rbs"),
            "type funnel_result = {\n  title: String,\n  ?stages: Array[stage_ref],\n  ?owner: owner_ref\n}\n",
        ).unwrap();

        let test_file = tmp.path().join("test.rb");
        fs::write(
            &test_file,
            "module Tool\n  class GetFunnel\n    # @rbs import funnel_result\n\n    # @rbs type success = {\n    #   success: true,\n    #   funnel: funnel_result\n    # }\n\n    #: () -> success\n    def call\n    end\n  end\nend\n",
        ).unwrap();

        let mut transpiler = SentinelTranspiler::new();
        transpiler.set_shared_paths(vec![shared_dir.clone()]);
        let result = transpiler.transpile_file(&test_file).unwrap();

        assert!(result.contains("type funnel_result ="), "Expected funnel_result type, got: {}", result);
        assert!(result.contains("type stage_ref ="), "Expected stage_ref to be recursively resolved, got: {}", result);
        assert!(result.contains("type owner_ref ="), "Expected owner_ref to be recursively resolved, got: {}", result);
        assert!(!result.contains("@desc"), "Expected annotations to be stripped, got: {}", result);
        assert!(!result.contains("@format"), "Expected annotations to be stripped, got: {}", result);

        // Dependencies should appear before the type that references them
        let stage_pos = result.find("type stage_ref =").unwrap();
        let owner_pos = result.find("type owner_ref =").unwrap();
        let funnel_pos = result.find("type funnel_result =").unwrap();
        assert!(stage_pos < funnel_pos, "stage_ref should appear before funnel_result");
        assert!(owner_pos < funnel_pos, "owner_ref should appear before funnel_result");
    }

    #[test]
    fn test_recursive_import_no_cycles() {
        let tmp = TempDir::new().unwrap();
        let shared_dir = tmp.path().join("shared");
        fs::create_dir_all(&shared_dir).unwrap();

        fs::write(shared_dir.join("type_a.rbs"), "type type_a = { ref: type_b }\n").unwrap();
        fs::write(shared_dir.join("type_b.rbs"), "type type_b = { ref: type_a }\n").unwrap();

        let test_file = tmp.path().join("test.rb");
        fs::write(
            &test_file,
            "class Foo\n  # @rbs import type_a\n\n  #: () -> type_a\n  def call\n  end\nend\n",
        ).unwrap();

        let mut transpiler = SentinelTranspiler::new();
        transpiler.set_shared_paths(vec![shared_dir.clone()]);
        let result = transpiler.transpile_file(&test_file).unwrap();

        assert!(result.contains("type type_a ="), "Expected type_a, got: {}", result);
        assert!(result.contains("type type_b ="), "Expected type_b, got: {}", result);
    }

    #[test]
    fn test_recursive_import_types_in_same_file() {
        let tmp = TempDir::new().unwrap();
        let shared_dir = tmp.path().join("shared");
        fs::create_dir_all(&shared_dir).unwrap();

        fs::write(
            shared_dir.join("bundle.rbs"),
            "type inner = { id: String }\n\ntype bundle = { item: inner }\n",
        ).unwrap();

        let test_file = tmp.path().join("test.rb");
        fs::write(
            &test_file,
            "class Foo\n  # @rbs import bundle\n\n  #: () -> bundle\n  def call\n  end\nend\n",
        ).unwrap();

        let mut transpiler = SentinelTranspiler::new();
        transpiler.set_shared_paths(vec![shared_dir.clone()]);
        let result = transpiler.transpile_file(&test_file).unwrap();

        assert!(result.contains("type inner ="), "Expected inner, got: {}", result);
        assert!(result.contains("type bundle ="), "Expected bundle, got: {}", result);

        // No duplicates — inner should appear exactly once
        let count = result.matches("type inner =").count();
        assert_eq!(count, 1, "Expected inner exactly once, found {} times in: {}", count, result);
    }

    #[test]
    fn test_colocated_types_reversed_file_order() {
        let tmp = TempDir::new().unwrap();
        let shared_dir = tmp.path().join("shared");
        fs::create_dir_all(&shared_dir).unwrap();

        // bundle is defined BEFORE inner in the file — dependency order is reversed
        fs::write(
            shared_dir.join("bundle.rbs"),
            "type bundle = { item: inner }\n\ntype inner = { id: String }\n",
        ).unwrap();

        let test_file = tmp.path().join("test.rb");
        fs::write(
            &test_file,
            "class Foo\n  # @rbs import bundle\n\n  #: () -> bundle\n  def call\n  end\nend\n",
        ).unwrap();

        let mut transpiler = SentinelTranspiler::new();
        transpiler.set_shared_paths(vec![shared_dir.clone()]);
        let result = transpiler.transpile_file(&test_file).unwrap();

        assert!(result.contains("type inner ="), "Expected inner, got: {}", result);
        assert!(result.contains("type bundle ="), "Expected bundle, got: {}", result);

        // inner must appear before bundle even though file order is reversed
        let inner_pos = result.find("type inner =").unwrap();
        let bundle_pos = result.find("type bundle =").unwrap();
        assert!(inner_pos < bundle_pos, "inner should appear before bundle (dependency order), got: {}", result);
    }

    #[test]
    fn test_references_type_word_boundary() {
        assert!(SentinelTranspiler::references_type("type foo = { e: error }", "error"));
        assert!(!SentinelTranspiler::references_type("type foo = { e: error_code }", "error"));
        assert!(SentinelTranspiler::references_type("type foo = { e: Array[error] }", "error"));
        assert!(SentinelTranspiler::references_type("type foo = { e: error, f: String }", "error"));
        assert!(SentinelTranspiler::references_type("type error = { code: String }", "error"));
    }
}
