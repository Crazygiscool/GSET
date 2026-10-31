use crate::Frontend;
use gset_ir::{
    BinaryOp, Block, ComparisonOp, Diagnostic, Else, Expr, ExprKind, Function, Item, LangId,
    Literal, LogicalOp, Lowered, Module, Name, Path, Pattern, SourceId, SourceMap, Span, Stmt,
    Type, UnaryOp, VarDecl,
};
use std::sync::Arc;

pub struct Java;
impl Frontend for Java {
    fn lang(&self) -> LangId {
        LangId::JAVA
    }
    fn name(&self) -> &'static str {
        "java"
    }
    fn handles_extension(&self, path: &std::path::Path) -> bool {
        matches!(path.extension().and_then(|e| e.to_str()), Some("java"))
    }
    fn lower(&self, source: &str, text: &str) -> Lowered {
        let mut source_map = SourceMap::new();
        let id = source_map.add(source.to_string(), text.to_string());
        let mut parser = tree_sitter::Parser::new();
        if let Err(error) = parser.set_language(&tree_sitter_java::LANGUAGE.into()) {
            let mut d = gset_ir::DiagnosticBag::new();
            d.push(Diagnostic::error(
                Span::synthetic(),
                format!("cannot load java grammar: {error}"),
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
                    "java parser produced no syntax tree",
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
                    "class_declaration" => {
                        if let Some(item) = lower_class(child, id, text) {
                            items.push(item);
                        }
                    }
                    "package_declaration" | "import_declaration" | "comment" => {}
                    _ => diagnostics.push(Diagnostic::warning(
                        span_of(child, id),
                        format!("unsupported top-level construct: {}", child.kind()),
                    )),
                }
            }
            ci += 1;
        }
        let module = if diagnostics.has_errors() {
            None
        } else {
            let mut m = Module::empty(LangId::JAVA, span_of(root, id));
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

fn lower_class(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Item> {
    let name = node.child_by_field_name("name")?;
    let body = node.child_by_field_name("body")?;
    let mut methods = Vec::new();
    let mut ci = 0;
    while ci < body.child_count() {
        if let Some(child) = body.child(ci) {
            if child.kind() == "method_declaration" {
                if let Some(f) = lower_method(child, source, text) {
                    methods.push(f);
                }
            }
        }
        ci += 1;
    }
    Some(Item::Class(gset_ir::Class {
        name: gset_ir::Name::from(text_of(name, text).to_string()),
        decorators: Vec::new(),
        extends: None,
        implements: Vec::new(),
        fields: Vec::new(),
        methods,
        constructors: Vec::new(),
        exported: false,
        generics: Vec::new(),
        span: span_of(node, source),
    }))
}

fn lower_method(node: tree_sitter::Node, source: SourceId, text: &str) -> Option<Function> {
    let name = node.child_by_field_name("name")?;
    let params_node = node.child_by_field_name("parameters");
    let mut params = Vec::new();
    if let Some(pn) = params_node {
        let mut ci = 0;
        while ci < pn.child_count() {
            if let Some(c) = pn.child(ci) {
                if c.kind() == "formal_parameter" {
                    let nm = c.child_by_field_name("name").unwrap();
                    params.push(Pattern::bind(
                        Arc::from(text_of(nm, text)),
                        span_of(nm, source),
                    ));
                }
            }
            ci += 1;
        }
    }
    let body_node = node.child_by_field_name("body")?;
    let body = lower_block(Some(body_node), source, text);
    Some(Function {
        name: gset_ir::Name::from(text_of(name, text).to_string()),
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
    })
}

fn lower_block(node: Option<tree_sitter::Node>, source: SourceId, text: &str) -> Block {
    let Some(node) = node else {
        return Block {
            statements: Vec::new(),
            span: Span::synthetic(),
        };
    };
    let mut statements = Vec::new();
    let mut ci = 0;
    while ci < node.child_count() {
        if let Some(child) = node.child(ci) {
            if child.kind() != "{" && child.kind() != "}" && child.kind() != ";" {
                if let Some(s) = lower_stmt(child, source, text) {
                    statements.push(s);
                }
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
    let span = span_of(node, source);
    match node.kind() {
        "return_statement" => {
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
        "if_statement" => {
            let cond_node = node.child_by_field_name("condition")?;
            let inner = cond_node.named_child(0)?;
            let condition = lower_expr(inner, source, text);
            let then_node = node.child_by_field_name("consequence")?;
            let then_branch = if then_node.kind() == "block" {
                lower_block(Some(then_node), source, text)
            } else {
                Block {
                    statements: lower_stmt(then_node, source, text).into_iter().collect(),
                    span: span_of(then_node, source),
                }
            };
            let else_branch = if let Some(alt) = node.child_by_field_name("alternative") {
                Some(Box::new(Else::Block(if alt.kind() == "block" {
                    lower_block(Some(alt), source, text)
                } else {
                    Block {
                        statements: lower_stmt(alt, source, text).into_iter().collect(),
                        span: span_of(alt, source),
                    }
                })))
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
        "expression_statement" => {
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    if c.kind() != ";" {
                        return Some(Stmt::Expr(lower_expr(c, source, text)));
                    }
                }
                ci += 1;
            }
            None
        }
        "local_variable_declaration" | "variable_declaration" => None,
        _ => {
            // Try treat as expression.
            None
        }
    }
}

fn lower_expr(node: tree_sitter::Node, source: SourceId, text: &str) -> Expr {
    let span = span_of(node, source);
    match node.kind() {
        "integral_literal" | "decimal_integer_literal" => Expr::new(
            ExprKind::Literal(Literal::Int(text_of(node, text).to_string())),
            span,
        ),
        "string_literal" => {
            let body = text_of(node, text).trim_matches('"').to_string();
            Expr::new(ExprKind::Literal(Literal::Str(body)), span)
        }
        "true" => Expr::new(ExprKind::Literal(Literal::Bool(true)), span),
        "false" => Expr::new(ExprKind::Literal(Literal::Bool(false)), span),
        "parenthesized_expression" => {
            let inner = node.named_child(0).expect("paren inner");
            lower_expr(inner, source, text)
        }
        "binary_expression" => {
            let left = lower_expr(node.child_by_field_name("left").unwrap(), source, text);
            let op_node = node.child_by_field_name("operator").unwrap();
            let op_text = text_of(op_node, text);
            let right = lower_expr(node.child_by_field_name("right").unwrap(), source, text);
            if matches!(op_text, "==" | "!=" | "<" | "<=" | ">" | ">=") {
                let op = match op_text {
                    "==" => ComparisonOp::Eq,
                    "!=" => ComparisonOp::Ne,
                    "<" => ComparisonOp::Lt,
                    "<=" => ComparisonOp::Le,
                    ">" => ComparisonOp::Gt,
                    ">=" => ComparisonOp::Ge,
                    _ => unreachable!(),
                };
                Expr::new(
                    ExprKind::Compare {
                        op,
                        lhs: Box::new(left),
                        rhs: Box::new(right),
                    },
                    span,
                )
            } else if matches!(op_text, "&&" | "||") {
                let op = if op_text == "&&" {
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
        "unary_expression" => {
            let mut last = None;
            for i in 0..node.child_count() {
                if let Some(c) = node.child(i as u32) {
                    last = Some(c);
                }
            }
            let operand = lower_expr(last.unwrap(), source, text);
            Expr::new(
                ExprKind::Unary {
                    op: UnaryOp::Neg,
                    operand: Box::new(operand),
                },
                span,
            )
        }
        "method_invocation" => {
            let object = node.child_by_field_name("object");
            let name = node.child_by_field_name("name").unwrap();
            let mut name_txt = text_of(name, text).to_string();
            if let Some(obj) = object {
                if text_of(obj, text) == "System.out" && name_txt == "println" {
                    name_txt = "print".to_string();
                }
            }
            let args_node = node.child_by_field_name("arguments").unwrap();
            let mut args = Vec::new();
            let mut ci = 0;
            while ci < args_node.child_count() {
                if let Some(c) = args_node.child(ci) {
                    if c.kind() != "(" && c.kind() != ")" && c.kind() != "," {
                        args.push(lower_expr(c, source, text));
                    }
                }
                ci += 1;
            }
            let special_print = object.as_ref().map(|o| text_of(*o, text) == "System.out")
                == Some(true)
                && name_txt == "print";
            let callee = if special_print {
                Expr::path(Arc::from(name_txt.clone()), span)
            } else if let Some(obj) = object {
                Expr::new(
                    ExprKind::Field {
                        target: Box::new(lower_expr(obj, source, text)),
                        field: gset_ir::Name::from(name_txt.clone()),
                    },
                    span,
                )
            } else {
                Expr::path(Arc::from(name_txt.clone()), span)
            };
            Expr::new(
                ExprKind::Call {
                    callee: Box::new(callee),
                    args,
                    named_args: Vec::new(),
                },
                span,
            )
        }
        "field_access" => {
            let target = lower_expr(node.child_by_field_name("object").unwrap(), source, text);
            let field = node.child_by_field_name("field").unwrap();
            Expr::new(
                ExprKind::Field {
                    target: Box::new(target),
                    field: gset_ir::Name::from(text_of(field, text).to_string()),
                },
                span,
            )
        }
        "identifier" => Expr::path(Arc::from(text_of(node, text).to_string()), span),
        _ => {
            // Return null-ish default
            Expr::new(ExprKind::Literal(Literal::Null), span)
        }
    }
}

fn span_of(node: tree_sitter::Node, source: SourceId) -> Span {
    Span::new(source, node.start_byte() as u32, node.end_byte() as u32)
}
fn text_of<'a>(node: tree_sitter::Node, text: &'a str) -> &'a str {
    &text[node.start_byte()..node.end_byte()]
}
