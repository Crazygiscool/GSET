//! The C frontend.

use crate::Frontend;
use gset_ir::{
    BinaryOp, Block, Class, ComparisonOp, DiagnosticBag, Else, Expr, ExprKind, Function, Item,
    LangId, Literal, LogicalOp, Lowered, Module, Name, Pattern, SourceId, SourceMap, Span, Stmt,
    UnaryOp, VarDecl,
};
use std::sync::Arc;

pub struct Rust;

impl Frontend for Rust {
    fn lang(&self) -> LangId {
        LangId::RUST
    }
    fn name(&self) -> &'static str {
        "rust"
    }
    fn handles_extension(&self, path: &std::path::Path) -> bool {
        matches!(path.extension().and_then(|e| e.to_str()), Some("rs"))
    }
    fn lower(&self, source: &str, text: &str) -> Lowered {
        let mut source_map = SourceMap::new();
        let id = source_map.add(source.to_string(), text.to_string());

        let mut parser = tree_sitter::Parser::new();
        if let Err(error) = parser.set_language(&tree_sitter_rust::LANGUAGE.into()) {
            let mut d = DiagnosticBag::new();
            d.push(gset_ir::Diagnostic::error(
                Span::synthetic(),
                format!("cannot load rust grammar: {error}"),
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
                d.push(gset_ir::Diagnostic::error(
                    Span::point(id, 0),
                    "rust parser produced no syntax tree",
                ));
                return Lowered {
                    module: None,
                    diagnostics: d,
                    source_map,
                };
            }
        };
        let root = tree.root_node();
        let mut items = Vec::new();
        let mut ci = 0;
        while ci < root.child_count() {
            if let Some(child) = root.child(ci) {
                match child.kind() {
                    "function_item" => {
                        let is_main = child
                            .child_by_field_name("name")
                            .map(|node| text_of(node, text) == "main")
                            .unwrap_or(false);
                        if is_main {
                            if let Some(body_node) = child.child_by_field_name("body") {
                                let body = lower_block(body_node, id, text);
                                items.extend(body.statements.into_iter().map(Item::Stmt));
                            }
                        } else if let Some(item) = lower_function(child, id, text) {
                            items.push(item);
                        }
                    }
                    "const_item" | "static_item" => {
                        if let Some(item) = lower_global(child, id, text) {
                            items.push(item);
                        }
                    }
                    "struct_item" => {
                        if let Some(item) = lower_rust_struct(child, id, text) {
                            if let Item::Class(mut class) = item {
                                let mut next = ci + 1;
                                while next < root.child_count() {
                                    let Some(impl_node) = root.child(next) else {
                                        break;
                                    };
                                    if impl_node.kind() != "impl_item" {
                                        break;
                                    }
                                    let type_name = impl_node
                                        .child_by_field_name("type")
                                        .or_else(|| {
                                            (0..impl_node.child_count())
                                                .filter_map(|i| impl_node.child(i))
                                                .find(|n| n.kind() == "type_identifier")
                                        })
                                        .map(|n| text_of(n, text));
                                    if type_name == Some(class.name.as_ref()) {
                                        for mut method in lower_rust_impl(impl_node, id, text) {
                                            if method.name.to_string() == "new" {
                                                method.name = Arc::from("__init__");
                                                method.params.insert(
                                                    0,
                                                    Pattern::bind(Arc::from("self"), method.span),
                                                );
                                                class.constructors.push(method);
                                            } else {
                                                class.methods.push(method);
                                            }
                                        }
                                        next += 1;
                                        continue;
                                    }
                                    break;
                                }
                                items.push(Item::Class(class));
                                ci = next.saturating_sub(1);
                            }
                        }
                    }
                    "impl_item" => {}
                    "mod_item" => {
                        if let Some(name_node) = child.child_by_field_name("name") {
                            items.push(Item::Import(gset_ir::Import::module(
                                gset_ir::Path::single(Arc::from(text_of(name_node, text))),
                                span_of(child, id),
                            )));
                        }
                    }
                    "use_declaration" => {
                        if let Some(textnode) = (0..child.child_count())
                            .filter_map(|i| child.child(i))
                            .find(|n| {
                                matches!(n.kind(), "scoped_identifier" | "identifier" | "path")
                            })
                        {
                            let root_name = text_of(textnode, text)
                                .split("::")
                                .next()
                                .unwrap_or("Unknown");
                            items.push(Item::Import(gset_ir::Import::module(
                                gset_ir::Path::single(Arc::from(root_name)),
                                span_of(child, id),
                            )));
                        }
                    }
                    "enum_item" | "trait_item" => {}
                    _ => {}
                }
            }
            ci += 1;
        }
        Lowered {
            module: Some(Module {
                items,
                lang: LangId::RUST,
                span: span_of(root, id),
            }),
            diagnostics: DiagnosticBag::new(),
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

fn first_identifier(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    if node.kind() == "identifier" {
        return Some(node);
    }
    let mut ci = 0;
    while ci < node.child_count() {
        if let Some(child) = node.child(ci) {
            if let Some(found) = first_identifier(child) {
                return Some(found);
            }
        }
        ci += 1;
    }
    None
}

fn lower_rust_struct(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Item> {
    let name = node
        .child_by_field_name("name")
        .map(|n| Arc::from(text_of(n, text)))?;
    let mut fields = Vec::new();
    for i in 0..node.child_count() {
        let child = node.child(i)?;
        if child.kind() != "field_declaration_list" {
            continue;
        }
        for j in 0..child.child_count() {
            if let Some(field) = child.child(j) {
                if field.kind() == "field_declaration" {
                    if let Some(name_node) = field.child_by_field_name("name") {
                        fields.push(VarDecl {
                            pattern: Pattern::bind(
                                Arc::from(text_of(name_node, text)),
                                span_of(name_node, source),
                            ),
                            ty: None,
                            value: None,
                            mutable: false,
                            exported: false,
                            span: span_of(field, source),
                        });
                    }
                }
            }
        }
    }
    Some(Item::Class(Class {
        name,
        decorators: Vec::new(),
        extends: None,
        implements: Vec::new(),
        fields,
        constructors: Vec::new(),
        methods: Vec::new(),
        generics: Vec::new(),
        exported: false,
        span: span_of(node, source),
    }))
}

fn lower_rust_impl(node: tree_sitter::Node, source: SourceId, text: &str) -> Vec<Function> {
    let mut functions = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        for i in 0..body.child_count() {
            if let Some(child) = body.child(i) {
                if child.kind() == "function_item" {
                    if let Some(Item::Function(function)) = lower_function(child, source, text) {
                        functions.push(function);
                    }
                }
            }
        }
    } else {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if child.kind() == "function_item" {
                    if let Some(Item::Function(function)) = lower_function(child, source, text) {
                        functions.push(function);
                    }
                }
            }
        }
    }
    functions
}

fn lower_function(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Item> {
    let name_node = node.child_by_field_name("name")?;
    let name: Name = Arc::from(text_of(name_node, text));
    let mut params = Vec::new();
    if let Some(params_node) = node.child_by_field_name("parameters") {
        let mut i = 0;
        while i < params_node.child_count() {
            if let Some(param) = params_node.child(i) {
                if param.kind() == "self_parameter" {
                    params.push(Pattern::bind(Arc::from("self"), span_of(param, source)));
                } else if param.kind() == "parameter" || param.kind() == "variadic_parameter" {
                    if let Some(name_node) = first_identifier(param) {
                        params.push(Pattern::bind(
                            Arc::from(text_of(name_node, text)),
                            span_of(name_node, source),
                        ));
                    }
                }
            }
            i += 1;
        }
    }
    let body_node = node.child_by_field_name("body")?;
    let mut body = lower_block(body_node, source, text);
    if name.to_string() == "new" {
        if let Some(gset_ir::Stmt::Expr(expr)) = body.statements.last() {
            if let ExprKind::StructLit { fields, .. } = &expr.kind {
                let fields = fields.clone();
                body.statements.pop();
                for field in &fields {
                    let target_expr = Expr::new(
                        ExprKind::Field {
                            target: Box::new(Expr::new(
                                ExprKind::Path(gset_ir::Path::single(Arc::from("self"))),
                                span_of(body_node, source),
                            )),
                            field: field.name.clone(),
                        },
                        span_of(body_node, source),
                    );
                    body.statements.push(Stmt::Assign {
                        target: Pattern::location(target_expr, span_of(body_node, source)),
                        value: field.value.clone(),
                        span: span_of(body_node, source),
                    });
                }
            }
        }
    }
    if matches!(body.statements.last(), Some(gset_ir::Stmt::Expr(_))) {
        if let Some(gset_ir::Stmt::Expr(expr)) = body.statements.pop() {
            body.statements.push(gset_ir::Stmt::Return {
                value: Some(expr),
                span: span_of(body_node, source),
            });
        }
    }
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
        span: span_of(node, source),
    }))
}

