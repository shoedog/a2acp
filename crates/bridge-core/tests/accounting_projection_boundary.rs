//! Composite failures stay intact until a wire response or a thin compatibility wrapper.
use std::path::{Path, PathBuf};
use syn::visit::Visit;

const ALLOWED: &[&str] = &[
    "bridge-coordinator/src/coordinator.rs:prompt_with_identity",
    "bridge-coordinator/src/coordinator.rs:continue_turn_with_identity",
    "bridge-a2a-inbound/src/server.rs:stream_message",
    "bridge-a2a-inbound/src/server.rs:spawn_local_producer",
    "bridge-a2a-inbound/src/server.rs:local_kiro_source",
    "bridge-a2a-inbound/src/server.rs:spawn_fanout_producer",
    "bridge-a2a-inbound/src/server.rs:unary_message",
    "bridge-a2a-inbound/src/server.rs:unary_fanout_message",
    "bridge-a2a-inbound/src/server.rs:session_compact",
    "bridge-mcp/src/server.rs:dispatch",
];

fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        matches!(&attr.meta, syn::Meta::List(meta) if meta.path.is_ident("cfg") && meta.tokens.to_string() == "test")
    })
}

struct Boundary<'a> {
    path: &'a str,
    function: String,
}
impl Boundary<'_> {
    fn projection(&self) {
        assert!(
            ALLOWED.contains(&format!("{}:{}", self.path, self.function).as_str()),
            "early accounting projection in {}:{}",
            self.path,
            self.function
        );
    }
    fn preparation(&self) {
        assert!(
            !self.path.starts_with("bridge-core/") && !self.path.starts_with("bridge-coordinator/"),
            "settle-then-? loses the original cause in {}:{}",
            self.path,
            self.function
        );
    }
}
impl<'ast> Visit<'ast> for Boundary<'_> {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !test_only(&node.attrs) {
            syn::visit::visit_item_mod(self, node);
        }
    }
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if !test_only(&node.attrs) {
            let previous = std::mem::replace(&mut self.function, node.sig.ident.to_string());
            syn::visit::visit_item_fn(self, node);
            self.function = previous;
        }
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if !test_only(&node.attrs) {
            let previous = std::mem::replace(&mut self.function, node.sig.ident.to_string());
            syn::visit::visit_impl_item_fn(self, node);
            self.function = previous;
        }
    }
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "surfaced_error" {
            self.projection();
        }
        syn::visit::visit_expr_method_call(self, node);
    }
    fn visit_expr_try(&mut self, node: &'ast syn::ExprTry) {
        if let syn::Expr::Await(awaited) = &*node.expr {
            if let syn::Expr::MethodCall(call) = &*awaited.base {
                if call.method == "preparation_failed" {
                    self.preparation();
                }
            }
        }
        syn::visit::visit_expr_try(self, node);
    }
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        // async_stream and tokio::select bodies are opaque to syn's AST visitor.
        let compact: String = node
            .tokens
            .to_string()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if compact.contains(".surfaced_error(") {
            self.projection();
        }
        if compact.contains(".preparation_failed().await?") {
            self.preparation();
        }
    }
}

fn scan(path: &Path, root: &Path) {
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, root);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let name = path.strip_prefix(root).unwrap().to_str().unwrap();
            let source = std::fs::read_to_string(&path).unwrap();
            Boundary {
                path: name,
                function: String::new(),
            }
            .visit_file(&syn::parse_file(&source).unwrap());
        }
    }
}

#[test]
fn accounting_projection_boundary() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for directory in ["crates", "bin"] {
        let root = workspace.join(directory);
        for package in std::fs::read_dir(&root).unwrap() {
            let source = package.unwrap().path().join("src");
            if source.is_dir() {
                scan(&source, &root);
            }
        }
    }
}
