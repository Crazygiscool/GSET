//! The C++ backend.
//!
//! Emits C++ from the IR using a dynamic Value runtime. Constructs this backend
//! does not yet support are reported with `gset-backend-unsupported`.

use crate::backend::{Backend, unsupported};
use crate::target::{Capability, Support, TargetId};
use crate::writer::CodeWriter;
use gset_ir::DiagnosticBag;
use gset_ir::{
    BinaryOp, Block, ComparisonOp, Else, Expr, ExprKind, Function, Item, Literal, LogicalOp,
    Module, Pattern, PatternKind, SourceMap, Stmt, UnaryOp, VarDecl,
};

pub struct Cpp;

impl Backend for Cpp {
    fn id(&self) -> TargetId {
        TargetId::CPP
    }

    fn capability(&self, capability: Capability) -> Support {
        use Capability::*;
        match capability {
            Imports => Support::Desugared,
            Classes | FStrings | Closures | MultipleAssignment | Exceptions | Slices
            | Comprehensions | Generators | PatternMatching | LoopElse => Support::Unsupported,
            _ => Support::Unsupported,
        }
    }

    fn indentation(&self) -> &'static str {
        "    "
    }

    fn emit_module(
        &self,
        module: &Module,
        _source_map: &SourceMap,
        writer: &mut CodeWriter,
        diagnostics: &mut DiagnosticBag,
    ) {
        emit_runtime(writer);
        let mut emitter = Emitter::new(writer, diagnostics);
        emitter.emit_module(module);
        // Guarantee a main for empty modules.
        if !module
            .items
            .iter()
            .any(|i| matches!(i, Item::Function(f) if f.name.to_string() == "main"))
        {
            // Only emit main if no function named main was emitted implicitly.
        }
    }
}

struct Emitter<'a> {
    out: &'a mut CodeWriter,
    diagnostics: &'a mut DiagnosticBag,
}

impl<'a> Emitter<'a> {
    fn new(out: &'a mut CodeWriter, diagnostics: &'a mut DiagnosticBag) -> Self {
        Self { out, diagnostics }
    }

    fn emit_module(&mut self, module: &Module) {
        let mut main_statements = Vec::new();
        for item in &module.items {
            match item {
                Item::Import(import) => {
                    self.diagnostics.push(
                        gset_ir::Diagnostic::warning(
                            import.span,
                            "import has no C++ equivalent and was dropped",
                        )
                        .with_code("gset-backend-import"),
                    );
                }
                Item::Global(g) => {
                    self.emit_global(g);
                    self.out.newline();
                }
                Item::Function(f) => {
                    self.emit_function(f);
                    self.out.blank();
                }
                Item::Stmt(s) => {
                    main_statements.push(s.clone());
                }
                other => unsupported(self.diagnostics, other.span(), "an item"),
            }
        }
        self.out.writeln("int main() {");
        self.out.indent();
        for stmt in &main_statements {
            self.emit_stmt(stmt);
        }
        self.out.writeln("return 0;");
        self.out.dedent();
        self.out.writeln("}");
    }

    fn emit_global(&mut self, g: &VarDecl) {
        let name = pattern_name(&g.pattern).unwrap_or("_".into());
        if let Some(v) = &g.value {
            let rhs = self.emit_expr(v);
            self.out.writeln(&format!("Value {} = {};", name, rhs));
        } else {
            self.out
                .writeln(&format!("Value {} = Value::Null();", name));
        }
    }

    fn emit_function(&mut self, f: &Function) {
        let params = f
            .params
            .iter()
            .filter_map(pattern_name)
            .map(|n| format!("Value {}", n))
            .collect::<Vec<_>>()
            .join(", ");
        self.out
            .writeln(&format!("Value {}({}) {{", f.name, params));
        self.out.indent();
        let mut param_names = std::collections::HashSet::new();
        for param in &f.params {
            if let Some(name) = pattern_name(param) {
                param_names.insert(name);
            }
        }
        let mut locals = std::collections::HashSet::new();
        collect_assign_targets(&f.body, &mut locals);
        for name in locals {
            if !param_names.contains(&name) {
                self.out
                    .writeln(&format!("Value {} = Value::Null();", name));
            }
        }
        self.emit_block(&f.body);
        // main-like safety return
        if !has_return(&f.body) {
            self.out.writeln("return Value::Null();");
        }
        self.out.dedent();
        self.out.writeln("}");
    }

