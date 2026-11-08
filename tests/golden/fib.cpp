
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

Value fib(Value n) {
    if (Value::Bool(n < Value::Int(2))) {
        return n;
    }
    return (fib((n - Value::Int(1))) + fib((n - Value::Int(2))));
}

int main() {
    Value::Str("Fibonacci with annotations.\n\nThe milestone 1 gate: `gset transpile tests/corpus/py/fib.py --to go` must emit\nGo that passes `gofmt` and `go vet`. Annotated deliberately, because the IR does\nnot infer types yet, and guessing a dynamic parameter's type is exactly the\ndefect class the rewrite exists to remove.\n");
    (std::cout << fib(Value::Int(10)) << std::endl, Value::Null());
    return 0;
}
