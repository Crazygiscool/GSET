//! The Go frontend.

use crate::Frontend;
use gset_ir::DiagnosticBag;
use gset_ir::{
    BinaryOp, Block, ComparisonOp, Diagnostic, Expr, ExprKind, Function, Item, LangId, Literal,
    Lowered, Module, Name, Path, Pattern, SourceId, SourceMap, Span, Stmt, VarDecl,
};
use std::sync::Arc;

/// The Go frontend.
pub struct Go;

impl Frontend for Go {
    fn lang(&self) -> LangId {
        LangId::GO
    }
    fn name(&self) -> &'static str {
        "go"
    }
    fn handles_extension(&self, path: &std::path::Path) -> bool {
        let ext = match path.extension() {
            Some(ext) => ext,
            None => return false,
        };
        matches!(ext.to_str(), Some("go"))
    }
    fn lower(&self, source: &str, text: &str) -> Lowered {
        let mut source_map = SourceMap::new();
        let id = source_map.add(source.to_string(), text.to_string());

        let mut parser = tree_sitter::Parser::new();
        if let Err(error) = parser.set_language(&tree_sitter_go::LANGUAGE.into()) {
            let mut d = DiagnosticBag::new();
            d.push(Diagnostic::error(
                Span::synthetic(),
                format!("cannot load go grammar: {error}"),
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
                let mut d = DiagnosticBag::new();
                d.push(Diagnostic::error(
                    Span::point(id, 0),
                    "go parser produced no syntax tree",
                ));
                return Lowered {
                    module: None,
                    diagnostics: d,
                    source_map,
                };
            }
        };
        let root = tree.root_node();

        let mut collect = gset_ir::DiagnosticBag::new();
        collect_syntax_diagnostics(root, text, id, &mut collect);
        let mut diagnostics = collect;

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
                    "method_declaration" => {
                        if let Some(item) = lower_method(child, id, text) {
                            items.push(item);
                        }
                    }
                    "type_declaration" => {}
                    "short_var_declaration" => {
                        if let Some(stmt) = lower_short_var(child, id, text) {
                            items.push(Item::Stmt(stmt));
                        }
                    }
                    "expression_statement" => {
                        if let Some(stmt) = lower_expr_statement(child, id, text) {
                            items.push(Item::Stmt(stmt));
                        }
                    }
                    "package_clause" | "import_declaration" => {}
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
            let mut m = Module::empty(LangId::GO, span_of(root, id));
            m.items = items;
            Some(m)
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
    out: &mut DiagnosticBag,
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
    let params_node = node.child_by_field_name("parameters");
    let body_node = node.child_by_field_name("body")?;

    let name: Name = Arc::from(text_of(name_node, text));
    let mut params = Vec::new();
    if let Some(pn) = params_node {
        let mut ci = 0;
        while ci < pn.child_count() {
            if let Some(c) = pn.child(ci)
                && c.kind() == "parameter_declaration"
            {
                let mut id = None;
                let mut j = 0;
                while j < c.child_count() {
                    if let Some(cc) = c.child(j)
                        && cc.kind() == "identifier"
                    {
                        id = Some(cc);
                        break;
                    }
                    j += 1;
                }
                if let Some(nn) = id {
                    let pname = text_of(nn, text);
                    params.push(Pattern::bind(Arc::from(pname), span_of(nn, source)));
                }
            }
            ci += 1;
        }
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

fn lower_method(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Item> {
    let mut receiver_param = None;
    let mut method_name = node.child_by_field_name("name"); // might be field identifier
    let params_node = node.child_by_field_name("parameters");
    let body_node = node.child_by_field_name("body");
    if method_name.is_none() {
        let mut ci = 0;
        while ci < node.child_count() {
            if let Some(c) = node.child(ci)
                && c.kind() == "field_identifier"
            {
                method_name = Some(c);
            }
            ci += 1;
        }
    }
    let method_name = {
        let n = method_name?;
        text_of(n, text)
    };

    // Receiver parameter.
    if let Some(pn) = node.child_by_field_name("receiver") {
        if let Some(c) = pn.child_by_field_name("name").or_else(|| {
            let mut j = 0;
            while j < pn.child_count() {
                if let Some(cc) = pn.child(j)
                    && cc.kind() == "identifier"
                {
                    return Some(cc);
                }
                j += 1;
            }
            None
        }) {
            receiver_param = Some(c);
        } else if let Some(_first) = pn.child(0) {
            let mut j = 0;
            while j < pn.child_count() {
                if let Some(cc) = pn.child(j)
                    && cc.kind() == "identifier"
                {
                    receiver_param = Some(cc);
                    break;
                }
                j += 1;
            }
        }
    }

    let name: Name = Arc::from(method_name);
    let mut params = Vec::new();
    if let Some(rpv) = receiver_param {
        let pname = text_of(rpv, text);
        params.push(Pattern::bind(Arc::from(pname), span_of(rpv, source)));
    }
    if let Some(pn) = params_node {
        let mut ci = 0;
        while ci < pn.child_count() {
            if let Some(c) = pn.child(ci)
                && c.kind() == "parameter_declaration"
            {
                let mut id = None;
                let mut j = 0;
                while j < c.child_count() {
                    if let Some(cc) = c.child(j)
                        && cc.kind() == "identifier"
                    {
                        id = Some(cc);
                        break;
                    }
                    j += 1;
                }
                if let Some(nn) = id {
                    let pname = text_of(nn, text);
                    params.push(Pattern::bind(Arc::from(pname), span_of(nn, source)));
                }
            }
            ci += 1;
        }
    }
    let body = match body_node {
        Some(b) => lower_block(b, source, text),
        None => Block::empty(span_of(node, source)),
    };
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

fn lower_short_var(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Stmt> {
    let span = span_of(node, source);
    let mut target = None;
    let mut init = None;
    let mut ci = 0;
    while ci < node.child_count() {
        if let Some(c) = node.child(ci) {
            if c.kind() == "expression_list" {
                if target.is_none() {
                    // first expression_list is LHS names
                    let mut j = 0;
                    while j < c.child_count() {
                        if let Some(cc) = c.child(j)
                            && cc.kind() == "identifier"
                        {
                            target = Some(Pattern::bind(
                                Arc::from(text_of(cc, text)),
                                span_of(cc, source),
                            ));
                            break;
                        }
                        j += 1;
                    }
                } else {
                    if let Some(cc) = c.child(0) {
                        init = Some(lower_expr(cc, source, text));
                    }
                }
            } else if target.is_none() && c.kind() == "identifier" {
                target = Some(Pattern::bind(
                    Arc::from(text_of(c, text)),
                    span_of(c, source),
                ));
            } else if init.is_none()
                && c.kind() != ":="
                && c.is_named()
                && c.kind() != "expression_list"
            {
                init = Some(lower_expr(c, source, text));
            }
        }
        ci += 1;
    }
    let target = target.unwrap_or(Pattern::bind(Arc::from("_"), span));
    Some(Stmt::Decl(VarDecl {
        pattern: target,
        ty: None,
        value: init,
        mutable: true,
        exported: false,
        span,
    }))
}

fn lower_block(node: tree_sitter::Node, source: SourceId, text: &str) -> Block {
    let mut stmts = Vec::new();
    let mut ci = 0;
    while ci < node.child_count() {
        if let Some(c) = node.child(ci) {
            if c.kind() == "statement_list" {
                let mut j = 0;
                while j < c.child_count() {
                    if let Some(cc) = c.child(j)
                        && let Some(s) = lower_stmt(cc, source, text)
                    {
                        stmts.push(s);
                    }
                    j += 1;
                }
            } else if let Some(s) = lower_stmt(c, source, text) {
                stmts.push(s);
            }
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
                    if c.kind() == "expression_list" {
                        let mut j = 0;
                        while j < c.child_count() {
                            if let Some(ec) = c.child(j) {
                                v = Some(lower_expr(ec, source, text));
                                break;
                            }
                            j += 1;
                        }
                    } else {
                        v = Some(lower_expr(c, source, text));
                    }
                }
                ci += 1;
            }
            Some(Stmt::Return { value: v, span })
        }
        "if_statement" => {
            let mut condition = Expr::null(span);
            let mut then_branch = Block {
                statements: Vec::new(),
                span,
            };
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    if c.kind() == "binary_expression"
                        || c.kind() == "unary_expression"
                        || c.kind() == "call_expression"
                        || c.kind() == "selector_expression"
                        || c.kind() == "identifier"
                        || c.kind() == "parenthesized_expression"
                    {
                        if condition == Expr::null(span) {
                            condition = lower_expr(c, source, text);
                        }
                    } else if c.kind() == "block" {
                        then_branch = lower_block(c, source, text);
                    }
                }
                ci += 1;
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
            ExprKind::Path(Path::single(Arc::from(text_of(node, text)))),
            span,
        ),
        "int_literal" => Expr::int(text_of(node, text), span),
        "float_literal" => Expr::new(
            ExprKind::Literal(Literal::Float(text_of(node, text).into())),
            span,
        ),
        "interpreted_string_literal" | "raw_string_literal" => Expr::new(
            ExprKind::Literal(Literal::Str(text_of(node, text).into())),
            span,
        ),
        "unary_expression" | "binary_expression" => {
            let mut lhs = Expr::null(span);
            let mut rhs = Expr::null(span);
            let mut op = "+";
            let mut count = 0;
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    match c.kind() {
                        "+" | "-" | "*" | "/" | "%" | "==" | "!=" | "<" | ">" | "<=" | ">=" => {
                            op = text_of(c, text);
                        }
                        _ => {
                            if count == 0 {
                                lhs = lower_expr(c, source, text);
                            } else {
                                rhs = lower_expr(c, source, text);
                            }
                            count += 1;
                        }
                    }
                }
                ci += 1;
            }
            if count < 2 {
                return Expr::null(span);
            }
            match op {
                "+" => Expr::new(
                    ExprKind::Binary {
                        op: BinaryOp::Add,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                ),
                "-" => Expr::new(
                    ExprKind::Binary {
                        op: BinaryOp::Sub,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                ),
                "*" => Expr::new(
                    ExprKind::Binary {
                        op: BinaryOp::Mul,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                ),
                "/" => Expr::new(
                    ExprKind::Binary {
                        op: BinaryOp::Div,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                ),
                "%" => Expr::new(
                    ExprKind::Binary {
                        op: BinaryOp::Rem,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                ),
                "<" => Expr::new(
                    ExprKind::Compare {
                        op: ComparisonOp::Lt,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                ),
                ">" => Expr::new(
                    ExprKind::Compare {
                        op: ComparisonOp::Gt,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                ),
                "<=" => Expr::new(
                    ExprKind::Compare {
                        op: ComparisonOp::Le,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                ),
                ">=" => Expr::new(
                    ExprKind::Compare {
                        op: ComparisonOp::Ge,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                ),
                "==" => Expr::new(
                    ExprKind::Compare {
                        op: ComparisonOp::Eq,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                ),
                "!=" => Expr::new(
                    ExprKind::Compare {
                        op: ComparisonOp::Ne,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                ),
                _ => Expr::null(span),
            }
        }
        "call_expression" => {
            let mut callee = Expr::null(span);
            let mut args = Vec::new();
            let mut in_args = false;
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    if c.kind() == "identifier" && !in_args {
                        callee = Expr::new(
                            ExprKind::Path(Path::single(Arc::from(text_of(c, text)))),
                            span,
                        );
                    } else if c.kind() == "selector_expression" {
                        let mut parts = Vec::new();
                        let mut j = 0;
                        while j < c.child_count() {
                            if let Some(cc) = c.child(j)
                                && (cc.kind() == "identifier" || cc.kind() == "field_identifier")
                            {
                                parts.push(text_of(cc, text));
                            }
                            j += 1;
                        }
                        if parts.len() >= 2
                            && let Some(first) = parts.first()
                        {
                            if first == &"fmt" {
                                callee = Expr::new(
                                    ExprKind::Path(Path::single(Arc::from("print"))),
                                    span,
                                );
                            } else {
                                let joined = parts.join(".");
                                callee = Expr::new(
                                    ExprKind::Path(Path::single(Arc::from(joined))),
                                    span,
                                );
                            }
                        }
                    } else if c.kind() == "argument_list" {
                        in_args = true;
                        let mut j = 0;
                        while j < c.child_count() {
                            if let Some(cc) = c.child(j)
                                && cc.kind() != "("
                                && cc.kind() != ")"
                                && cc.kind() != ","
                            {
                                args.push(lower_expr(cc, source, text));
                            }
                            j += 1;
                        }
                    }
                }
                ci += 1;
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
        "selector_expression" => {
            // Fall back to a single path when the target is a fmt selector.
            let mut target = Expr::null(span);
            let mut field = String::new();
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    if (c.kind() == "identifier") && target == Expr::null(span) {
                        target = Expr::new(
                            ExprKind::Path(Path::single(Arc::from(text_of(c, text)))),
                            span,
                        );
                    } else if c.kind() == "field_identifier" {
                        field = text_of(c, text).to_string();
                    }
                }
                ci += 1;
            }
            if !field.is_empty() {
                // Represent as Expr::Field? Go backend already emits `.field`.
                // We keep it as a field to match the IR, but if the base path is
                // `fmt` we collapse to print in call handling only.
                Expr::new(
                    ExprKind::Field {
                        target: Box::new(target),
                        field: Arc::from(field),
                    },
                    span,
                )
            } else if target != Expr::null(span) {
                target
            } else {
                Expr::null(span)
            }
        }
        _ => Expr::error(format!("unsupported Go expression: {}", node.kind()), span),
    }
}
