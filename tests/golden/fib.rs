
use std::collections::HashMap;

#[derive(Clone, Debug)]
enum Value {

    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    List(Vec<Value>),
    Map(HashMap<String, Value>),
    Null,
}

impl Value {
    pub fn abs(&self) -> Value {
        match self {
            Value::Int(v) => Value::Int(v.abs()),
            Value::Float(v) => Value::Float(v.abs()),
            _ => Value::Null
        }
    }
    pub fn len(&self) -> i64 {
        match self {
            Value::Str(v) => v.chars().count() as i64,
            Value::List(v) => v.len() as i64,
            Value::Map(v) => v.len() as i64,
            _ => 0
        }
    }
    pub fn to_uppercase(&self) -> Value {
        match self { Value::Str(v) => Value::Str(v.to_uppercase()), _ => self.clone() }
    }
    pub fn to_lowercase(&self) -> Value {
        match self { Value::Str(v) => Value::Str(v.to_lowercase()), _ => self.clone() }
    }
    pub fn trim(&self) -> Value {
        match self { Value::Str(v) => Value::Str(v.trim().to_string()), _ => self.clone() }
    }
    pub fn get(&self, key: &str) -> Value {
        match self { Value::Map(v) => v.get(key).cloned().unwrap_or(Value::Null), _ => Value::Null }
    }
    pub fn as_int(&self) -> i64 {
        match self {
            Value::Int(v) => *v,
            Value::Bool(v) => if *v { 1 } else { 0 },
            Value::Float(v) => *v as i64,
            Value::Str(v) => v.parse::<i64>().unwrap_or(0),
            _ => 0,
        }
    }
    pub fn gset_truthy(&self) -> bool {
        match self {
            Value::Bool(v) => *v,
            Value::Int(v) => *v != 0,
            Value::Float(v) => *v != 0.0,
            Value::Str(v) => !v.is_empty(),
            Value::List(v) => !v.is_empty(),
            Value::Map(v) => !v.is_empty(),
            Value::Null => false,
        }
    }
    pub fn gset_iterate(&self) -> Vec<Value> {
        match self {
            Value::List(v) => v.clone(),
            Value::Str(v) => v.chars().map(|c: char| Value::Str(c.to_string())).collect(),
            Value::Map(v) => v.values().cloned().collect(),
            _ => vec![self.clone()],
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{}", v),
            Value::Float(v) => write!(f, "{}", v),
            Value::Bool(v) => write!(f, "{}", v),
            Value::Str(v) => write!(f, "{}", v),
            Value::List(v) => write!(f, "{:?}", v),
            Value::Map(v) => write!(f, "{:?}", v),
            Value::Null => write!(f, "nil"),
        }
    }
}

impl std::ops::Add for Value {
    type Output = Value;
    fn add(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Value::Int(l), Value::Int(r)) => Value::Int(l + r),
            (Value::Float(l), Value::Float(r)) => Value::Float(l + r),
            (Value::Float(l), Value::Int(r)) => Value::Float(l + (r as f64)),
            (Value::Int(l), Value::Float(r)) => Value::Float((l as f64) + r),
            (Value::Str(l), Value::Str(r)) => Value::Str(format!("{}{}", l , r)),
            _ => Value::Null,
        }
    }
}

impl std::ops::Sub for Value {
    type Output = Value;
    fn sub(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Value::Int(l), Value::Int(r)) => Value::Int(l - r),
            (Value::Float(l), Value::Float(r)) => Value::Float(l - r),
            (Value::Float(l), Value::Int(r)) => Value::Float(l - (r as f64)),
            (Value::Int(l), Value::Float(r)) => Value::Float((l as f64) - r),
            _ => Value::Null,
        }
    }
}

impl std::ops::Mul for Value {
    type Output = Value;
    fn mul(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Value::Int(l), Value::Int(r)) => Value::Int(l * r),
            (Value::Float(l), Value::Float(r)) => Value::Float(l * r),
            (Value::Float(l), Value::Int(r)) => Value::Float(l * (r as f64)),
            (Value::Int(l), Value::Float(r)) => Value::Float((l as f64) * r),
            _ => Value::Null,
        }
    }
}

impl std::ops::Div for Value {
    type Output = Value;
    fn div(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Value::Int(l), Value::Int(r)) => Value::Int(l / r),
            (Value::Float(l), Value::Float(r)) => Value::Float(l / r),
            (Value::Float(l), Value::Int(r)) => Value::Float(l / (r as f64)),
            (Value::Int(l), Value::Float(r)) => Value::Float((l as f64) / r),
            _ => Value::Null,
        }
    }
}

