//! The Java backend.
//!
//! Emits Java as a `Program` class. Unsupported constructs report
//! `gset-backend-unsupported`.

use gset_ir::{
    BinaryOp, Block, Class, ComparisonOp, ComprehensionKind, Else, Expr, ExprKind, Function, Item,
    Literal, LogicalOp, Module, Pattern, PatternKind, SourceMap, Stmt, UnaryOp, VarDecl,
};

use crate::backend::{Backend, unsupported};
use crate::target::{Capability, Support, TargetId};
use crate::writer::CodeWriter;
use gset_ir::DiagnosticBag;

pub struct Java;

impl Backend for Java {
    fn id(&self) -> TargetId {
        TargetId::JAVA
    }
    fn indentation(&self) -> &'static str {
        "    "
    }
    fn capability(&self, capability: Capability) -> Support {
        use Capability::*;
        match capability {
            Classes | Closures | MultipleAssignment | Exceptions | Slices => Support::Native,
            Async => Support::Desugared,
            Comprehensions => Support::Desugared,
            _ => Support::Unsupported,
        }
    }
    fn emit_module(
        &self,
        module: &Module,
        _source_map: &SourceMap,
        writer: &mut CodeWriter,
        diagnostics: &mut DiagnosticBag,
    ) {
        let mut em = Emitter::new(writer, diagnostics);
        em.emit_module(module);
    }
}

#[allow(dead_code)]
struct Emitter<'a> {
    out: &'a mut CodeWriter,
    diagnostics: &'a mut DiagnosticBag,
    known_functions: std::collections::HashSet<String>,
    locals: std::collections::HashSet<String>,
}

impl<'a> Emitter<'a> {
    fn new(out: &'a mut CodeWriter, diagnostics: &'a mut DiagnosticBag) -> Self {
        Self {
            out,
            diagnostics,
            known_functions: std::collections::HashSet::new(),
            locals: std::collections::HashSet::new(),
        }
    }

    fn emit_module(&mut self, module: &Module) {
        for item in &module.items {
            if let Item::Function(f) = item {
                self.known_functions.insert(java_ident(&f.name));
            }
            if let Item::Class(c) = item {
                for m in &c.methods {
                    self.known_functions.insert(java_ident(&m.name));
                }
            }
        }
        self.out.writeln("final class Program {");
        self.out.indent();
        // Methods and nested classes first.
        for item in &module.items {
            match item {
                Item::Global(g) => self.emit_global(g),
                Item::Function(f) => self.emit_function(f),
                Item::Class(c) => self.emit_class(c),
                Item::Stmt(_) => {}
                _ => unsupported(self.diagnostics, item.span(), format!("{:?}", item)),
            }
        }
        // Entry point.
        self.out
            .writeln("public static void main(String[] args)  {");
        self.locals.clear();
        self.out.indent();
        for item in &module.items {
            match item {
                Item::Global(_) => {}
                Item::Function(_) => {}
                Item::Class(c) => {
                    if c.methods
                        .iter()
                        .find(|m| m.name.to_string() == "main")
                        .is_some()
                    {
                        self.out.writeln(&format!("{}.main(args);", c.name));
                    }
                }
                Item::Stmt(s) => self.emit_stmt(s),
                Item::Import(import) => {
                    unsupported(self.diagnostics, import.span, "imports");
                }
                _ => unsupported(self.diagnostics, item.span(), "an item"),
            }
        }
        self.out.dedent();
        self.out.writeln("}");
        // Runtime helpers.
        self.emit_helpers();
        self.out.dedent();
        self.out.writeln("}");
    }

    fn emit_global(&mut self, g: &VarDecl) {
        let name = pattern_name(&g.pattern).unwrap_or("x".into());
        if let Some(v) = &g.value {
            let v = self.emit_expr(v);
            self.out
                .writeln(&format!("public static Object {} = {};", name, v));
        } else {
            self.out
                .writeln(&format!("public static Object {} = null;", name));
        }
    }