fn lower_global(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Item> {
    let name_node = node.child_by_field_name("name")?;
    let value_node = node.child_by_field_name("value");
    let target = Pattern::bind(
        Arc::from(text_of(name_node, text)),
        span_of(name_node, source),
    );
    let value = value_node.map(|n| lower_expr(n, source, text));
    Some(Item::Global(VarDecl {
        pattern: target,
        ty: None,
        value,
        mutable: false,
        exported: false,
        span: span_of(node, source),
    }))
}

fn lower_block(node: tree_sitter::Node, source: SourceId, text: &str) -> Block {
    let mut statements = Vec::new();
    let mut ci = 0;
    while ci < node.child_count() {
        if let Some(child) = node.child(ci) {
            if child.kind() == "{" || child.kind() == "}" {
                ci += 1;
                continue;
            }
            if child.kind() == "let_declaration" {
                if let Some(stmt) = lower_let(child, source, text) {
                    statements.push(stmt);
                }
            } else if let Some(stmt) = lower_stmt(child, source, text) {
                statements.push(stmt);
            } else if child.is_named() {
                statements.push(Stmt::Expr(lower_expr(child, source, text)));
            }
        }
        ci += 1;
    }
    Block {
        statements,
        span: span_of(node, source),
    }
}

fn lower_stmt(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Stmt> {
    match node.kind() {
        "block" => Some(Stmt::Block(lower_block(node, source, text))),
        "let_declaration" => lower_let(node, source, text),
        "expression_statement" => {
            let expr_node = named_child(node, 0)?;
            if matches!(
                expr_node.kind(),
                "if_expression"
                    | "while_expression"
                    | "loop_expression"
                    | "for_expression"
                    | "return_expression"
            ) {
                lower_stmt(expr_node, source, text)
            } else {
                Some(Stmt::Expr(lower_expr(expr_node, source, text)))
            }
        }
        "return_expression" => {
            let mut value = None;
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(child) = node.child(ci) {
                    if child.is_named() {
                        value = Some(lower_expr(child, source, text));
                    }
                }
                ci += 1;
            }
            Some(Stmt::Return {
                value,
                span: span_of(node, source),
            })
        }
        "if_expression" => {
            let condition_node = node.child_by_field_name("condition")?;
            let condition = lower_expr(condition_node, source, text);
            let consequence = node.child_by_field_name("consequence")?;
            let then_branch = statement_to_block(consequence, source, text);
            let else_branch = node.child_by_field_name("alternative").and_then(|alt| {
                // else_clause
                let mut first: Option<tree_sitter::Node> = None;
                let mut ci = 0;
                while ci < alt.child_count() {
                    if let Some(child) = alt.child(ci) {
                        if child.is_named() {
                            first = Some(child);
                            break;
                        }
                    }
                    ci += 1;
                }
                let first = first?;
                if first.kind() == "block" {
                    Some(Box::new(Else::Block(lower_block(first, source, text))))
                } else {
                    lower_stmt(first, source, text).map(|s| {
                        Box::new(Else::Block(Block {
                            statements: vec![s],
                            span: span_of(first, source),
                        }))
                    })
                }
            });
            Some(Stmt::If {
                condition,
                then_branch,
                else_branch,
                span: span_of(node, source),
            })
        }
        "while_expression" => {
            let condition_node = node.child_by_field_name("condition")?;
            let condition = lower_expr(condition_node, source, text);
            let body_node = node.child_by_field_name("body")?;
            let body = statement_to_block(body_node, source, text);
            Some(Stmt::While {
                condition,
                body,
                else_body: None,
                span: span_of(node, source),
            })
        }
        "loop_expression" => {
            let body_node = node.child_by_field_name("body")?;
            let body = statement_to_block(body_node, source, text);
            Some(Stmt::While {
                condition: Expr::bool(true, span_of(node, source)),
                body,
                else_body: None,
                span: span_of(node, source),
            })
        }
        "for_expression" => {
            // Lower to an infinite while with body; condition/update approximated best-effort.
            let body_node = node.child_by_field_name("body")?;
            let body = statement_to_block(body_node, source, text);
            Some(Stmt::While {
                condition: Expr::bool(true, span_of(node, source)),
                body,
                else_body: None,
                span: span_of(node, source),
            })
        }
        "break_expression" => Some(Stmt::Break {
            label: None,
            span: span_of(node, source),
        }),
        "continue_expression" => Some(Stmt::Continue {
            label: None,
            span: span_of(node, source),
        }),
        _ => None,
    }
}

fn statement_to_block(node: tree_sitter::Node, source: SourceId, text: &str) -> Block {
    if node.kind() == "compound_statement" || node.kind() == "block" {
        lower_block(node, source, text)
    } else if let Some(stmt) = lower_stmt(node, source, text) {
        Block {
            statements: vec![stmt],
            span: span_of(node, source),
        }
    } else {
        Block {
            statements: Vec::new(),
            span: span_of(node, source),
        }
    }
}

fn lower_let(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Stmt> {
    let pattern_node = node.child_by_field_name("pattern")?;
    let target_node = first_identifier(pattern_node)?;
    let target = Pattern::bind(
        Arc::from(text_of(target_node, text)),
        span_of(target_node, source),
    );
    let value = node
        .child_by_field_name("value")
        .map(|n| lower_expr(n, source, text));
    Some(Stmt::Decl(VarDecl {
        pattern: target,
        ty: None,
        value,
        mutable: true,
        exported: false,
        span: span_of(node, source),
    }))
}

fn named_child(node: tree_sitter::Node, index: usize) -> Option<tree_sitter::Node> {
    let mut count = 0;
    let mut i = 0;
    while i < node.child_count() {
        if let Some(child) = node.child(i) {
            if child.is_named() {
                if count == index {
                    return Some(child);
                }
                count += 1;
            }
        }
        i += 1;
    }
    None
}

fn lower_expr(node: tree_sitter::Node, source: SourceId, text: &str) -> Expr {
    let span = span_of(node, source);
    if node.kind() == "scoped_identifier" {
        let text = text_of(node, text);
        if text.ends_with("::new") {
            let type_name = text.split("::").next().unwrap_or(text).to_string();
            return Expr::new(
                ExprKind::Path(gset_ir::Path::single(Arc::from(type_name))),
                span,
            );
        }
    }
    if node.kind() == "struct_expression" {
        let name = (0..node.child_count())
            .filter_map(|i| node.child(i))
            .find(|child| child.kind() == "type_identifier")
            .map(|child| text_of(child, text).to_string())
            .unwrap_or_default();
        let list = (0..node.child_count())
            .filter_map(|i| node.child(i))
            .find(|child| child.kind() == "field_initializer_list")
            .unwrap_or(node);
        let mut fields = Vec::new();
        let mut i = 0;
        while i < list.child_count() {
            if let Some(field) = list.child(i) {
                match field.kind() {
                    "field_initializer" => {
                        let field_name = field
                            .child_by_field_name("name")
                            .map(|n| text_of(n, text))
                            .unwrap_or("");
                        let value_node = field.child_by_field_name("value");
                        if let Some(value_node) = value_node {
                            let value = lower_expr(value_node, source, text);
                            fields.push(gset_ir::NamedArg {
                                name: gset_ir::Name::from(field_name.to_string()),
                                value,
                            });
                        }
                    }
                    "shorthand_field_initializer" => {
                        let field_name = text_of(field, text).to_string();
                        let value = Expr::new(
                            ExprKind::Path(gset_ir::Path::single(Arc::from(field_name.clone()))),
                            span,
                        );
                        fields.push(gset_ir::NamedArg {
                            name: gset_ir::Name::from(field_name),
                            value,
                        });
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        return Expr::new(
            ExprKind::StructLit {
                path: gset_ir::Path::single(Arc::from(name)),
                fields,
            },
            span,
        );
    }
    match node.kind() {
        "identifier" | "self" => Expr::new(
            ExprKind::Path(gset_ir::Path::single(Arc::from(text_of(node, text)))),
            span,
        ),
        "scoped_identifier" | "path" => Expr::new(
            ExprKind::Path(gset_ir::Path::single(Arc::from(text_of(node, text)))),
            span,
        ),
        "integer_literal" | "float_literal" | "number" | "number_literal" => {
            let s = text_of(node, text);
            if s.contains('.') || s.contains('e') || s.contains('E') {
                Expr::new(ExprKind::Literal(Literal::Float(s.into())), span)
            } else {
                Expr::int(s, span)
            }
        }
        "string_literal" => Expr::new(
            ExprKind::Literal(Literal::Str(text_of(node, text).into())),
            span,
        ),
        "boolean_literal" => Expr::new(
            ExprKind::Literal(Literal::Bool(text_of(node, text).trim() == "true")),
            span,
        ),
        "char_literal" => Expr::new(
            ExprKind::Literal(Literal::Str(text_of(node, text).into())),
            span,
        ),
        "parenthesized_expression" => {
            let mut i = 0;
            while i < node.child_count() {
                if let Some(child) = node.child(i) {
                    if child.is_named() {
                        return lower_expr(child, source, text);
                    }
                }
                i += 1;
            }
            Expr::null(span)
        }
        "assignment_expression" | "compound_assignment_expr" => {
            let left = node
                .child_by_field_name("left")
                .or_else(|| named_child(node, 0));
            let right = node
                .child_by_field_name("right")
                .or_else(|| named_child(node, 2));
            let op_node = node.child_by_field_name("operator");
            let op_text = op_node.map(|n| text_of(n, text)).unwrap_or("=").trim();
            let value = right
                .map(|n| lower_expr(n, source, text))
                .unwrap_or_else(|| Expr::null(span));
            // For compound assignment, reuse binary expression as value is not exact,
            // but keeps output valid and readable.
            let (previous, op) = if op_text == "=" {
                (None, None)
            } else {
                let base = op_text.strip_suffix('=').unwrap_or(op_text);
                let binary_op = match base {
                    "+" => Some(BinaryOp::Add),
                    "-" => Some(BinaryOp::Sub),
                    "*" => Some(BinaryOp::Mul),
                    "/" => Some(BinaryOp::Div),
                    "%" => Some(BinaryOp::Rem),
                    "&" => None,
                    "|" => None,
                    "^" => None,
                    "<<" => None,
                    ">>" => None,
                    _ => None,
                };
                if binary_op.is_some() {
                    let left_expr = left
                        .map(|n| lower_expr(n, source, text))
                        .unwrap_or_else(|| Expr::null(span));
                    (Some(Box::new(left_expr)), binary_op)
                } else {
                    (None, None)
                }
            };
            let target_pattern = if let Some(left_node) = left {
                if left_node.kind() == "identifier" {
                    Pattern::bind(
                        Arc::from(text_of(left_node, text)),
                        span_of(left_node, source),
                    )
                } else {
                    Pattern::ignore(span)
                }
            } else {
                Pattern::ignore(span)
            };
            Expr::new(
                ExprKind::Assign {
                    target: Box::new(target_pattern),
                    previous,
                    op,
                    value: Box::new(value),
                },
                span,
            )
        }
        "conditional_expression" => {
            let condition = named_child(node, 0)
                .map(|n| lower_expr(n, source, text))
                .unwrap_or_else(|| Expr::null(span));
            let then_branch = named_child(node, 1)
                .map(|n| lower_expr(n, source, text))
                .unwrap_or_else(|| Expr::null(span));
            let else_branch = named_child(node, 2)
                .map(|n| lower_expr(n, source, text))
                .unwrap_or_else(|| Expr::null(span));
            Expr::new(
                ExprKind::Conditional {
                    condition: Box::new(condition),
                    then_branch: Box::new(then_branch),
                    else_branch: Box::new(else_branch),
                },
                span,
            )
        }
        "binary_expression" => {
            let mut lhs = Expr::null(span);
            let mut rhs = Expr::null(span);
            let mut op_text = "+";
            let mut count = 0;
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    match c.kind() {
                        "+" | "-" | "*" | "/" | "%" | "==" | "!=" | "<" | ">" | "<=" | ">="
                        | "&&" | "||" | "&" | "|" | "^" | "<<" | ">>" => {
                            op_text = text_of(c, text);
                        }
                        _ => {
                            if c.is_named() {
                                if count == 0 {
                                    lhs = lower_expr(c, source, text);
                                } else {
                                    rhs = lower_expr(c, source, text);
                                }
                                count += 1;
                            }
                        }
                    }
                }
                ci += 1;
            }
            if count < 2 {
                return Expr::null(span);
            }
            match op_text {
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
                "&&" => Expr::new(
                    ExprKind::Logical {
                        op: LogicalOp::And,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                ),
                "||" => Expr::new(
                    ExprKind::Logical {
                        op: LogicalOp::Or,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                ),
                _ => Expr::error(format!("unsupported operator {op_text}"), span),
            }
        }
        "reference_expression" => {
            let inner = node
                .child_by_field_name("value")
                .or_else(|| named_child(node, 0));
            let operand = inner
                .map(|n| lower_expr(n, source, text))
                .unwrap_or_else(|| Expr::null(span));
            Expr::new(
                ExprKind::Unary {
                    op: UnaryOp::Ref,
                    operand: Box::new(operand),
                },
                span,
            )
        }
        "dereference_expression" => {
            let inner = node
                .child_by_field_name("value")
                .or_else(|| named_child(node, 0));
            let operand = inner
                .map(|n| lower_expr(n, source, text))
                .unwrap_or_else(|| Expr::null(span));
            Expr::new(
                ExprKind::Unary {
                    op: UnaryOp::Deref,
                    operand: Box::new(operand),
                },
                span,
            )
        }
        "unary_expression" => {
            // operator then operand, or operand then postfix operator.
            let mut op = "-";
            let mut operand: Option<Expr> = None;
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    match c.kind() {
                        "-" | "!" | "~" | "&" | "*" | "++" | "--" => {
                            op = text_of(c, text);
                            // Heuristic: operator before a named operand is prefix.
                        }
                        _ => {
                            if c.is_named() {
                                operand = Some(lower_expr(c, source, text));
                            }
                        }
                    }
                }
                ci += 1;
            }
            let operand = operand.unwrap_or_else(|| Expr::null(span));
            // Distinguish prefix/postfix by operator position relative to operand in textual order.
            // Default to prefix semantics; postfix ++/-- handled below by inspecting text.
            match op {
                "-" => Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span,
                ),
                "!" => Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    },
                    span,
                ),
                "~" => Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::BitNot,
                        operand: Box::new(operand),
                    },
                    span,
                ),
                "&" => Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Ref,
                        operand: Box::new(operand),
                    },
                    span,
                ),
                "*" => Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Deref,
                        operand: Box::new(operand),
                    },
                    span,
                ),
                "++" => {
                    if text_of(node, text).starts_with("++") {
                        Expr::new(
                            ExprKind::Unary {
                                op: UnaryOp::PreIncrement,
                                operand: Box::new(operand),
                            },
                            span,
                        )
                    } else {
                        Expr::new(
                            ExprKind::Unary {
                                op: UnaryOp::PostIncrement,
                                operand: Box::new(operand),
                            },
                            span,
                        )
                    }
                }
                "--" => {
                    if text_of(node, text).starts_with("--") {
                        Expr::new(
                            ExprKind::Unary {
                                op: UnaryOp::PreDecrement,
                                operand: Box::new(operand),
                            },
                            span,
                        )
                    } else {
                        Expr::new(
                            ExprKind::Unary {
                                op: UnaryOp::PostDecrement,
                                operand: Box::new(operand),
                            },
                            span,
                        )
                    }
                }
                _ => Expr::null(span),
            }
        }
        "call_expression" => {
            let callee_node = node
                .child_by_field_name("function")
                .or_else(|| named_child(node, 0));
            let mut callee = callee_node
                .map(|n| {
                    if n.kind() == "identifier" {
                        Expr::new(
                            ExprKind::Path(gset_ir::Path::single(Arc::from(text_of(n, text)))),
                            span_of(n, source),
                        )
                    } else {
                        lower_expr(n, source, text)
                    }
                })
                .unwrap_or_else(|| Expr::null(span));
            // Flatten member calls into a single path-ish callee for simplicity.
            if let Some(field) = callee_node.filter(|n| n.kind() == "field_expression") {
                // Keep as ExprKind::Field by reconstructing handled in lower_expr.
                callee = lower_expr(field, source, text);
            }
            let mut args = Vec::new();
            if let Some(args_node) = node.child_by_field_name("arguments") {
                let mut i = 0;
                while i < args_node.child_count() {
                    if let Some(child) = args_node.child(i) {
                        if child.is_named() {
                            args.push(lower_expr(child, source, text));
                        }
                    }
                    i += 1;
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
        "field_expression" => {
            let arg = node
                .child_by_field_name("argument")
                .or_else(|| node.child_by_field_name("value"))
                .map(|n| lower_expr(n, source, text))
                .unwrap_or_else(|| Expr::null(span));
            let field = node
                .child_by_field_name("field")
                .map(|n| text_of(n, text))
                .unwrap_or("");
            Expr::new(
                ExprKind::Field {
                    target: Box::new(arg),
                    field: Arc::from(field),
                },
                span,
            )
        }
        "subscript_expression" | "index_expression" => {
            let arg = node
                .child_by_field_name("argument")
                .or_else(|| node.child_by_field_name("value"))
                .map(|n| lower_expr(n, source, text))
                .unwrap_or_else(|| Expr::null(span));
            let index = node
                .child_by_field_name("index")
                .map(|n| lower_expr(n, source, text))
                .unwrap_or_else(|| Expr::null(span));
            Expr::new(
                ExprKind::Index {
                    target: Box::new(arg),
                    index: Box::new(index),
                },
                span,
            )
        }
        "cast_expression" | "type_cast_expression" => {
            // Ignore the target type and lower the value.
            node.child_by_field_name("value")
                .or_else(|| node.child_by_field_name("operand"))
                .or_else(|| named_child(node, 0))
                .map(|n| lower_expr(n, source, text))
                .unwrap_or_else(|| Expr::null(span))
        }
        "initializer_list" => {
            let mut elements = Vec::new();
            let mut i = 0;
            while i < node.child_count() {
                if let Some(child) = node.child(i) {
                    if child.is_named() {
                        elements.push(lower_expr(child, source, text));
                    }
                }
                i += 1;
            }
            Expr::new(ExprKind::List { elements }, span)
        }
        "macro_invocation" => {
            let mut name = "";
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(child) = node.child(ci) {
                    if child.kind() == "identifier" {
                        name = text_of(child, text);
                        break;
                    }
                }
                ci += 1;
            }
            let mut args = Vec::new();
            if name == "println" || name == "print" {
                let macro_text = text_of(node, text);
                if let Some(open) = macro_text.find('(') {
                    if let Some(close) = macro_text.rfind(')') {
                        let content = &macro_text[open + 1..close];
                        let mut comma = None;
                        let mut in_string = false;
                        let mut depth = 0usize;
                        let mut quote_escape = false;
                        for (offset, byte) in content.as_bytes().iter().enumerate() {
                            if in_string {
                                if quote_escape {
                                    quote_escape = false;
                                } else if *byte == b'\\' {
                                    quote_escape = true;
                                } else if *byte == b'"' {
                                    in_string = false;
                                }
                                continue;
                            }
                            match *byte {
                                b'"' => in_string = true,
                                b'(' | b'[' | b'{' => depth += 1,
                                b')' | b']' | b'}' => {
                                    if depth > 0 {
                                        depth -= 1;
                                    }
                                }
                                b',' => {
                                    if depth == 0 {
                                        comma = Some(offset);
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        if let Some(comma) = comma {
                            let snippet = content[comma + 1..].trim();
                            if !snippet.is_empty() {
                                let wrapper = format!("fn __gset_macro() {{ {} }}", snippet);
                                let mut parser = tree_sitter::Parser::new();
                                if parser
                                    .set_language(&tree_sitter_rust::LANGUAGE.into())
                                    .is_ok()
                                {
                                    if let Some(tree) = parser.parse(&wrapper, None) {
                                        let function = (0..tree.root_node().child_count())
                                            .find_map(|index| {
                                                tree.root_node()
                                                    .child(index)
                                                    .filter(|child| child.kind() == "function_item")
                                            });
                                        if let Some(function) = function {
                                            if let Some(body) = function.child_by_field_name("body")
                                            {
                                                let block = lower_block(
                                                    body,
                                                    SourceId::from_raw(0),
                                                    &wrapper,
                                                );
                                                if let Some(Stmt::Expr(expr)) =
                                                    block.statements.last()
                                                {
                                                    args.push(expr.clone());
                                                } else if let Some(Stmt::Return {
                                                    value: Some(expr),
                                                    ..
                                                }) = block.statements.last()
                                                {
                                                    args.push(expr.clone());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if name == "println" || name == "print" {
                if args.is_empty() {
                    Expr::null(span)
                } else {
                    Expr::new(
                        ExprKind::Call {
                            callee: Box::new(Expr::new(
                                ExprKind::Path(gset_ir::Path::single(Arc::from("print"))),
                                span,
                            )),
                            args,
                            named_args: Vec::new(),
                        },
                        span,
                    )
                }
            } else {
                Expr::error(format!("unsupported macro {}", name), span)
            }
        }
        "comma_expression" => {
            // Evaluate all, keep the last.
            let mut last = Expr::null(span);
            let mut i = 0;
            while i < node.child_count() {
                if let Some(child) = node.child(i) {
                    if child.is_named() {
                        last = lower_expr(child, source, text);
                    }
                }
                i += 1;
            }
            last
        }
        _ => Expr::error(
            format!("unsupported Rust expression: {}", node.kind()),
            span,
        ),
    }
}