impl std::ops::Rem for Value {
    type Output = Value;
    fn rem(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Value::Int(l), Value::Int(r)) => Value::Int(l % r),
            _ => Value::Null,
        }
    }
}

impl std::cmp::PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(l), Value::Int(r)) => l == r,
            (Value::Float(l), Value::Float(r)) => l == r,
            (Value::Str(l), Value::Str(r)) => l == r,
            (Value::Bool(l), Value::Bool(r)) => l == r,
            _ => false,
        }
    }
}

impl std::cmp::PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Value::Int(l), Value::Int(r)) => l.partial_cmp(r),
            (Value::Float(l), Value::Float(r)) => l.partial_cmp(r),
            _ => None,
        }
    }
}

impl std::cmp::PartialEq<i64> for Value {
    fn eq(&self, other: &i64) -> bool {
        match self {
            Value::Int(v) => *v == *other,
            Value::Float(v) => *v == *other as f64,
            _ => false,
        }
    }
}

impl std::cmp::PartialOrd<i64> for Value {
    fn partial_cmp(&self, other: &i64) -> Option<std::cmp::Ordering> {
        match self {
            Value::Int(v) => v.partial_cmp(other),
            Value::Float(v) => (*v as i64).partial_cmp(other),
            _ => None,
        }
    }
}

impl std::cmp::PartialEq<Value> for i64 {
    fn eq(&self, other: &Value) -> bool {
        match other {
            Value::Int(v) => *self == *v,
            Value::Float(v) => *self as f64 == *v,
            _ => false,
        }
    }
}

impl std::cmp::PartialOrd<Value> for i64 {
    fn partial_cmp(&self, other: &Value) -> Option<std::cmp::Ordering> {
        match other {
            Value::Int(v) => self.partial_cmp(v),
            Value::Float(v) => self.partial_cmp(&(*v as i64)),
            _ => None,
        }
    }
}

impl Default for Value {
    fn default() -> Self { Value::Int(0) }
}

fn gset_truthy(value: &Value) -> bool { value.gset_truthy() }
fn gset_iterate(value: &Value) -> Vec<Value> { value.gset_iterate() }

impl std::ops::Add<Value> for i64 {
    type Output = Value;
    fn add(self, rhs: Value) -> Self::Output {
        match rhs {
            Value::Int(r) => Value::Int(self + r),
            Value::Float(r) => Value::Float((self as f64) + r),
            _ => Value::Null,
        }
    }
}

impl std::ops::Sub<i64> for Value {
    type Output = Value;
    fn sub(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v - rhs),
            Value::Float(v) => Value::Float(v - (rhs as f64)),
            _ => Value::Null,
        }
    }
}

impl std::ops::Mul<i64> for Value {
    type Output = Value;
    fn mul(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v * rhs),
            Value::Float(v) => Value::Float(v * (rhs as f64)),
            _ => Value::Null,
        }
    }
}

impl std::ops::Add<i64> for Value {
    type Output = Value;
    fn add(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v + rhs),
            Value::Float(v) => Value::Float(v + rhs as f64),
            _ => Value::Null,
        }
    }
}

impl std::ops::Div<i64> for Value {
    type Output = Value;
    fn div(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v / rhs),
            Value::Float(v) => Value::Float(v / rhs as f64),
            _ => Value::Null,
        }
    }
}

impl std::ops::Rem<i64> for Value {
    type Output = Value;
    fn rem(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v % rhs),
            _ => Value::Null,
        }
    }
}

impl std::ops::BitOr<i64> for Value {
    type Output = Value;
    fn bitor(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v | rhs),
            _ => Value::Null,
        }
    }
}

impl std::ops::Shl<i64> for Value {
    type Output = Value;
    fn shl(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v << rhs),
            _ => Value::Null,
        }
    }
}

impl std::ops::Shr<i64> for Value {
    type Output = Value;
    fn shr(self, rhs: i64) -> Self {
        match self {
            Value::Int(v) => Value::Int(v >> rhs),
            _ => Value::Null,
        }
    }
}

fn fib(n: i64) -> i64 {
    if n < 2 {
        return n;
    }
    ;
    return fib(n - 1) + fib(n - 2);
}

fn main() {
    println!("{}", fib(10));
}