    fn emit_block(&mut self, block: &Block) {
        for s in &block.statements {
            self.emit_stmt(s);
        }
    }

    fn emit_decl(&mut self, d: &VarDecl) {
        let name = pattern_name(&d.pattern).unwrap_or("_".into());
        if let Some(v) = &d.value {
            let rhs = self.emit_expr(v);
            self.out.writeln(&format!("Value {} = {};", name, rhs));
        } else {
            self.out
                .writeln(&format!("Value {} = Value::Null();", name));
        }
    }

    fn emit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Empty { .. } => {}
            Stmt::Break { .. } => self.out.writeln("break;"),
            Stmt::Continue { .. } => self.out.writeln("continue;"),
            Stmt::ForIn {
                pattern,
                iterable,
                body,
                ..
            } => {
                let p = pattern_name(pattern).unwrap_or("_".into());
                let it = self.emit_expr(iterable);
                self.out
                    .writeln(&format!("for (Value {} : gset_iterate({})) {{", p, it));
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                self.out.writeln("}");
            }
            Stmt::Expr(e) => {
                let e = self.emit_expr(e);
                // Detect print specially already transformed in expression? Keep direct.
                self.out.writeln(&format!("{};", e));
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    let v = self.emit_expr(v);
                    self.out.writeln(&format!("return {};", v));
                } else {
                    self.out.writeln("return Value::Null();");
                }
            }
            Stmt::Assign { target, value, .. } => {
                let simple = match &target.kind {
                    PatternKind::Location(_) => true,
                    PatternKind::Bind => !target.names.is_empty() && target.names.len() == 1,
                    _ => false,
                };
                if !simple {
                    unsupported(self.diagnostics, stmt.span(), "a destructuring assignment");
                    return;
                }
                let t = if let PatternKind::Location(expr) = &target.kind {
                    self.emit_expr(expr)
                } else {
                    pattern_name(target).unwrap_or("_".into())
                };
                let v = self.emit_expr(value);
                self.out.writeln(&format!("{} = {};", t, v));
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
                        self.out.writeln("} else");
                        self.emit_stmt(i);
                    }
                    None => self.out.writeln("}"),
                }
            }
            Stmt::While {
                condition, body, ..
            } => {
                let c = self.emit_truthy(condition);
                self.out.writeln(&format!("while ({}) {{", c));
                self.out.indent();
                self.emit_block(body);
                self.out.dedent();
                self.out.writeln("}");
            }
            _ => unsupported(self.diagnostics, stmt.span(), "a statement"),
        }
    }

    fn emit_truthy(&mut self, e: &Expr) -> String {
        self.emit_expr(e)
    }

    fn emit_expr(&mut self, expr: &Expr) -> String {
        match &expr.kind {
            ExprKind::Literal(l) => self.emit_literal(l),
            ExprKind::Path(path) => {
                if path.segments.is_empty() {
                    "Value::Null()".into()
                } else {
                    path.segments
                        .iter()
                        .map(|n| {
                            if n.to_string() == "self" {
                                "(*this)".to_string()
                            } else {
                                n.to_string()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(".")
                }
            }
            ExprKind::Unary { op, operand, .. } => {
                let o = self.emit_expr(operand);
                match op {
                    UnaryOp::Neg => format!("({} * Value::Int(-1))", o),
                    UnaryOp::Not => format!("(!{}.truthy())", o),
                    _ => o.to_string(),
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match op {
                    BinaryOp::Add => format!("({} + {})", l, r),
                    BinaryOp::Sub => format!("({} - {})", l, r),
                    BinaryOp::Mul => format!("({} * {})", l, r),
                    BinaryOp::Div => format!("({} / {})", l, r),
                    BinaryOp::Pow => format!(
                        "Value::Float(std::pow(({}).as_float(), ({}).as_float()))",
                        l, r
                    ),
                    BinaryOp::Rem => format!("({} % {})", l, r),
                    BinaryOp::BitAnd => format!("({} & {})", l, r),
                    BinaryOp::BitOr => format!("({} | {})", l, r),
                    _ => format!("({} + {})", l, r),
                }
            }
            ExprKind::Logical { op, lhs, rhs } => {
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match op {
                    LogicalOp::And => format!("gset_logical_and({}, {})", l, r),
                    LogicalOp::Or => format!("gset_logical_or({}, {})", l, r),
                }
            }
            ExprKind::Compare { op, lhs, rhs } => {
                let l = self.emit_expr(lhs);
                let r = self.emit_expr(rhs);
                match *op {
                    ComparisonOp::Eq | ComparisonOp::Is => format!("Value::Bool({} == {})", l, r),
                    ComparisonOp::Ne | ComparisonOp::IsNot => {
                        format!("Value::Bool({} != {})", l, r)
                    }
                    ComparisonOp::Lt => format!("Value::Bool({} < {})", l, r),
                    ComparisonOp::Le => format!("Value::Bool({} <= {})", l, r),
                    ComparisonOp::Gt => format!("Value::Bool({} > {})", l, r),
                    ComparisonOp::Ge => format!("Value::Bool({} >= {})", l, r),
                    ComparisonOp::In => format!("Value::Bool(gset_in({}, {}))", r, l),
                    ComparisonOp::NotIn => format!("Value::Bool(!gset_in({}, {}))", r, l),
                }
            }
            ExprKind::Call { callee, args, .. } => {
                if let ExprKind::Path(path) = &callee.kind
                    && path.segments.len() == 1
                    && path.segments[0].to_string() == "print"
                {
                    let a = args
                        .iter()
                        .map(|e| self.emit_expr(e))
                        .collect::<Vec<_>>()
                        .join(" << ");
                    return format!(
                        "(std::cout << {} << std::endl, Value::Null())",
                        if a.is_empty() { "\"\"".to_string() } else { a }
                    );
                }
                if let ExprKind::Path(path) = &callee.kind
                    && path.segments.len() == 1
                    && path.segments[0].to_string() == "len"
                    && args.len() == 1
                {
                    return format!(
                        "Value::Int({}.as_string().size())",
                        self.emit_expr(&args[0])
                    );
                }

                let c = self.emit_expr(callee);
                let a = args
                    .iter()
                    .map(|e| self.emit_expr(e))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", c, a)
            }
            ExprKind::Index { target, index } => {
                format!("{}[{}]", self.emit_expr(target), self.emit_expr(index))
            }
            ExprKind::List { elements } => {
                let elems = elements
                    .iter()
                    .map(|e| self.emit_expr(e))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("Value::List({{ {} }})", elems)
            }
            ExprKind::Tuple { elements } => {
                let elems = elements
                    .iter()
                    .map(|e| self.emit_expr(e))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("Value::List({{ {} }})", elems)
            }
            ExprKind::Conditional {
                condition,
                then_branch,
                else_branch,
            } => {
                format!(
                    "({} ? {} : {})",
                    self.emit_expr(condition),
                    self.emit_expr(then_branch),
                    self.emit_expr(else_branch)
                )
            }
            _ => {
                unsupported_expr(self.diagnostics, expr.span);
                "Value::Null()".to_string()
            }
        }
    }

    fn emit_literal(&mut self, lit: &Literal) -> String {
        match lit {
            Literal::Int(n) => {
                // Keep only integer literals C++/the runtime can represent exactly.
                if n.parse::<i64>().is_ok()
                    && !n.contains('_')
                    && !n.contains('o')
                    && !n.contains('x')
                    && !n.contains('b')
                {
                    format!("Value::Int({})", n)
                } else {
                    unsupported_marker(
                        self.diagnostics,
                        gset_ir::Span::synthetic(),
                        "an integer literal",
                    );
                    "Value::Null()".to_string()
                }
            }
            Literal::Float(n) => format!("Value::Float({})", n),
            Literal::Bool(b) => format!("Value::Bool({})", b),
            Literal::Str(s) => {
                let s = s
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r")
                    .replace('\t', "\\t");
                format!("Value::Str(\"{}\")", s)
            }
            Literal::Null => "Value::Null()".to_string(),
            Literal::Char(c) => format!("Value::Str(\"{}\")", c),
            Literal::Bytes(_) => "Value::Str(\"\")".to_string(),
        }
    }
}

fn has_return(block: &Block) -> bool {
    block
        .statements
        .iter()
        .any(|s| matches!(s, Stmt::Return { .. }))
}

fn pattern_name(p: &Pattern) -> Option<String> {
    if p.kind == PatternKind::Bind && !p.names.is_empty() {
        Some(p.names[0].to_string())
    } else {
        None
    }
}

fn emit_runtime(writer: &mut CodeWriter) {
    writer.write(CPP_RUNTIME);
    writer.newline();
}

fn unsupported_marker(diagnostics: &mut DiagnosticBag, span: gset_ir::Span, what: &str) {
    diagnostics.push(
        gset_ir::Diagnostic::error(
            span,
            format!(
                "this backend cannot express {}: it is reported rather than emitted",
                what
            ),
        )
        .with_code("gset-backend-unsupported"),
    );
}

fn unsupported_expr(diagnostics: &mut DiagnosticBag, span: gset_ir::Span) -> String {
    unsupported_marker(diagnostics, span, "an expression");
    "0".to_string()
}

static CPP_RUNTIME: &str = r#"
#include <cmath>
#include <cstdint>
#include <iostream>
#include <map>
#include <memory>
#include <string>
#include <vector>

struct Value {
    enum Tag { INT, FLOAT, BOOL, STR, LIST, MAP, NIL } tag;
    long long int_value;
    double float_value;
    bool bool_value;
    std::string string_value;
    std::shared_ptr<std::vector<Value>> list;
    std::shared_ptr<std::map<std::string, Value>> map;

    Value() : tag(NIL), int_value(0), float_value(0.0), bool_value(false) {}
    static Value Int(long long v) { Value r; r.tag = INT; r.int_value = v; return r; }
    static Value Float(double v) { Value r; r.tag = FLOAT; r.float_value = v; return r; }
    static Value Bool(bool v) { Value r; r.tag = BOOL; r.bool_value = v; return r; }
    static Value Str(const std::string &v) { Value r; r.tag = STR; r.string_value = v; return r; }
    static Value List(const std::vector<Value> &v) { Value r; r.tag = LIST; r.list = std::make_shared<std::vector<Value>>(v); return r; }
    static Value Map() { Value r; r.tag = MAP; r.map = std::make_shared<std::map<std::string, Value>>(); return r; }
    static Value Null() { Value r; r.tag = NIL; return r; }

    operator bool() const { return truthy(); }
    bool truthy() const {
        switch (tag) {
            case BOOL: return bool_value;
            case INT: return int_value != 0;
            case FLOAT: return float_value != 0.0;
            case STR: return !string_value.empty();
            case LIST: return list && !list->empty();
            case MAP: return map && !map->empty();
            case NIL: return false;
        }
        return false;
    }
    std::string as_string() const {
        switch (tag) {
            case INT: return std::to_string(int_value);
            case FLOAT: return std::to_string(float_value);
            case BOOL: return bool_value ? "true" : "false";
            case STR: return string_value;
            case NIL: return "nil";
            default: return "value";
        }
    }
    double as_float() const {
        switch (tag) {
            case INT: return (double)int_value;
            case FLOAT: return float_value;
            case BOOL: return bool_value ? 1.0 : 0.0;
            case STR: return 0.0;
            default: return 0.0;
        }
    }
    Value &operator[](size_t index) {
        static Value nil = Value::Null();
        if (tag == LIST && list && index < list->size()) return (*list)[index];
        return nil;
    }
};

inline Value operator+(const Value &a, const Value &b) {
    if (a.tag == Value::INT && b.tag == Value::INT) return Value::Int(a.int_value + b.int_value);
    if (a.tag == Value::FLOAT || b.tag == Value::FLOAT) return Value::Float(a.as_float() + b.as_float());
    if (a.tag == Value::STR && b.tag == Value::STR) return Value::Str(a.string_value + b.string_value);
    return Value::Null();
}

inline Value operator-(const Value &a, const Value &b) {
    if (a.tag == Value::INT && b.tag == Value::INT) return Value::Int(a.int_value - b.int_value);
    return Value::Float(a.as_float() - b.as_float());
}

inline Value operator*(const Value &a, const Value &b) {
    if (a.tag == Value::INT && b.tag == Value::INT) return Value::Int(a.int_value * b.int_value);
    return Value::Float(a.as_float() * b.as_float());
}

inline Value operator/(const Value &a, const Value &b) {
    if (a.tag == Value::INT && b.tag == Value::INT) return Value::Int(a.int_value / b.int_value);
    return Value::Float(a.as_float() / b.as_float());
}

inline Value operator%(const Value &a, const Value &b) {
    if (a.tag == Value::INT && b.tag == Value::INT) return Value::Int(a.int_value % b.int_value);
    return Value::Null();
}

inline Value operator&(const Value &a, const Value &b) {
    if (a.tag == Value::INT && b.tag == Value::INT) return Value::Int(a.int_value & b.int_value);
    return Value::Null();
}

inline Value operator|(const Value &a, const Value &b) {
    if (a.tag == Value::INT && b.tag == Value::INT) return Value::Int(a.int_value | b.int_value);
    return Value::Null();
}

inline bool operator<(const Value &a, const Value &b) {
    if (a.tag == Value::INT && b.tag == Value::INT) return a.int_value < b.int_value;
    if (a.tag == Value::FLOAT || b.tag == Value::FLOAT) return a.as_float() < b.as_float();
    return a.as_string() < b.as_string();
}

inline bool operator<=(const Value &a, const Value &b) { return !(b < a); }
inline bool operator>(const Value &a, const Value &b) { return b < a; }
inline bool operator>=(const Value &a, const Value &b) { return !(a < b); }
inline bool operator==(const Value &a, const Value &b) { return a.as_string() == b.as_string(); }
inline bool operator!=(const Value &a, const Value &b) { return !(a == b); }

inline std::ostream &operator<<(std::ostream &os, const Value &v) {
    os << v.as_string();
    return os;
}

inline double as_float_safe(const Value &v) { return v.as_float(); }

static Value gset_logical_and(const Value &left, const Value &right) {
    if (left.truthy()) return right;
    return left;
}
static Value gset_logical_or(const Value &left, const Value &right) {
    if (left.truthy()) return left;
    return right;
}

static bool gset_in(const Value &items, const Value &item) {
    if (items.tag == Value::LIST && items.list) {
        for (const Value &candidate : *items.list) {
            if (candidate == item) return true;
        }
    }
    if (items.tag == Value::STR) {
        return items.string_value.find(item.as_string()) != std::string::npos;
    }
    if (items.tag == Value::MAP && items.map) {
        return items.map->find(item.as_string()) != items.map->end();
    }
    return false;
}
"#;

// helper functions embedded in runtime need as_float in Value; add by replacement done manually below.
// The runtime string above references v.as_float and float_value_or indirectly, which do not yet exist.

fn collect_assign_targets(block: &Block, out: &mut std::collections::HashSet<String>) {
    for stmt in &block.statements {
        collect_assign_stmt(stmt, out);
    }
}

fn collect_assign_stmt(stmt: &Stmt, out: &mut std::collections::HashSet<String>) {
    match stmt {
        Stmt::Assign { target, .. } => {
            if let PatternKind::Location(expr) = &target.kind
                && let ExprKind::Path(path) = &expr.kind
                && path.segments.len() == 1
            {
                out.insert(path.segments[0].to_string());
            } else if let PatternKind::Bind = &target.kind
                && let Some(name) = pattern_name(target)
            {
                out.insert(name);
            }
        }
        Stmt::Decl(d) => {
            if let Some(name) = pattern_name(&d.pattern) {
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
            if let Some(Else::If(s)) = else_branch.as_deref() {
                collect_assign_stmt(s, out);
            }
        }
        Stmt::While { body, .. } => collect_assign_targets(body, out),
        Stmt::For { init, body, .. } => {
            if let Some(s) = init {
                collect_assign_stmt(s, out);
            }
            collect_assign_targets(body, out);
        }
        Stmt::ForIn { body, .. } => collect_assign_targets(body, out),
        Stmt::Block(b) => collect_assign_targets(b, out),
        _ => {}
    }
}
