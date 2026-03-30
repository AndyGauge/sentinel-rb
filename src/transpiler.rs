use std::fs;
use std::path::Path;
use tree_sitter::{Node, Parser};

struct ClassInfo {
    modules: Vec<String>,
    class_name: String,
    methods: Vec<(String, String)>,
    self_methods: Vec<(String, String)>,
    type_aliases: Vec<String>,
    attributes: Vec<(String, String, String)>, // (attr_kind, attr_name, type)
}

pub struct SentinelTranspiler {
    parser: Parser,
}

impl SentinelTranspiler {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        let lang = tree_sitter_ruby::language();
        parser
            .set_language(lang)
            .expect("Error loading Ruby grammar");

        Self { parser }
    }

    /// Extract the text of a node from source
    fn node_text<'a>(source: &'a str, node: &Node) -> &'a str {
        &source[node.start_byte()..node.end_byte()]
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
    fn collect_structure(source: &str, root: Node) -> ClassInfo {
        let mut module_stack = Vec::new();
        let mut info = ClassInfo {
            modules: Vec::new(),
            class_name: "UnknownClass".to_string(),
            methods: Vec::new(),
            self_methods: Vec::new(),
            type_aliases: Vec::new(),
            attributes: Vec::new(),
        };
        Self::walk(source, root, &mut module_stack, &mut info);
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
    ) {
        let mut pending_annotation: Option<String> = None;
        let mut pending_type_alias: Option<String> = None;

        for &child in children {
            match child.kind() {
                "comment" => {
                    let text = Self::node_text(source, &child);

                    // Check for @rbs type alias
                    if let Some(rest) = text.strip_prefix("# @rbs type ") {
                        let trimmed = rest.trim().to_string();
                        if Self::is_balanced(&trimmed) {
                            info.type_aliases.push(format!("type {}", trimmed));
                            pending_type_alias = None;
                        } else {
                            pending_type_alias = Some(trimmed);
                        }
                        pending_annotation = None;
                    } else if let Some(ref mut alias) = pending_type_alias {
                        // Continue multi-line type alias
                        if let Some(cont) = text.strip_prefix('#') {
                            alias.push(' ');
                            alias.push_str(cont.trim());
                            if Self::is_balanced(alias) {
                                info.type_aliases
                                    .push(format!("type {}", alias));
                                pending_type_alias = None;
                            }
                        } else {
                            pending_type_alias = None;
                        }
                    } else if let Some(sig) = text.strip_prefix("#: ") {
                        pending_annotation = Some(sig.trim().to_string());
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
                            info.self_methods.push((method_name, sig));
                        }
                    }
                }
                "singleton_class" => {
                    // class << self — methods inside are class methods
                    let inner_children = Self::flatten_children(child);
                    Self::scan_body(source, &inner_children, info, true);
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
    }

    fn walk(
        source: &str,
        node: Node,
        module_stack: &mut Vec<String>,
        info: &mut ClassInfo,
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
                        Self::walk(source, child, module_stack, info);
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

                if let Some(name_node) = node.child_by_field_name("name") {
                    info.class_name = Self::node_text(source, &name_node).to_string();
                }

                // Flatten direct children + body_statement children, then scan
                let children = Self::flatten_children(node);
                Self::scan_body(source, &children, info, false);

                return;
            }
            _ => {}
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            Self::walk(source, child, module_stack, info);
        }
    }

    /// Returns true if the generated RBS has meaningful content worth writing
    pub fn has_content(rbs: &str) -> bool {
        rbs.contains("def ") || rbs.contains("type ") || rbs.contains("attr_")
    }

    pub fn transpile_file(
        &mut self,
        rb_path: &Path,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let source = fs::read_to_string(rb_path)?;
        let tree = self.parser.parse(&source, None).ok_or("Failed to parse")?;

        let info = Self::collect_structure(&source, tree.root_node());

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
        rbs_output.push_str(&format!("{}class {}\n", class_indent, info.class_name));

        // Type aliases
        for alias in &info.type_aliases {
            rbs_output.push_str(&format!("{}{}\n", member_indent, alias));
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
            rbs_output.push_str(&format!(
                "{}def self.{}: {}\n",
                member_indent, name, sig
            ));
        }

        // Instance methods
        for (name, sig) in &info.methods {
            rbs_output.push_str(&format!("{}def {}: {}\n", member_indent, name, sig));
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
    fn test_has_content() {
        assert!(SentinelTranspiler::has_content("  def foo: () -> void\n"));
        assert!(SentinelTranspiler::has_content("  type error_code = String\n"));
        assert!(SentinelTranspiler::has_content("  attr_reader name: String\n"));
        assert!(!SentinelTranspiler::has_content("class Foo\nend\n"));
    }
}
