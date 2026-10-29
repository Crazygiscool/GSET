//! The JavaScript frontend.

use gset_ir::{
    BinaryOp, Block, ComparisonOp, Diagnostic, Expr, ExprKind, Function, Item, LangId, Lowered,
    Module, Name, Path as IrPath, Pattern, SourceId, SourceMap, Span, Stmt,
};

use crate::Frontend;
use gset_ir::DiagnosticBag;
use std::sync::Arc;

/// The JavaScript frontend.
pub struct JavaScript;

impl Frontend for JavaScript {
    fn lang(&self) -> LangId {
        LangId::JAVASCRIPT
    }

    fn name(&self) -> &'static str {
        "javascript"
    }

    fn handles_extension(&self, path: &std::path::Path) -> bool {
        let ext = match path.extension() {
            Some(ext) => ext,
            None => return false,
        };
        let name = match ext.to_str() {
            Some(name) => name,
            None => return false,
        };
        let name = name.to_lowercase();
        name == "js" || name == "jsx" || name == "mjs" || name == "cjs"
    }

    fn lower(&self, source: &str, text: &str) -> Lowered {
        let mut source_map = SourceMap::new();
        let id = source_map.add(source.to_string(), text.to_string());

        let mut parser = tree_sitter::Parser::new();
        if let Err(error) = parser.set_language(&tree_sitter_javascript::LANGUAGE.into()) {
            let mut diagnostics = DiagnosticBag::new();
            diagnostics.push(Diagnostic::error(
                Span::synthetic(),
                format!("cannot load javascript grammar: {error}"),
            ));
            return Lowered {
                module: None,
                diagnostics,
                source_map,
            };
        }
        let tree = match parser.parse(text, None) {
            Some(t) => t,
            None => {
                let mut diagnostics = DiagnosticBag::new();
                diagnostics.push(Diagnostic::error(
                    Span::point(id, 0),
                    "javascript parser produced no syntax tree",
                ));
                return Lowered {
                    module: None,
                    diagnostics,
                    source_map,
                };
            }
        };
        let root = tree.root_node();

        let mut diagnostics = DiagnosticBag::new();
        collect_syntax_diagnostics(root, text, id, &mut diagnostics);

        let mut items = Vec::new();
        let mut ci = 0;
        while ci < root.child_count() {
            if let Some(child) = root.child(ci) {
                match child.kind() {
                    "function_declaration" => {
                        if let Some(item) = lower_function(child, id, text) {
                            items.push(item);
                        }
                    }
                    "expression_statement" => {
                        if let Some(stmt) = lower_expr_statement(child, id, text) {
                            items.push(Item::Stmt(stmt));
                        }
                    }
                    "program" => {}
                    _ => {
                        diagnostics.push(Diagnostic::warning(
                            span_of(child, id),
                            format!("unsupported top-level construct: {}", child.kind()),
                        ));
                    }
                }
            }
            ci += 1;
        }

        let module = if diagnostics.has_errors() {
            None
        } else {
            let mut module = Module::empty(LangId::JAVASCRIPT, span_of(root, id));
            module.items = items;
            Some(module)
        };

        Lowered {
            module,
            diagnostics,
            source_map,
        }
    }
}

fn span_of(node: tree_sitter::Node, source: SourceId) -> Span {
    Span::new(source, node.start_byte() as u32, node.end_byte() as u32)
}

fn text_of<'a>(node: tree_sitter::Node, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

fn collect_syntax_diagnostics(
    node: tree_sitter::Node,
    _text: &str,
    source: SourceId,
    out: &mut gset_ir::DiagnosticBag,
) {
    if node.is_missing() {
        out.push(Diagnostic::error(span_of(node, source), "missing node"));
    }
    if node.kind() == "ERROR" {
        out.push(Diagnostic::error(span_of(node, source), "syntax error"));
    }
    let mut ci = 0;
    while ci < node.child_count() {
        if let Some(c) = node.child(ci) {
            collect_syntax_diagnostics(c, _text, source, out);
        }
        ci += 1;
    }
}

