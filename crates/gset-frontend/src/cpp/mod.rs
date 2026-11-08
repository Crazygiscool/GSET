//! The C frontend.

use crate::Frontend;
use gset_ir::{
    BinaryOp, Block, ComparisonOp, DiagnosticBag, Else, Expr, ExprKind, Function, Item, LangId,
    Literal, LogicalOp, Lowered, Module, Name, Pattern, SourceId, SourceMap, Span, Stmt, UnaryOp,
    VarDecl,
};
use std::sync::Arc;

pub struct Cpp;

impl Frontend for Cpp {
    fn lang(&self) -> LangId {
        LangId::CPP
    }
    fn name(&self) -> &'static str {
        "cpp"
    }
    fn handles_extension(&self, path: &std::path::Path) -> bool {
        matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("cpp" | "cxx" | "cc" | "hpp" | "hxx" | "hh")
        )
    }
    fn lower(&self, source: &str, text: &str) -> Lowered {
        let mut source_map = SourceMap::new();
        let id = source_map.add(source.to_string(), text.to_string());

        let mut parser = tree_sitter::Parser::new();
        if let Err(error) = parser.set_language(&tree_sitter_cpp::LANGUAGE.into()) {
            let mut d = DiagnosticBag::new();
            d.push(gset_ir::Diagnostic::error(
                Span::synthetic(),
                format!("cannot load cpp grammar: {error}"),
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
                    "cpp parser produced no syntax tree",
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
                    "function_definition" => {
                        if let Some(item) = lower_function(child, id, text) {
                            items.push(item);
                        }
                    }
                    "declaration" => {
                        if let Some(item) = lower_global(child, id, text) {
                            items.push(item);
                        }
                    }
                    _ => {}
                }
            }
            ci += 1;
        }
        Lowered {
            module: Some(Module {
                items,
                lang: LangId::CPP,
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

fn lower_function(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Item> {
    let declarator = node.child_by_field_name("declarator")?;
    let name_node = first_identifier(declarator)?;
    let name: Name = Arc::from(text_of(name_node, text));
    let mut params = Vec::new();
    if let Some(params_node) = declarator.child_by_field_name("parameters").or_else(|| {
        let mut ci = 0;
        while ci < declarator.child_count() {
            if let Some(child) = declarator.child(ci) {
                if child.kind() == "parameter_list" {
                    return Some(child);
                }
            }
            ci += 1;
        }
        None
    }) {
        let mut ci = 0;
        while ci < params_node.child_count() {
            if let Some(param) = params_node.child(ci) {
                if param.kind() == "parameter_declaration" {
                    if let Some(name_node) = first_identifier(param) {
                        params.push(Pattern::bind(
                            Arc::from(text_of(name_node, text)),
                            span_of(name_node, source),
                        ));
                    }
                }
            }
            ci += 1;
        }
    }
    let body_node = node.child_by_field_name("body")?;
    let body = lower_block(body_node, source, text);
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
    let decl = lower_declaration(node, source, text)?;
    match decl {
        Stmt::Decl(var) => Some(Item::Global(var)),
        _ => None,
    }
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
            if child.kind() == "declaration" {
                if let Some(stmt) = lower_declaration(child, source, text) {
                    statements.push(stmt);
                }
            } else if let Some(stmt) = lower_stmt(child, source, text) {
                statements.push(stmt);
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
        "compound_statement" => Some(Stmt::Block(lower_block(node, source, text))),
        "declaration" => lower_declaration(node, source, text),
        "expression_statement" => {
            if let Some(child) = named_child(node, 0) {
                if child.kind() == "binary_expression"
                    && text_of(child, text).contains("cout")
                    && text_of(child, text).contains("<<")
                {
                    let mut operands = Vec::new();
                    if collect_cout_operands(child, text, &mut operands) {
                        let args: Vec<Expr> = operands
                            .into_iter()
                            .filter(|expr_node| {
                                let t = text_of(*expr_node, text).trim();
                                t != "std::endl" && t != "endl"
                            })
                            .map(|expr_node| lower_expr(expr_node, source, text))
                            .collect();
                        if !args.is_empty() {
                            let callee = Expr::new(
                                ExprKind::Path(gset_ir::Path::single(Arc::from("print"))),
                                span_of(child, source),
                            );
                            return Some(Stmt::Expr(Expr::new(
                                ExprKind::Call {
                                    callee: Box::new(callee),
                                    args,
                                    named_args: Vec::new(),
                                },
                                span_of(node, source),
                            )));
                        }
                    }
                }
            }
            let expr = named_child(node, 0).map(|c| lower_expr(c, source, text))?;
            Some(Stmt::Expr(expr))
        }
        "return_statement" => {
            let mut value = None;
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(child) = node.child(ci) {
                    if child.is_named() && child.kind() != "return" {
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
        "if_statement" => {
            let condition_node = node.child_by_field_name("condition")?;
            let condition = lower_condition(condition_node, source, text);
            let consequence = node.child_by_field_name("consequence")?;
            let then_branch = statement_to_block(consequence, source, text);
            let else_branch = node.child_by_field_name("alternative").and_then(|alt| {
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
                if first.kind() == "compound_statement" {
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
        "while_statement" => {
            let condition_node = node.child_by_field_name("condition")?;
            let condition = lower_condition(condition_node, source, text);
            let body_node = node.child_by_field_name("body")?;
            let body = statement_to_block(body_node, source, text);
            Some(Stmt::While {
                condition,
                body,
                else_body: None,
                span: span_of(node, source),
            })
        }
        "do_statement" => {
            let body_node = node.child_by_field_name("body")?;
            let body = statement_to_block(body_node, source, text);
            let condition_node = node.child_by_field_name("condition")?;
            let condition = lower_condition(condition_node, source, text);
            Some(Stmt::DoWhile {
                body,
                condition,
                span: span_of(node, source),
            })
        }
        "for_statement" => {
            let init = node.child_by_field_name("initializer").and_then(|n| {
                if n.kind() == "declaration" {
                    lower_declaration(n, source, text)
                } else {
                    Some(Stmt::Expr(lower_expr(n, source, text)))
                }
            });
            let condition = node
                .child_by_field_name("condition")
                .map(|n| lower_expr(n, source, text));
            let update = node
                .child_by_field_name("update")
                .map(|n| lower_expr(n, source, text));
            let body_node = node.child_by_field_name("body")?;
            let body = statement_to_block(body_node, source, text);
            Some(Stmt::For {
                init: init.map(Box::new),
                condition,
                update,
                body,
                label: None,
                span: span_of(node, source),
            })
        }
        "break_statement" => Some(Stmt::Break {
            label: None,
            span: span_of(node, source),
        }),
        "continue_statement" => Some(Stmt::Continue {
            label: None,
            span: span_of(node, source),
        }),
        "empty_statement" | "null_statement" => Some(Stmt::Empty {
            span: span_of(node, source),
        }),
        _ => None,
    }
}

fn statement_to_block(node: tree_sitter::Node, source: SourceId, text: &str) -> Block {
    if node.kind() == "compound_statement" {
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

fn lower_declaration(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Stmt> {
    let declarator_field = node.child_by_field_name("declarator")?;
    let (decl_node, value_node) = if declarator_field.kind() == "init_declarator" {
        (
            declarator_field.child_by_field_name("declarator")?,
            declarator_field.child_by_field_name("value"),
        )
    } else {
        (declarator_field, None)
    };
    let target_node = first_identifier(decl_node)?;
    let target = Pattern::bind(
        Arc::from(text_of(target_node, text)),
        span_of(target_node, source),
    );
    let value = value_node.map(|n| lower_expr(n, source, text));
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
    match node.kind() {
        "true" => Expr::new(ExprKind::Literal(Literal::Bool(true)), span),
        "false" => Expr::new(ExprKind::Literal(Literal::Bool(false)), span),
        "NULL" | "nullptr" => Expr::new(ExprKind::Literal(Literal::Null), span),
        "identifier" => Expr::new(
            ExprKind::Path(gset_ir::Path::single(Arc::from(text_of(node, text)))),
            span,
        ),
        "number" | "number_literal" => {
            let s = text_of(node, text);
            if s.contains('.') || s.contains('e') || s.contains('E') {
                Expr::new(ExprKind::Literal(Literal::Float(s.into())), span)
            } else {
                Expr::int(s, span)
            }
        }
        "string_literal" => {
            let raw = text_of(node, text).trim();
            let content = if (raw.starts_with('"') && raw.ends_with('"'))
                || (raw.starts_with('\'') && raw.ends_with('\''))
            {
                &raw[1..raw.len().saturating_sub(1)]
            } else {
                raw
            };
            Expr::new(ExprKind::Literal(Literal::Str(content.into())), span)
        }
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
        "assignment_expression" => {
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
        "subscript_expression" => {
            let arg = node
                .child_by_field_name("argument")
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
        "cast_expression" => {
            // Ignore the target type and lower the value.
            node.child_by_field_name("value")
                .or_else(|| named_child(node, 1))
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
        _ => Expr::error(format!("unsupported C++ expression: {}", node.kind()), span),
    }
}

fn lower_condition(node: tree_sitter::Node, source: SourceId, text: &str) -> Expr {
    if node.kind() == "condition_clause" {
        let mut current = Some(node);
        while let Some(cur) = current {
            let mut first = None;
            let mut i = 0;
            while i < cur.child_count() {
                if let Some(child) = cur.child(i) {
                    if child.is_named() {
                        first = Some(child);
                        break;
                    }
                }
                i += 1;
            }
            match first {
                Some(child) if child.kind() == "condition_clause" => current = Some(child),
                Some(child) => return lower_expr(child, source, text),
                None => return Expr::null(span_of(cur, source)),
            }
        }
    }
    lower_expr(node, source, text)
}

fn collect_cout_operands<'a>(
    node: tree_sitter::Node<'a>,
    text: &str,
    out: &mut Vec<tree_sitter::Node<'a>>,
) -> bool {
    if node.kind() != "binary_expression" {
        let t = text_of(node, text).trim();
        return t == "std::cout" || t == "cout";
    }
    let mut lhs = None;
    let mut rhs = None;
    let mut op_text = "";
    let mut ci = 0;
    while ci < node.child_count() {
        if let Some(c) = node.child(ci) {
            match c.kind() {
                "<<" => op_text = "<<",
                _ => {
                    if c.is_named() {
                        if lhs.is_none() {
                            lhs = Some(c);
                        } else {
                            rhs = Some(c);
                        }
                    }
                }
            }
        }
        ci += 1;
    }
    let (Some(lhs), Some(rhs)) = (lhs, rhs) else {
        return false;
    };
    if op_text != "<<" {
        return false;
    }
    if !collect_cout_operands(lhs, text, out) {
        return false;
    }
    let t = text_of(rhs, text).trim();
    if t != "std::endl" && t != "endl" {
        out.push(rhs);
    }
    true
}
