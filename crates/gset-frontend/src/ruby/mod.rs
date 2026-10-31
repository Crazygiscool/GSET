use crate::Frontend;
use gset_ir::{
    BinaryOp, Block, ComparisonOp, Diagnostic, Else, Expr, ExprKind, Function, Item, LangId,
    Literal, LogicalOp, Lowered, Module, Name, Path, Pattern, SourceId, SourceMap, Span, Stmt,
    UnaryOp,
};
use std::sync::Arc;

pub struct Ruby;
impl Frontend for Ruby {
    fn lang(&self) -> LangId {
        LangId::RUBY
    }
    fn name(&self) -> &'static str {
        "ruby"
    }
    fn handles_extension(&self, path: &std::path::Path) -> bool {
        matches!(path.extension().and_then(|e| e.to_str()), Some("rb"))
    }
    fn lower(&self, source: &str, text: &str) -> Lowered {
        let mut source_map = SourceMap::new();
        let id = source_map.add(source.to_string(), text.to_string());
        let mut parser = tree_sitter::Parser::new();
        if let Err(error) = parser.set_language(&tree_sitter_ruby::LANGUAGE.into()) {
            let mut d = gset_ir::DiagnosticBag::new();
            d.push(Diagnostic::error(
                Span::synthetic(),
                format!("cannot load ruby grammar: {error}"),
            ));
            return Lowered {
                module: None,
                diagnostics: d,
                source_map,
            };
        }
        let tree = match parser.parse(text, None) {
            Some(t) => t,
            None => {
                let mut d = gset_ir::DiagnosticBag::new();
                d.push(Diagnostic::error(
                    Span::point(id, 0),
                    "ruby parser produced no syntax tree",
                ));
                return Lowered {
                    module: None,
                    diagnostics: d,
                    source_map,
                };
            }
        };
        let root = tree.root_node();
        let mut diagnostics = gset_ir::DiagnosticBag::new();
        let mut items = Vec::new();
        let mut ci = 0;
        while ci < root.child_count() {
            if let Some(child) = root.child(ci) {
                match child.kind() {
                    "method" => {
                        if let Some(item) = lower_method(child, id, text) {
                            items.push(item);
                        }
                    }
                    "class" | "module" => diagnostics.push(Diagnostic::warning(
                        span_of(child, id),
                        format!("unsupported top-level construct: {}", child.kind()),
                    )),
                    _ => {
                        // Treat bare top-level expressions as statements.
                        if let Some(s) = lower_stmt(child, id, text) {
                            items.push(Item::Stmt(s));
                        } else {
                            diagnostics.push(Diagnostic::warning(
                                span_of(child, id),
                                format!("unsupported top-level construct: {}", child.kind()),
                            ));
                        }
                    }
                }
            }
            ci += 1;
        }
        let module = Module::empty(LangId::RUBY, span_of(root, id));
        let mut m = module;
        m.items = items;
        Lowered {
            module: Some(m),
            diagnostics,
            source_map,
        }
    }
}

fn lower_method(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Item> {
    let name = node.child_by_field_name("name")?;
    let params_node = node.child_by_field_name("parameters");
    let mut params = Vec::new();
    if let Some(pn) = params_node {
        let mut ci = 0;
        while ci < pn.child_count() {
            if let Some(c) = pn.child(ci) {
                if c.kind() == "identifier" {
                    params.push(Pattern::bind(
                        Arc::from(text_of(c, text)),
                        span_of(c, source),
                    ));
                }
            }
            ci += 1;
        }
    }
    let body_node = node.child_by_field_name("body");
    let body = lower_body(body_node, source, text);
    Some(Item::Function(Function {
        name: Name::from(text_of(name, text).to_string()),
        params,
        param_types: Vec::new(),
        ret: None,
        body,
        variadic: false,
        is_async: false,
        exported: false,
        decorators: Vec::new(),
        generics: Vec::new(),
        span: span_of(node, source),
    }))
}

fn lower_body(node: Option<tree_sitter::Node>, source: SourceId, text: &str) -> Block {
    let Some(node) = node else {
        return Block {
            statements: Vec::new(),
            span: Span::synthetic(),
        };
    };
    // If it's a body_statement or a node with statements
    let target = if node.kind() == "body_statement" {
        Some(node)
    } else {
        Some(node)
    };
    let mut statements = Vec::new();
    if let Some(n) = target {
        let mut ci = 0;
        while ci < n.child_count() {
            if let Some(child) = n.child(ci) {
                if child.kind() != "\n" {
                    if let Some(s) = lower_stmt(child, source, text) {
                        statements.push(s);
                    }
                }
            }
            ci += 1;
        }
    }
    let span = target
        .map(|n| span_of(n, source))
        .unwrap_or_else(Span::synthetic);
    Block { statements, span }
}

fn lower_stmt(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Stmt> {
    let span = span_of(node, source);
    match node.kind() {
        "return" => {
            // return expression? often separate node; handle by expression siblings
            let mut v = None;
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    if c.kind() != "return" && c.kind() != ";" {
                        v = Some(lower_expr(c, source, text));
                    }
                }
                ci += 1;
            }
            Some(Stmt::Return { value: v, span })
        }
        "if" | "unless" => {
            let cond_node = node.child_by_field_name("condition")?;
            let condition = lower_expr(cond_node, source, text);
            let cons_node = node.child_by_field_name("consequence");
            let then_branch = if let Some(n) = cons_node {
                lower_body(Some(n), source, text)
            } else {
                lower_body(node.child_by_field_name("body"), source, text)
            };
            let else_branch = if let Some(alt) = node.child_by_field_name("alternative") {
                Some(Box::new(Else::Block(lower_body(Some(alt), source, text))))
            } else {
                None
            };
            Some(Stmt::If {
                condition,
                then_branch,
                else_branch,
                span,
            })
        }
        "call" | "command" | "command_call" | "method_call" => {
            Some(Stmt::Expr(lower_expr(node, source, text)))
        }
        "binary" | "identifier" | "integer" | "string" | "true" | "false" | "nil" => {
            Some(Stmt::Expr(lower_expr(node, source, text)))
        }
        "expression_statement" => {
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    if c.kind() != ";" {
                        if let Some(s) = lower_stmt(c, source, text) {
                            return Some(s);
                        }
                    }
                }
                ci += 1;
            }
            None
        }
        _ => {
            // Try all children
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    if let Some(s) = lower_stmt(c, source, text) {
                        return Some(s);
                    }
                }
                ci += 1;
            }
            None
        }
    }
}