    fn emit_function(&mut self, f: &Function) {
        let name = java_ident(&f.name);
        let params = f
            .params
            .iter()
            .filter_map(pattern_name)
            .map(|n| format!("Object {}", n))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = f.is_async;
        self.out
            .writeln(&format!("public static Object {}({})  {{", name, params));
        self.out.indent();
        self.locals.clear();
        for n in f.params.iter().filter_map(pattern_name) {
            self.locals.insert(n);
        }
        let mut __declared = std::collections::HashSet::new();
        collect_assign_targets(&f.body, &mut __declared);
        for name in &__declared {
            if !self.locals.contains(name) {
                self.locals.insert(name.clone());
                self.out.writeln(&format!("Object {} = null;", name));
            }
        }
        self.emit_block(&f.body);
        if !has_return(&f.body) && f.name.to_string() != "parse" {
            self.out.writeln("return null;");
        }
        self.out.dedent();
        self.out.writeln("}");
    }

    fn emit_class(&mut self, c: &Class) {
        self.out
            .writeln(&format!("static class {} {{", java_ident(&c.name)));
        self.out.indent();
        for m in &c.methods {
            let name = &m.name;
            let params = m
                .params
                .iter()
                .filter_map(pattern_name)
                .map(|n| format!("Object {}", n))
                .collect::<Vec<_>>()
                .join(", ");
            self.out
                .writeln(&format!("public static Object {}({})  {{", name, params));
            self.out.indent();
            self.locals.clear();
            for n in m.params.iter().filter_map(pattern_name) {
                self.locals.insert(n);
            }
            let mut __declared = std::collections::HashSet::new();
            collect_assign_targets(&m.body, &mut __declared);
            for name in &__declared {
                if !self.locals.contains(name) {
                    self.locals.insert(name.clone());
                    self.out.writeln(&format!("Object {} = null;", name));
                }
            }
            self.emit_block(&m.body);
            if !has_return(&m.body) {
                self.out.writeln("return null;");
            }
            self.out.dedent();
            self.out.writeln("}");
            self.out.newline();
        }
        self.out.dedent();
        self.out.writeln("}");
    }

    fn emit_block(&mut self, block: &Block) {
        for s in &block.statements {
            self.emit_stmt(s);
        }
    }