fn lower_function(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Item> {
    let name_node = node.child_by_field_name("name")?;
    let params_node = node.child_by_field_name("parameters")?;
    let body_node = node.child_by_field_name("body")?;

    let name: Name = Arc::from(text_of(name_node, text));
    let mut params = Vec::new();
    let mut ci = 0;
    while ci < params_node.child_count() {
        if let Some(c) = params_node.child(ci)
            && c.kind() == "identifier"
        {
            let pname = text_of(c, text);
            params.push(Pattern::bind(Arc::from(pname), span_of(c, source)));
        }
        ci += 1;
    }

    let body = lower_block(body_node, source, text);
    let span = span_of(node, source);

    Some(Item::Function(Function {
        name,
        params: params.clone(),
        param_types: vec![None; params.len()],
        ret: None,
        body,
        variadic: false,
        is_async: false,
        exported: false,
        decorators: Vec::new(),
        generics: Vec::new(),
        span,
    }))
}

fn lower_block(node: tree_sitter::Node, source: SourceId, text: &str) -> Block {
    let mut stmts = Vec::new();
    let mut ci = 0;
    while ci < node.child_count() {
        if let Some(c) = node.child(ci)
            && let Some(s) = lower_stmt(c, source, text)
        {
            stmts.push(s);
        }
        ci += 1;
    }
    Block {
        statements: stmts,
        span: span_of(node, source),
    }
}

fn lower_expr_statement(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Stmt> {
    let span = span_of(node, source);
    let mut ci = 0;
    while ci < node.child_count() {
        if let Some(c) = node.child(ci)
            && c.kind() != ";"
        {
            return Some(Stmt::Expr(lower_expr(c, source, text)));
        }
        ci += 1;
    }
    Some(Stmt::Empty { span })
}

fn lower_stmt(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Stmt> {
    let span = span_of(node, source);
    match node.kind() {
        "return_statement" => {
            let mut v = None;
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci)
                    && c.kind() != "return"
                    && c.kind() != ";"
                {
                    v = Some(lower_expr(c, source, text));
                }
                ci += 1;
            }
            Some(Stmt::Return { value: v, span })
        }
        "if_statement" => {
            let mut cond_opt = None;
            let mut ci_outer = 0;
            while ci_outer < node.child_count() {
                if let Some(cc) = node.child(ci_outer)
                    && cc.kind() == "parenthesized_expression"
                {
                    let mut ci_inner = 0;
                    while ci_inner < cc.child_count() {
                        if let Some(ec) = cc.child(ci_inner)
                            && ec.kind() != "("
                            && ec.kind() != ")"
                        {
                            cond_opt = Some(lower_expr(ec, source, text));
                        }
                        ci_inner += 1;
                    }
                }
                ci_outer += 1;
            }
            let condition = match cond_opt {
                Some(e) => e,
                None => Expr::null(span),
            };
            let mut then_branch = Block {
                statements: Vec::new(),
                span,
            };
            let mut ci2 = 0;
            while ci2 < node.child_count() {
                if let Some(cc) = node.child(ci2) {
                    if cc.kind() == "statement_block" {
                        then_branch = lower_block(cc, source, text);
                    } else if (cc.kind() == "return_statement"
                        || cc.kind() == "expression_statement"
                        || cc.kind() == "if_statement"
                        || cc.kind() == "statement_block")
                        && let Some(st) = lower_stmt(cc, source, text)
                    {
                        then_branch = Block {
                            statements: vec![st],
                            span,
                        };
                    }
                }
                ci2 += 1;
            }
            Some(Stmt::If {
                condition,
                then_branch,
                else_branch: None,
                span,
            })
        }
        "expression_statement" => lower_expr_statement(node, source, text),
        _ => None,
    }
}

fn lower_expr(node: tree_sitter::Node, source: SourceId, text: &str) -> Expr {
    let span = span_of(node, source);
    match node.kind() {
        "identifier" => Expr::new(
            ExprKind::Path(IrPath::single(Arc::from(text_of(node, text)))),
            span,
        ),
        "number" => Expr::int(text_of(node, text), span),
        "parenthesized_expression" => {
            let mut ci = 0;
            let mut inner = None;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci)
                    && c.kind() != "("
                    && c.kind() != ")"
                {
                    inner = Some(lower_expr(c, source, text));
                }
                ci += 1;
            }
            inner.unwrap_or(Expr::null(span))
        }
        "binary_expression" => {
            let mut left = Expr::null(span);
            let mut right = Expr::null(span);
            let mut op_text = "+";
            // fields via node fields safer
            if let Some(l) = node.child_by_field_name("left") {
                left = lower_expr(l, source, text);
            }
            if let Some(r) = node.child_by_field_name("right") {
                right = lower_expr(r, source, text);
            }
            if let Some(o) = node.child_by_field_name("operator") {
                op_text = text_of(o, text);
            }
            let op = match op_text {
                "+" => BinaryOp::Add,
                "-" => BinaryOp::Sub,
                "*" => BinaryOp::Mul,
                "/" => BinaryOp::Div,
                "%" => BinaryOp::Rem,
                "<" => {
                    return Expr::new(
                        ExprKind::Compare {
                            op: ComparisonOp::Lt,
                            lhs: Box::new(left),
                            rhs: Box::new(right),
                        },
                        span,
                    );
                }
                "<=" => {
                    return Expr::new(
                        ExprKind::Compare {
                            op: ComparisonOp::Le,
                            lhs: Box::new(left),
                            rhs: Box::new(right),
                        },
                        span,
                    );
                }
                ">" => {
                    return Expr::new(
                        ExprKind::Compare {
                            op: ComparisonOp::Gt,
                            lhs: Box::new(left),
                            rhs: Box::new(right),
                        },
                        span,
                    );
                }
                ">=" => {
                    return Expr::new(
                        ExprKind::Compare {
                            op: ComparisonOp::Ge,
                            lhs: Box::new(left),
                            rhs: Box::new(right),
                        },
                        span,
                    );
                }
                "===" | "==" => {
                    return Expr::new(
                        ExprKind::Compare {
                            op: ComparisonOp::Eq,
                            lhs: Box::new(left),
                            rhs: Box::new(right),
                        },
                        span,
                    );
                }
                "!=" | "!==" => {
                    return Expr::new(
                        ExprKind::Compare {
                            op: ComparisonOp::Ne,
                            lhs: Box::new(left),
                            rhs: Box::new(right),
                        },
                        span,
                    );
                }
                "&&" => BinaryOp::BitAnd, // placeholder
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
        "call_expression" => {
            let callee = match node
                .child_by_field_name("function")
                .or_else(|| node.child(0))
            {
                Some(c) => lower_expr(c, source, text),
                None => Expr::path("undefined", span),
            };
            let mut args = Vec::new();
            let mut i = 0;
            while i < node.child_count() {
                if let Some(c) = node.child(i)
                    && c.kind() == "arguments"
                {
                    let mut j = 0;
                    while j < c.child_count() {
                        if let Some(cc) = c.child(j) {
                            let k = cc.kind();
                            if k != "(" && k != ")" && k != "," {
                                args.push(lower_expr(cc, source, text));
                            }
                        }
                        j += 1;
                    }
                }
                i += 1;
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
        _ => Expr::error(format!("unsupported JS expression: {}", node.kind()), span),
    }
}
