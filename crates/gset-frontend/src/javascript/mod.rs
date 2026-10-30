//! The JavaScript frontend.

use gset_ir::{
    BinaryOp, Block, ComparisonOp, Diagnostic, Expr, ExprKind, Function, Item, LangId, Literal,
    Lowered, Module, Name, NamedArg, Path as IrPath, Pattern, SourceId, SourceMap, Span, Stmt,
    UnaryOp, VarDecl,
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
                    "for_statement" | "while_statement" | "for_in_statement" | "if_statement" => {
                        if let Some(stmt) = lower_stmt(child, id, text) {
                            items.push(Item::Stmt(stmt));
                        }
                    }
                    "lexical_declaration" | "variable_declaration" => {
                        lower_lexical_declaration(child, id, text, &mut items);
                    }
                    "program" => {}
                    "interface_declaration"
                    | "type_alias_declaration"
                    | "enum_declaration"
                    | "import_statement" => {}
                    "export_statement" => {
                        let mut ci = 0;
                        while ci < child.child_count() {
                            if let Some(c) = child.child(ci) {
                                match c.kind() {
                                    "function_declaration" => {
                                        if let Some(i) = lower_function(c, id, text) {
                                            items.push(i);
                                        }
                                    }
                                    "lexical_declaration" | "variable_declaration" => {
                                        lower_lexical_declaration(c, id, text, &mut items);
                                    }
                                    "expression_statement" => {
                                        if let Some(s) = lower_expr_statement(c, id, text) {
                                            items.push(Item::Stmt(s));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            ci += 1;
                        }
                    }
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
    let mut is_async = false;
    {
        let mut ci = 0;
        while ci < node.child_count() {
            if let Some(c) = node.child(ci)
                && c.kind() == "async"
            {
                is_async = true;
            }
            ci += 1;
        }
    }
    let mut params = Vec::new();
    let mut ci = 0;
    while ci < params_node.child_count() {
        if let Some(c) = params_node.child(ci) {
            let mut identifier = None;
            if c.kind() == "identifier" {
                identifier = Some(c);
            } else if c.kind() == "required_parameter"
                || c.kind() == "optional_parameter"
                || c.kind() == "rest_parameter"
            {
                let mut j = 0;
                while j < c.child_count() {
                    if let Some(cc) = c.child(j)
                        && cc.kind() == "identifier"
                        && identifier.is_none()
                    {
                        identifier = Some(cc);
                    }
                    j += 1;
                }
            }
            if let Some(nn) = identifier {
                let pname = text_of(nn, text);
                params.push(Pattern::bind(Arc::from(pname), span_of(nn, source)));
            }
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
        is_async,
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

fn lower_lexical_declaration(
    node: tree_sitter::Node,
    source: SourceId,
    text: &str,
    items: &mut Vec<Item>,
) {
    let span = span_of(node, source);
    let mut ci = 0;
    while ci < node.child_count() {
        if let Some(c) = node.child(ci)
            && c.kind() == "variable_declarator"
        {
            let mut name_node = c.child_by_field_name("name");
            if name_node.is_none() {
                let mut j = 0;
                while let Some(child) = c.child(j as u32) {
                    if child.kind() == "identifier" {
                        name_node = Some(child);
                        break;
                    }
                    j += 1;
                }
            }
            if let Some(nn) = name_node {
                let pname = text_of(nn, text);
                let value_node = c.child_by_field_name("value");
                let value = value_node.map(|vn| lower_expr(vn, source, text));
                items.push(Item::Global(VarDecl {
                    pattern: Pattern::bind(Arc::from(pname), span_of(nn, source)),
                    ty: None,
                    value,
                    mutable: true,
                    exported: false,
                    span,
                }));
            }
        }
        ci += 1;
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
        "for_statement" => {
            let mut init = None;
            let mut condition = None;
            let mut update = None;
            let mut body = Block {
                statements: Vec::new(),
                span,
            };
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    if c.kind() == "lexical_declaration" || c.kind() == "variable_declaration" {
                        let mut ci2 = 0;
                        while ci2 < c.child_count() {
                            if let Some(decl) = c.child(ci2)
                                && decl.kind() == "variable_declarator"
                            {
                                let mut name_node = decl.child_by_field_name("name");
                                if name_node.is_none() {
                                    let mut j = 0;
                                    while j < decl.child_count() {
                                        if let Some(ch) = decl.child(j)
                                            && ch.kind() == "identifier"
                                        {
                                            name_node = Some(ch);
                                        }
                                        j += 1;
                                    }
                                }
                                if let Some(nn) = name_node {
                                    let pname = text_of(nn, text);
                                    let value_node = decl.child_by_field_name("value");
                                    let value = value_node.map(|vn| lower_expr(vn, source, text));
                                    init = Some(Box::new(Stmt::Decl(VarDecl {
                                        pattern: Pattern::bind(
                                            Arc::from(pname),
                                            span_of(nn, source),
                                        ),
                                        ty: None,
                                        value,
                                        mutable: true,
                                        exported: false,
                                        span,
                                    })));
                                }
                            }
                            ci2 += 1;
                        }
                    } else if c.kind() == "binary_expression"
                        || c.kind() == "comparison_expression"
                        || c.kind() == "assignment_expression"
                        || c.kind() == "parenthesized_expression"
                    {
                        if condition.is_none() {
                            let e = if c.kind() == "parenthesized_expression" {
                                // use first child
                                let mut inner = None;
                                let mut j = 0;
                                while j < c.child_count() {
                                    if let Some(ec) = c.child(j)
                                        && ec.kind() != "("
                                        && ec.kind() != ")"
                                    {
                                        inner = Some(lower_expr(ec, source, text));
                                    }
                                    j += 1;
                                }
                                inner.unwrap_or(Expr::null(span))
                            } else {
                                lower_expr(c, source, text)
                            };
                            condition = Some(e);
                        }
                    } else if c.kind() == "update_expression" {
                        if let Some(ec) = c.child(0) {
                            update = Some(lower_expr(ec, source, text));
                        }
                    } else if c.kind() == "statement_block" {
                        body = lower_block(c, source, text);
                    } else if let Some(st) = lower_stmt(c, source, text) {
                        body = Block {
                            statements: vec![st],
                            span,
                        };
                    }
                }
                ci += 1;
            }
            // Map JS C-style for to a While loop so every backend that
            // understands while (and assignment) can run it.
            let mut statements = Vec::new();
            if let Some(i) = init {
                statements.push(*i);
            }
            let mut body_stmts = body.statements;
            if let Some(u) = update {
                body_stmts.push(Stmt::Expr(u));
            }
            statements.push(Stmt::While {
                condition: condition.unwrap_or(Expr::null(span)),
                body: Block {
                    statements: body_stmts,
                    span,
                },
                else_body: None,
                span,
            });
            Some(Stmt::Block(Block { statements, span }))
        }
        "for_in_statement" => {
            let mut p = String::from("x");
            let mut it = Expr::null(span);
            let mut body = Block {
                statements: Vec::new(),
                span,
            };
            let mut ci = 0;
            let mut found_binding = false;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    if c.kind() == "const" || c.kind() == "let" || c.kind() == "var" {
                        // skip
                    } else if c.kind() == "identifier" {
                        if !found_binding {
                            p = text_of(c, text).to_string();
                            found_binding = true;
                        } else if true {
                        }
                    } else if c.kind() == "statement_block" {
                        body = lower_block(c, source, text);
                    } else if c.kind() != "in"
                        && c.kind() != "of"
                        && c.kind() != "("
                        && c.kind() != ")"
                    {
                        // iterable expression
                        // avoid overriding p if simple identifier initial already used
                        if !found_binding {
                            // no op
                        } else {
                            // attempt to lower as expression
                            it = lower_expr(c, source, text);
                        }
                    }
                }
                ci += 1;
            }
            Some(Stmt::ForIn {
                pattern: Pattern::bind(Arc::from(p), span),
                iterable: it,
                body,
                else_body: None,
                is_parallel: false,
                is_async: false,
                label: None,
                span,
            })
        }
        "lexical_declaration" | "variable_declaration" => {
            let mut ci = 0;
            let mut out = Vec::new();
            while ci < node.child_count() {
                if let Some(c) = node.child(ci)
                    && c.kind() == "variable_declarator"
                {
                    let mut nn = c.child_by_field_name("name");
                    if nn.is_none() {
                        let mut j = 0;
                        while j < c.child_count() {
                            if let Some(child) = c.child(j)
                                && child.kind() == "identifier"
                            {
                                nn = Some(child);
                                break;
                            }
                            j += 1;
                        }
                    }
                    if let Some(nn) = nn {
                        let pname = text_of(nn, text);
                        let value_node = c.child_by_field_name("value");
                        let value = value_node.map(|vn| lower_expr(vn, source, text));
                        out.push(Stmt::Decl(VarDecl {
                            pattern: Pattern::bind(Arc::from(pname), span_of(nn, source)),
                            ty: None,
                            value,
                            mutable: true,
                            exported: false,
                            span,
                        }));
                    }
                }
                ci += 1;
            }
            if out.is_empty() {
                None
            } else if out.len() == 1 {
                Some(out.pop().unwrap())
            } else {
                Some(Stmt::Block(Block {
                    statements: out,
                    span,
                }))
            }
        }
        "while_statement" => {
            let mut cond_opt = None;
            let mut body = Block {
                statements: Vec::new(),
                span,
            };
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    if c.kind() == "parenthesized_expression" {
                        let mut inner = 0;
                        while inner < c.child_count() {
                            if let Some(ec) = c.child(inner)
                                && ec.kind() != "("
                                && ec.kind() != ")"
                            {
                                cond_opt = Some(lower_expr(ec, source, text));
                            }
                            inner += 1;
                        }
                    } else if c.kind() == "statement_block" {
                        body = lower_block(c, source, text);
                    } else if let Some(st) = lower_stmt(c, source, text) {
                        body = Block {
                            statements: vec![st],
                            span,
                        };
                    }
                }
                ci += 1;
            }
            Some(Stmt::While {
                condition: cond_opt.unwrap_or(Expr::null(span)),
                body,
                else_body: None,
                span,
            })
        }
        "expression_statement" => lower_expr_statement(node, source, text),
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
        "update_expression" => {
            let mut op = UnaryOp::PostIncrement;
            let mut operand = Expr::null(span);
            let mut i = 0;
            while i < node.child_count() {
                if let Some(c) = node.child(i) {
                    if c.kind() == "++" || c.kind() == "--" {
                        op = if c.kind() == "++" {
                            UnaryOp::PostIncrement
                        } else {
                            UnaryOp::PostDecrement
                        };
                    } else {
                        operand = lower_expr(c, source, text);
                    }
                }
                i += 1;
            }
            Expr::new(
                ExprKind::Unary {
                    op,
                    operand: Box::new(operand),
                },
                span,
            )
        }
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
        "assignment_expression" => {
            let (value, previous, op) = {
                let _target_node = node.child_by_field_name("left");
                let value_node = node.child_by_field_name("right");
                let op_text = match node.child_by_field_name("operator") {
                    Some(n) => text_of(n, text),
                    None => "=",
                };
                let previous = None;
                let op = match op_text {
                    "=" => None,
                    "+=" => Some(BinaryOp::Add),
                    "-=" => Some(BinaryOp::Sub),
                    "*=" => Some(BinaryOp::Mul),
                    "/=" => Some(BinaryOp::Div),
                    _ => None,
                };
                let val = match value_node {
                    Some(v) => lower_expr(v, source, text),
                    None => Expr::null(span),
                };
                (val, previous, op)
            };
            // target pattern
            let target_name = match node.child_by_field_name("left") {
                Some(n) if n.kind() == "identifier" => text_of(n, text),
                _ => "_",
            };
            Expr::new(
                ExprKind::Assign {
                    target: Box::new(Pattern::bind(Arc::from(target_name), span)),
                    previous,
                    op,
                    value: Box::new(value),
                },
                span,
            )
        }
        "call_expression" => {
            let callee_raw = match node
                .child_by_field_name("function")
                .or_else(|| node.child(0))
            {
                Some(c) => lower_expr(c, source, text),
                None => Expr::path("undefined", span),
            };
            let callee = if let ExprKind::Field { target, field } = &callee_raw.kind {
                if let ExprKind::Path(path) = &target.kind
                    && path.segments.len() == 1
                    && path.segments[0].as_ref() == "console"
                    && field.as_ref() == "log"
                {
                    Expr::path("print", span)
                } else {
                    callee_raw
                }
            } else {
                callee_raw
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
        "member_expression" => {
            let mut target = Expr::null(span);
            let mut index = 0usize;
            let mut seen_dot = false;
            while (index as u32) < node.child_count() {
                if let Some(c) = node.child(index as u32) {
                    if c.kind() == "." {
                        seen_dot = true;
                    } else if seen_dot {
                        // property, keep name
                    } else {
                        target = lower_expr(c, source, text);
                    }
                }
                index += 1;
            }
            let mut prop = String::from("prop");
            let mut index = 0usize;
            while (index as u32) < node.child_count() {
                if let Some(c) = node.child(index as u32)
                    && (c.kind() == "property_identifier" || c.kind() == "identifier")
                    && index > 0
                {
                    prop = text_of(c, text).to_string();
                }
                index += 1;
            }
            let name = std::sync::Arc::from(prop.as_str());
            Expr::new(
                ExprKind::Field {
                    target: Box::new(target),
                    field: name,
                },
                span,
            )
        }
        "subscript_expression" => {
            let mut target = Expr::null(span);
            let mut index_expr = Expr::null(span);
            let mut after = false;
            let mut i = 0;
            while i < node.child_count() {
                if let Some(c) = node.child(i) {
                    if c.kind() == "[" {
                        after = true;
                    } else if after && c.kind() == "]" {
                        after = false;
                    } else if !after
                        && matches!(target.kind, ExprKind::Literal(Literal::Null))
                        && (c.kind() == "identifier"
                            || c.kind() == "number"
                            || c.kind() == "member_expression"
                            || c.kind() == "subscript_expression"
                            || c.kind() == "array"
                            || c.kind() == "object"
                            || c.kind() == "call_expression")
                    {
                        target = lower_expr(c, source, text);
                    } else if after {
                        index_expr = lower_expr(c, source, text);
                    }
                }
                i += 1;
            }
            Expr::new(
                ExprKind::Index {
                    target: Box::new(target),
                    index: Box::new(index_expr),
                },
                span,
            )
        }
        "array" => {
            let mut elements = Vec::new();
            let mut i = 0;
            while i < node.child_count() {
                if let Some(c) = node.child(i)
                    && c.kind() != "["
                    && c.kind() != "]"
                    && c.kind() != ","
                {
                    elements.push(lower_expr(c, source, text));
                }
                i += 1;
            }
            Expr::new(ExprKind::List { elements }, span)
        }
        "object" => {
            let mut entries = Vec::new();
            let mut i = 0;
            while i < node.child_count() {
                if let Some(c) = node.child(i)
                    && c.kind() == "pair"
                {
                    let mut j = 0;
                    let mut key_name = String::from("prop");
                    let mut val_node = None;
                    let mut saw_colon = false;
                    while j < c.child_count() {
                        if let Some(cc) = c.child(j) {
                            if (cc.kind() == "property_identifier" || cc.kind() == "identifier")
                                && !saw_colon
                            {
                                key_name = text_of(cc, text).to_string();
                            }
                            if cc.kind() == ":" {
                                saw_colon = true;
                            } else if saw_colon {
                                val_node = Some(cc);
                                saw_colon = false;
                            }
                        }
                        j += 1;
                    }
                    let value = match val_node {
                        Some(v) => lower_expr(v, source, text),
                        None => Expr::null(span),
                    };
                    entries.push(NamedArg {
                        name: Arc::from(key_name.as_str()),
                        value,
                    });
                }
                i += 1;
            }
            Expr::new(ExprKind::Map { entries }, span)
        }
        "arrow_function" => {
            let mut params = Vec::new();
            let params_node = node.child_by_field_name("parameters");
            let body_node = node.child_by_field_name("body");
            if let Some(pn) = params_node {
                let mut j = 0;
                while j < pn.child_count() {
                    if let Some(c) = pn.child(j) {
                        if c.kind() == "identifier" {
                            params.push(Pattern::bind(
                                Arc::from(text_of(c, text)),
                                span_of(c, source),
                            ));
                        } else if c.kind() == "required_parameter"
                            || c.kind() == "optional_parameter"
                            || c.kind() == "rest_parameter"
                        {
                            let mut k = 0;
                            while k < c.child_count() {
                                if let Some(cc) = c.child(k)
                                    && cc.kind() == "identifier"
                                {
                                    params.push(Pattern::bind(
                                        Arc::from(text_of(cc, text)),
                                        span_of(cc, source),
                                    ));
                                    break;
                                }
                                k += 1;
                            }
                        }
                    }
                    j += 1;
                }
            } else {
                // single-param shorthand: identifier => expr
                let mut j = 0;
                while j < node.child_count() {
                    if let Some(c) = node.child(j)
                        && c.kind() == "identifier"
                    {
                        params.push(Pattern::bind(
                            Arc::from(text_of(c, text)),
                            span_of(c, source),
                        ));
                        break;
                    }
                    j += 1;
                }
            }
            let body_expr = match body_node {
                Some(b) if b.kind() != "=>" => {
                    if b.kind() == "statement_block" {
                        Expr::null(span)
                    } else {
                        lower_expr(b, source, text)
                    }
                }
                None => {
                    // expression body is the last named child not an identifier/arrow
                    let mut node_opt = None;
                    let mut j = 0;
                    while j < node.child_count() {
                        if let Some(c) = node.child(j)
                            && c.kind() != "=>"
                            && c.kind() != "formal_parameters"
                            && c.kind() != "identifier"
                            && c.is_named()
                        {
                            node_opt = Some(c);
                        }
                        j += 1;
                    }
                    match node_opt {
                        Some(b) => lower_expr(b, source, text),
                        None => Expr::null(span),
                    }
                }
                _ => Expr::null(span),
            };
            Expr::new(
                ExprKind::Lambda {
                    params,
                    body: Box::new(body_expr),
                },
                span,
            )
        }
        "await_expression" => {
            let mut inner = None;
            let mut ci = 0;
            while ci < node.child_count() {
                if let Some(c) = node.child(ci) {
                    if c.kind() == "await" {
                        // skip
                    } else {
                        inner = Some(lower_expr(c, source, text));
                    }
                }
                ci += 1;
            }
            Expr::new(
                ExprKind::Await {
                    expr: Box::new(inner.unwrap_or(Expr::null(span))),
                },
                span,
            )
        }
        _ => Expr::error(format!("unsupported JS expression: {}", node.kind()), span),
    }
}