fn lower_expr(node: tree_sitter::Node, source: SourceId, text: &str) -> Expr {
    let span = span_of(node, source);
    match node.kind() {
        "integer" | "float" => Expr::new(
            ExprKind::Literal(Literal::Int(text_of(node, text).to_string())),
            span,
        ),
        "string" | "simple_string" => {
            let body = text_of(node, text)
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            Expr::new(ExprKind::Literal(Literal::Str(body)), span)
        }
        "true" => Expr::new(ExprKind::Literal(Literal::Bool(true)), span),
        "false" => Expr::new(ExprKind::Literal(Literal::Bool(false)), span),
        "nil" => Expr::new(ExprKind::Literal(Literal::Null), span),
        "binary" => {
            let left = lower_expr(node.child_by_field_name("left").unwrap(), source, text);
            let op_text = text_of(node.child_by_field_name("operator").unwrap(), text);
            let right = lower_expr(node.child_by_field_name("right").unwrap(), source, text);
            if matches!(op_text, "==" | "!=" | "<" | "<=" | ">" | ">=") {
                let op = match op_text {
                    "==" => ComparisonOp::Eq,
                    "!=" => ComparisonOp::Ne,
                    "<" => ComparisonOp::Lt,
                    "<=" => ComparisonOp::Le,
                    ">" => ComparisonOp::Gt,
                    ">=" => ComparisonOp::Ge,
                    _ => ComparisonOp::Eq,
                };
                Expr::new(
                    ExprKind::Compare {
                        op,
                        lhs: Box::new(left),
                        rhs: Box::new(right),
                    },
                    span,
                )
            } else if matches!(op_text, "&&" | "||" | "and" | "or") {
                let op = if op_text == "&&" || op_text == "and" {
                    LogicalOp::And
                } else {
                    LogicalOp::Or
                };
                Expr::new(
                    ExprKind::Logical {
                        op,
                        lhs: Box::new(left),
                        rhs: Box::new(right),
                    },
                    span,
                )
            } else {
                let op = match op_text {
                    "+" => BinaryOp::Add,
                    "-" => BinaryOp::Sub,
                    "*" => BinaryOp::Mul,
                    "/" => BinaryOp::Div,
                    "%" => BinaryOp::Rem,
                    "**" => BinaryOp::Pow,
                    _ => BinaryOp::Add,
                };
                Expr::new(
                    ExprKind::Binary {
                        op,
                        lhs: Box::new(left),
                        rhs: Box::new(right),
                    },
                    span,
                )
            }
        }
        "call" | "command" | "command_call" | "method_call" => {
            let method_field = node.child_by_field_name("method");
            let name_txt = if let Some(m) = method_field {
                text_of(m, text).to_string()
            } else {
                text_of(node.named_child(0).unwrap(), text).to_string()
            };
            let obj = node.child_by_field_name("receiver");
            let callee = if let Some(o) = obj {
                Expr::new(
                    ExprKind::Field {
                        target: Box::new(lower_expr(o, source, text)),
                        field: Name::from(name_txt.clone()),
                    },
                    span,
                )
            } else {
                Expr::path(Arc::from(name_txt), span)
            };
            let mut args = Vec::new();
            // argument_list
            if let Some(al) = node.child_by_field_name("arguments") {
                let mut ci = 0;
                while ci < al.child_count() {
                    if let Some(c) = al.child(ci) {
                        if c.kind() != "(" && c.kind() != ")" && c.kind() != "," {
                            args.push(lower_expr(c, source, text));
                        }
                    }
                    ci += 1;
                }
            }
            Expr::new(
                ExprKind::Call {
                    callee: Box::new(callee),
                    args,
                    named_args: Vec::new(),
                },
                span,
            )
        }
        "identifier" | "_variable" => Expr::path(Arc::from(text_of(node, text).to_string()), span),
        "parenthesized_statements" | "parenthesized" => {
            if let Some(inner) = node.named_child(0) {
                lower_expr(inner, source, text)
            } else {
                Expr::new(ExprKind::Literal(Literal::Null), span)
            }
        }
        _ => {
            // Try first named child
            if let Some(inner) = node.named_child(0) {
                lower_expr(inner, source, text)
            } else {
                Expr::new(ExprKind::Literal(Literal::Null), span)
            }
        }
    }
}
fn span_of(node: tree_sitter::Node, source: SourceId) -> Span {
    Span::new(source, node.start_byte() as u32, node.end_byte() as u32)
}
fn text_of<'a>(node: tree_sitter::Node, text: &'a str) -> &'a str {
    &text[node.start_byte()..node.end_byte()]
}
