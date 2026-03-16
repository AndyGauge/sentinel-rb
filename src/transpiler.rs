use std::fs;
use std::path::Path;
use tree_sitter::{Node, Parser};

struct ClassInfo {
    modules: Vec<String>,
    class_name: String,
    methods: Vec<(String, String)>,
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

    /// Collect structure from the AST: module nesting, class name, and annotated methods.
    fn collect_structure(source: &str, root: Node) -> ClassInfo {
        let mut module_stack = Vec::new();
        let mut info = ClassInfo {
            modules: Vec::new(),
            class_name: "UnknownClass".to_string(),
            methods: Vec::new(),
        };
        Self::walk(source, root, &mut module_stack, &mut info);
        info
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

                // Collect annotated methods from the class.
                //
                // Tree structure: the first annotation may appear as a direct
                // child of `class` (before `body_statement`), while subsequent
                // annotations are siblings of methods INSIDE `body_statement`:
                //
                //   class
                //     comment "#: ..."          ← 1st annotation (class child)
                //     body_statement
                //       method (initialize)
                //       comment "#: ..."        ← 2nd annotation (body_statement child)
                //       method (call)
                //       comment "#: ..."        ← 3rd annotation
                //       method (other)
                //
                // We first check for a class-level annotation, then scan
                // body_statement children for all comment→method pairs.
                let mut pending_annotation: Option<String> = None;
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    match child.kind() {
                        "comment" => {
                            let text = Self::node_text(source, &child);
                            if let Some(sig) = text.strip_prefix("#: ") {
                                pending_annotation = Some(sig.trim().to_string());
                            } else {
                                pending_annotation = None;
                            }
                        }
                        "body_statement" => {
                            // Scan body_statement children for comment→method pairs
                            let mut bs_cursor = child.walk();
                            for bs_child in child.children(&mut bs_cursor) {
                                match bs_child.kind() {
                                    "comment" => {
                                        let text = Self::node_text(source, &bs_child);
                                        if let Some(sig) = text.strip_prefix("#: ") {
                                            pending_annotation = Some(sig.trim().to_string());
                                        } else {
                                            pending_annotation = None;
                                        }
                                    }
                                    "method" => {
                                        if let Some(sig) = pending_annotation.take() {
                                            if let Some(name_node) =
                                                bs_child.child_by_field_name("name")
                                            {
                                                let method_name =
                                                    Self::node_text(source, &name_node)
                                                        .to_string();
                                                info.methods.push((method_name, sig));
                                            }
                                        }
                                    }
                                    _ => {
                                        pending_annotation = None;
                                    }
                                }
                            }
                        }
                        _ => {
                            if child.kind() != "superclass" {
                                pending_annotation = None;
                            }
                        }
                    }
                }
                return;
            }
            _ => {}
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            Self::walk(source, child, module_stack, info);
        }
    }

    pub fn transpile_file(
        &mut self,
        rb_path: &Path,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let source = fs::read_to_string(rb_path)?;
        let tree = self.parser.parse(&source, None).ok_or("Failed to parse")?;

        let info = Self::collect_structure(&source, tree.root_node());
        let modules = info.modules;
        let class_name = info.class_name;
        let method_sigs = info.methods;

        let mut rbs_output = String::new();
        rbs_output.push_str("# Generated by Sentinel - Do not edit manually\n\n");

        // Emit nested module/class structure
        let depth = modules.len();
        for (i, module_name) in modules.iter().enumerate() {
            let indent = "  ".repeat(i);
            rbs_output.push_str(&format!("{}module {}\n", indent, module_name));
        }

        let class_indent = "  ".repeat(depth);
        let method_indent = "  ".repeat(depth + 1);
        rbs_output.push_str(&format!("{}class {}\n", class_indent, class_name));

        for (name, sig) in &method_sigs {
            rbs_output.push_str(&format!("{}def {}: {}\n", method_indent, name, sig));
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
}