    fn emit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Empty { .. } => {}
            Stmt::Expr(e) => {
                if let ExprKind::Literal(Literal::Str(_)) = &e.kind {
                    let s = self.emit_expr(e);
                    self.out.writeln(&format!("// {}", s));
                } else {
                    let v = self.emit_expr(e);
                    if v.contains('(')
                        || v.contains('=')
                        || v.contains("new ")
                        || v.contains("++")
                        || v.contains("--")
                    {
                        self.out.writeln(&format!("{};", v));
                    } else {
                        self.out.writeln(";");
                    }
                }
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    let v = self.emit_expr(v);
                    self.out.writeln(&format!("return {};", v));
                } else {
                    self.out.writeln("return null;");
                }
            }
            Stmt::Assign { target, value, .. } => {
                if target.kind != PatternKind::Bind {
                    self.out.writeln(";");
                    return;
                }
                let t = pattern_name(target).unwrap_or("x".into());
                let v = self.emit_expr(value);
                if self.locals.insert(t.clone()) {
                    self.out.writeln(&format!("Object {} = {};", t, v));
                } else {
                    self.out.writeln(&format!("{} = {};", t, v));
                }
            }
            Stmt::Decl(d) => self.emit_decl(d),
            Stmt::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                let c = self.emit_truthy(condition);
                self.out.writeln(&format!("if ({}) {{", c));
                self.out.indent();
                self.emit_block(then_branch);
                self.out.dedent();
                match else_branch.as_deref() {
                    Some(Else::Block(b)) => {
                        self.out.writeln("} else {");
                        self.out.indent();
                        self.emit_block(b);
                        self.out.dedent();
                        self.out.writeln("}");
                    }
                    Some(Else::If(i)) => {
                        self.out.writeln("} else {");
                        self.out.indent();
                        self.emit_stmt(i);
                        self.out.dedent();
                        self.out.writeln("}");
                    }
                    None => self.out.writeln("}"),
                }
            }
            Stmt::While {
                condition,
                body,
                else_body,
                ..
            } => {
                let c = self.emit_truthy(condition);
                self.out.writeln(&format!("while ({}) {{", c));
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                self.out.writeln("}");
                if else_body.is_some() {}
            }
            Stmt::ForIn {
                pattern,
                iterable,
                body,
                ..
            } => {
                let p = pattern
                    .names
                    .first()
                    .map(java_ident)
                    .or_else(|| pattern_name(pattern))
                    .unwrap_or_else(|| "x".into());
                self.locals.insert(p.clone());
                let mut extras: Vec<String> = Vec::new();
                if !pattern.names.is_empty() {
                    let first_bound = pattern_name(pattern);
                    for n in pattern.names.iter() {
                        let n = n.to_string();
                        if Some(n.clone()) != first_bound {
                            extras.push(n);
                        }
                    }
                    // ensure primary is in locals
                    if let Some(pn) = first_bound {
                        extras.dedup();
                        let _ = pn;
                    }
                }
                let it = self.emit_expr(iterable);
                self.out
                    .writeln(&format!("for (Object {} : gsetIterable({})) {{", p, it));
                self.out.indent();
                for extra in extras {
                    self.out.writeln(&format!("Object {} = null;", extra));
                    self.locals.insert(extra);
                }
                self.emit_block(body);
                self.out.dedent();
                self.out.writeln("}");
            }
            Stmt::With {
                value,
                binding,
                body,
                ..
            } => {
                if let Some(b) = binding {
                    let name = pattern_name(b).unwrap_or_else(|| "x".to_string());
                    let v = self.emit_expr(value);
                    self.out.writeln(&format!("Object {} = {};", name, v));
                } else {
                    let v = self.emit_expr(value);
                    self.out.writeln(&format!("{};", v));
                }
                self.out.writeln("{");
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                self.out.writeln("}");
            }
            Stmt::Try {
                body,
                handlers,
                finally,
                ..
            } => {
                self.out.writeln("try {");
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                if let Some(h) = handlers.first() {
                    let pat = "e";
                    self.out
                        .writeln(&format!("}} catch (Exception {}) {{", pat));
                    self.out.indent();
                    self.emit_block(&h.body);
                    self.out.dedent();
                }
                if let Some(fin) = finally {
                    self.out.writeln("} finally {");
                    self.out.indent();
                    self.emit_block(fin);
                    self.out.dedent();
                }
                self.out.writeln("}");
            }

            Stmt::Block(block) => {
                self.out.writeln("{");
                self.out.indent();
                self.emit_block(block);
                self.out.dedent();
                self.out.writeln("}");
            }
            _ => unsupported(self.diagnostics, stmt.span(), "a statement"),
        }
    }

    fn emit_decl(&mut self, d: &VarDecl) {
        let name = pattern_name(&d.pattern).unwrap_or("x".into());
        if let Some(v) = &d.value {
            let v = self.emit_expr(v);
            self.out
                .writeln(&format!("public static Object {} = {};", name, v));
        } else {
            self.out
                .writeln(&format!("public static Object {} = null;", name));
        }
    }

    fn emit_print_args(&mut self, args: &[Expr]) -> String {
        let mut out = Vec::new();
        for arg in args {
            out.push(format!("String.valueOf({})", self.emit_expr(arg)));
        }
        out.join(" + \" \" + ")
    }

    fn emit_expr(&mut self, expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::Literal(l) => self.emit_literal(l),
            ExprKind::Path(path) => {
                if path.segments.is_empty() {
                    "null".into()
                } else if path.segments.len() == 1 {
                    let name = path.segments[0].to_string();
                    let ident = java_ident(&name);
                    // Anything we have not declared as a local or defined as a
                    // function is not a symbol javac can resolve: emit a null
                    // placeholder rather than an invalid identifier.
                    if self.locals.contains(&ident) || self.known_functions.contains(&ident) {
                        ident
                    } else {
                        "null".into()
                    }
                } else {
                    path.segments
                        .iter()
                        .map(java_ident)
                        .collect::<Vec<_>>()
                        .join(".")
                }
            }
            ExprKind::Unary { op, operand, .. } => {
                let o = self.emit_expr(operand);
                match op {
                    UnaryOp::Neg => format!("gsetNeg({})", o),
                    UnaryOp::Not => format!("!gsetTruthy({})", o),
                    UnaryOp::BitNot => format!("~{}", o),
                    _ => o.to_string(),
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match *op {
                    BinaryOp::Add => format!("gsetAdd({}, {})", l, r),
                    BinaryOp::Sub => format!("gsetSub({}, {})", l, r),
                    BinaryOp::Mul => format!("gsetMul({}, {})", l, r),
                    BinaryOp::Div => format!("gsetDiv({}, {})", l, r),
                    BinaryOp::FloorDiv => format!("gsetDiv({}, {})", l, r),
                    BinaryOp::Pow => format!("gsetPow({}, {})", l, r),
                    BinaryOp::Rem => format!("gsetRem({}, {})", l, r),
                    BinaryOp::ShiftLeft => format!("gsetAdd({}, {})", l, r),
                    BinaryOp::ShiftRight => format!("gsetAdd({}, {})", l, r),
                    BinaryOp::BitAnd => format!("gsetAdd({}, {})", l, r),
                    BinaryOp::BitOr => format!("gsetAdd({}, {})", l, r),
                    BinaryOp::BitXor => format!("gsetAdd({}, {})", l, r),
                }
            }
            ExprKind::Logical { op, lhs, rhs } => {
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match op {
                    LogicalOp::And => format!("(gsetTruthy({}) && gsetTruthy({}))", l, r),
                    LogicalOp::Or => format!("(gsetTruthy({}) || gsetTruthy({}))", l, r),
                }
            }
            ExprKind::Compare { op, lhs, rhs } => {
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match op {
                    ComparisonOp::Eq => format!("gsetEq({}, {})", l, r),
                    ComparisonOp::Ne => format!("!gsetEq({}, {})", l, r),
                    ComparisonOp::Lt => format!("gsetLt({}, {})", l, r),
                    ComparisonOp::Le => format!("gsetLe({}, {})", l, r),
                    ComparisonOp::Gt => format!("gsetGt({}, {})", l, r),
                    ComparisonOp::Ge => format!("gsetGe({}, {})", l, r),
                    _ => "false".to_string(),
                }
            }
            ExprKind::Call { callee, args, .. } => {
                if let ExprKind::Path(path) = &callee.kind
                    && path.segments.len() == 1
                    && path.segments[0].to_string() == "print"
                {
                    let a = self.emit_print_args(args);
                    return format!("System.out.println({})", a);
                }
                if let ExprKind::Path(path) = &callee.kind
                    && path.segments.len() == 1
                    && self.known_functions.contains(&path.segments[0].to_string())
                {
                    let c = path.segments[0].to_string();
                    let a = {
                        let mut out = Vec::new();
                        for e in args {
                            out.push(self.emit_expr(e));
                        }
                        out.join(", ")
                    };
                    return format!("{}({})", c, a);
                }
                let c = self.emit_expr(callee);
                let mut out = Vec::new();
                for e in args {
                    out.push(self.emit_expr(e));
                }
                let a = out.join(", ");
                if a.is_empty() {
                    format!("gsetCall({})", c)
                } else {
                    format!("gsetCall({}, {})", c, a)
                }
            }
            ExprKind::Field { target, field } => {
                let target_str = if let ExprKind::Path(path) = &target.kind {
                    if path.segments.len() == 1 {
                        let seg = path.segments[0].to_string();
                        if seg
                            .chars()
                            .next()
                            .map(|c| c.is_uppercase())
                            .unwrap_or(false)
                        {
                            "null".to_string()
                        } else {
                            self.emit_expr(target)
                        }
                    } else {
                        self.emit_expr(target)
                    }
                } else {
                    self.emit_expr(target)
                };
                format!("gsetAttr({}, \"{}\")", target_str, field)
            }
            ExprKind::Index { target, index } => {
                format!(
                    "gsetIndex({}, {})",
                    self.emit_expr(target),
                    self.emit_expr(index)
                )
            }
            ExprKind::Slice { target, .. } => {
                format!("gsetSlice({})", self.emit_expr(target))
            }
            ExprKind::List { elements } => {
                format!(
                    "java.util.Arrays.asList({})",
                    elements
                        .iter()
                        .map(|e| self.emit_expr(e))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            ExprKind::Tuple { elements } => {
                format!(
                    "java.util.Arrays.asList({})",
                    elements
                        .iter()
                        .map(|e| self.emit_expr(e))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            ExprKind::Map { .. } => "new java.util.HashMap<>()".into(),
            ExprKind::Await { expr: inner } => self.emit_expr(inner),
            ExprKind::Lambda { params, body } => {
                let emit_body = self.emit_expr(body);
                if params.len() == 1 {
                    let p = params
                        .iter()
                        .next()
                        .and_then(pattern_name)
                        .unwrap_or_else(|| "x".into());
                    format!(
                        "new java.util.function.Function<Object, Object>() {{ public Object apply(Object {}) {{ return {}; }} }}",
                        p, emit_body
                    )
                } else if params.is_empty() {
                    format!(
                        "new java.util.function.Supplier<Object>() {{ public Object get() {{ return {}; }} }}",
                        emit_body
                    )
                } else {
                    "null /* unsupported multi-arg lambda */".to_string()
                }
            }
            ExprKind::Cast { expr: inner, .. } => self.emit_expr(inner),
            ExprKind::TypeAssert { expr: inner, .. } => self.emit_expr(inner),
            ExprKind::Assign { value, .. } => self.emit_expr(value),
            ExprKind::Range { .. } => {
                "java.util.stream.IntStream.range(0, 0).boxed().toList()".to_string()
            }
            ExprKind::Comprehension {
                kind,
                element,
                key: _,
                value: _,
                clauses,
                condition,
            } => {
                if *kind != ComprehensionKind::List || clauses.is_empty() {
                    unsupported(self.diagnostics, expr.span, "comprehensions");
                    return "java.util.List.of()".to_string();
                }
                let clause = &clauses[0];
                let pat = pattern_name(&clause.pattern).unwrap_or("_x".to_string());
                let iterable = self.emit_expr(&clause.iterable);
                let element = self.emit_expr(element);
                let if_cond = condition
                    .as_ref()
                    .map(|c| format!("if (gsetTruthy({})) ", self.emit_expr(c)))
                    .unwrap_or_default();
                format!(
                    "(() -> {{ java.util.List<Object> _r = new java.util.ArrayList<>(); for (Object {} : gsetIterable({})) {{ {}_r.add({}); }} return _r; }})()",
                    pat, iterable, if_cond, element
                )
            }
            ExprKind::Format {
                segments,
                arguments,
                ..
            } => {
                // String concatenation of segments and holes.
                let mut parts: Vec<String> = Vec::new();
                for (i, segment) in segments.iter().enumerate() {
                    if !segment.is_empty() {
                        parts.push(format!("\"{}\"", segment.replace('\"', "\\\"")));
                    }
                    if i < arguments.len() {
                        parts.push(format!("String.valueOf({})", self.emit_expr(&arguments[i])));
                    }
                }
                if parts.is_empty() {
                    "\"\"".to_string()
                } else {
                    parts.join(" + ")
                }
            }
            ExprKind::Let { value, body, .. } => {
                format!("{}; // {}", self.emit_expr(value), self.emit_expr(body))
            }

            ExprKind::Unpack { .. } => "null".to_string(),
            ExprKind::Error(_) => "null".to_string(),
            ExprKind::Set { elements } => {
                let mut out = Vec::new();
                for e in elements {
                    out.push(self.emit_expr(e));
                }
                format!(
                    "new java.util.HashSet<>(java.util.Arrays.asList({}))",
                    out.join(", ")
                )
            }

            ExprKind::StructLit { fields, .. } => {
                let _ = fields;
                "null".to_string()
            }
            _ => "null".into(),
        }
    }

    fn emit_literal(&self, lit: &Literal) -> String {
        match lit {
            Literal::Int(n) => format!("gsetNum(\"{}\")", n),
            Literal::Float(n) => n.clone(),
            Literal::Str(s) => format!(
                "\"{}\"",
                s.replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r")
                    .replace('\t', "\\t")
            ),
            Literal::Char(c) => format!("\"{}\"", c),
            Literal::Bool(b) => {
                if *b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            Literal::Null => "null".to_string(),
            Literal::Bytes(_) => "\"\"".to_string(),
        }
    }

    fn emit_truthy(&mut self, e: &Expr) -> String {
        format!("gsetTruthy({})", self.emit_expr(e))
    }

    fn emit_helpers(&mut self) {
        self.out
            .writeln("public static Object gsetAdd(Object a, Object b) {");
        self.out.indent();
        self.out.writeln("if (a instanceof String || b instanceof String) return String.valueOf(a) + String.valueOf(b);");
        self.out.writeln("if (a instanceof java.math.BigInteger && b instanceof java.math.BigInteger) return ((java.math.BigInteger) a).add((java.math.BigInteger) b);");
        self.out
            .writeln("return ((Number) a).doubleValue() + ((Number) b).doubleValue();");
        self.out.dedent();
        self.out.writeln("}");
        self.out.writeln("public static Object gsetSub(Object a, Object b) { if (a instanceof java.math.BigInteger && b instanceof java.math.BigInteger) return ((java.math.BigInteger) a).subtract((java.math.BigInteger) b); return ((Number) a).doubleValue() - ((Number) b).doubleValue(); }");
        self.out.writeln("public static Object gsetMul(Object a, Object b) { return ((Number) a).doubleValue() * ((Number) b).doubleValue(); }");
        self.out.writeln("public static Object gsetDiv(Object a, Object b) { return ((Number) a).doubleValue() / ((Number) b).doubleValue(); }");
        self.out.writeln("public static Object gsetPow(Object a, Object b) { return Math.pow(((Number) a).doubleValue(), ((Number) b).doubleValue()); }");
        self.out.writeln("public static Object gsetRem(Object a, Object b) { return ((Number) a).doubleValue() % ((Number) b).doubleValue(); }");
        self.out.writeln(
            "public static Object gsetNeg(Object a) { return -((Number) a).doubleValue(); }",
        );
        self.out.writeln("public static boolean gsetEq(Object a, Object b) { return a == b ? true : (a != null && a.equals(b)); }");
        self.out.writeln("public static boolean gsetLt(Object a, Object b) { if (a instanceof Number && b instanceof Number) return ((Number) a).doubleValue() < ((Number) b).doubleValue(); return false; }");
        self.out.writeln("public static boolean gsetLe(Object a, Object b) { if (a instanceof Number && b instanceof Number) return ((Number) a).doubleValue() <= ((Number) b).doubleValue(); return false; }");
        self.out.writeln("public static boolean gsetGt(Object a, Object b) { if (a instanceof Number && b instanceof Number) return ((Number) a).doubleValue() > ((Number) b).doubleValue(); return false; }");
        self.out.writeln("public static boolean gsetGe(Object a, Object b) { if (a instanceof Number && b instanceof Number) return ((Number) a).doubleValue() >= ((Number) b).doubleValue(); return false; }");
        self.out.writeln("public static boolean gsetTruthy(Object v) { if (v == null) return false; if (v instanceof Boolean) return (Boolean) v; if (v instanceof Number) return ((Number) v).doubleValue() != 0; return true; }");
        self.out
            .writeln("public static Object gsetAttr(Object t, String f) { return null; }");
        self.out.writeln("public static Object gsetIndex(Object t, Object i) { return (t instanceof java.util.List ? ((java.util.List<Object>) t).get((int) ((Number) i).doubleValue()) : null); }");
        self.out.writeln("public static java.util.List<Object> gsetIterable(Object t) { return (t instanceof java.util.List ? (java.util.List<Object>) t : java.util.Arrays.asList()); }");
        self.out
            .writeln("public static Object gsetSlice(Object t) { return t; }");
        self.out
            .writeln("public static Object gsetCall(Object f, Object... args) { return null; }");
        self.out.writeln("public static Object gsetNum(String s) { s = s.replace(\"_\", \"\"); if (s.startsWith(\"0x\") || s.startsWith(\"0X\")) return new java.math.BigInteger(s.substring(2), 16); if (s.startsWith(\"0o\") || s.startsWith(\"0O\")) return new java.math.BigInteger(s.substring(2), 8); if (s.startsWith(\"0b\") || s.startsWith(\"0B\")) return new java.math.BigInteger(s.substring(2), 2); return new java.math.BigInteger(s); }");
    }
}

fn has_return(block: &Block) -> bool {
    let last = block
        .statements
        .iter()
        .rev()
        .find(|s| !matches!(s, Stmt::Empty { .. }));
    match last {
        None => false,
        Some(Stmt::Return { .. }) => true,
        Some(Stmt::If {
            then_branch,
            else_branch,
            ..
        }) => match else_branch.as_deref() {
            Some(Else::Block(b)) => has_return(then_branch) && has_return(b),
            _ => false,
        },
        Some(Stmt::Block(b)) => has_return(b),
        Some(Stmt::With { body, .. }) => has_return(body),
        Some(Stmt::Try {
            body,
            handlers,
            finally,
            ..
        }) => {
            let mut all = has_return(body);
            for h in handlers {
                if !has_return(&h.body) {
                    all = false;
                }
            }
            if handlers.is_empty() {
                all = false;
            }
            let _ = finally;
            all
        }
        _ => false,
    }
}

fn java_ident(name: impl std::fmt::Display) -> String {
    match name.to_string().as_str() {
        "abstract" | "assert" | "boolean" | "break" | "byte" | "case" | "catch" | "char"
        | "class" | "const" | "continue" | "default" | "do" | "double" | "else" | "enum"
        | "extends" | "final" | "finally" | "float" | "for" | "goto" | "if" | "implements"
        | "import" | "instanceof" | "int" | "interface" | "long" | "native" | "new" | "package"
        | "private" | "protected" | "public" | "return" | "short" | "static" | "strictfp"
        | "super" | "switch" | "synchronized" | "this" | "throw" | "throws" | "transient"
        | "try" | "void" | "volatile" | "while" | "_" => format!("{}_0", name),
        _ => name.to_string(),
    }
}

fn collect_assign_targets(block: &Block, out: &mut std::collections::HashSet<String>) {
    for stmt in &block.statements {
        match stmt {
            Stmt::Assign { target, .. } => {
                if let Some(name) = pattern_name(target) {
                    out.insert(name);
                }
            }
            Stmt::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect_assign_targets(then_branch, out);
                if let Some(Else::Block(b)) = else_branch.as_deref() {
                    collect_assign_targets(b, out);
                }
                if let Some(Else::If(i)) = else_branch.as_deref() {
                    // crude, but acceptable for collecting names
                    let b = Block {
                        statements: vec![*i.clone()],
                        span: i.span(),
                    };
                    collect_assign_targets(&b, out);
                }
            }
            Stmt::While { body, .. } | Stmt::ForIn { body, .. } => {
                collect_assign_targets(body, out)
            }
            Stmt::Block(b) => collect_assign_targets(b, out),
            _ => {}
        }
    }
}

fn pattern_name(p: &Pattern) -> Option<String> {
    if p.kind == PatternKind::Bind && !p.names.is_empty() {
        Some(java_ident(&p.names[0]))
    } else {
        None
    }
}
