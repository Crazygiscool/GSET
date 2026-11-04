
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
    pub fn as_float(&self) -> f64 {
        match self {
            Value::Float(v) => *v,
            Value::Int(v) => *v as f64,
            Value::Bool(v) => if *v { 1.0 } else { 0.0 },
            Value::Str(v) => v.parse::<f64>().unwrap_or(0.0),
            _ => 0.0,
        }
    }
    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(v) => *v,
            Value::Int(v) => *v != 0,
            Value::Float(v) => *v != 0.0,
            Value::Str(v) => !v.is_empty(),
            Value::List(v) => !v.is_empty(),
            Value::Map(v) => !v.is_empty(),
            _ => false,
        }
    }
    pub fn as_str(&self) -> String {
        match self {
            Value::Str(v) => v.clone(),
            Value::Int(v) => v.to_string(),
            Value::Float(v) => v.to_string(),
            Value::Bool(v) => v.to_string(),
            Value::Null => String::new(),
            _ => format!("{:?}", self),
        }
    }    pub fn get_index(&self, index: usize) -> Value {
        match self {
            Value::List(v) => v.get(index).cloned().unwrap_or(Value::Null),
            Value::Str(v) => v
                .chars()
                .nth(index)
                .map(|value| Value::Str(value.to_string()))
                .unwrap_or(Value::Null),
            _ => Value::Null,
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

impl Eq for Value {}

impl std::hash::Hash for Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Value::Int(v) => {
                0u8.hash(state);
                v.hash(state);
            }
            Value::Float(v) => {
                1u8.hash(state);
                v.to_bits().hash(state);
            }
            Value::Bool(v) => {
                2u8.hash(state);
                v.hash(state);
            }
            Value::Str(v) => {
                3u8.hash(state);
                v.hash(state);
            }
            Value::List(v) => {
                4u8.hash(state);
                v.hash(state);
            }
            Value::Map(v) => {
                5u8.hash(state);
                for (key, value) in v.iter() {
                    key.hash(state);
                    value.hash(state);
                }
            }
            Value::Null => {
                6u8.hash(state);
            }
        }
    }
}

impl IntoIterator for Value {
    type Item = Value;
    type IntoIter = std::vec::IntoIter<Value>;
    fn into_iter(self) -> Self::IntoIter {
        match self {
            Value::List(values) => values.into_iter(),
            Value::Map(values) => values
                .into_iter()
                .map(|(key, value)| Value::List(vec![Value::Str(key), value]))
                .collect::<Vec<_>>()
                .into_iter(),
            _ => Vec::new().into_iter(),
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

fn gset_value_cmp(left: Value, right: Value) -> std::cmp::Ordering {
    match (left, right) {
        (Value::Int(l), Value::Int(r)) => l.cmp(&r),
        (Value::Float(l), Value::Float(r)) => l.partial_cmp(&r).unwrap_or(std::cmp::Ordering::Equal),
        (Value::Float(l), Value::Int(r)) => l.partial_cmp(&(r as f64)).unwrap_or(std::cmp::Ordering::Equal),
        (Value::Int(l), Value::Float(r)) => (l as f64).partial_cmp(&r).unwrap_or(std::cmp::Ordering::Equal),
        (Value::Str(l), Value::Str(r)) => l.cmp(&r),
        (Value::Bool(l), Value::Bool(r)) => l.cmp(&r),
        _ => std::cmp::Ordering::Equal,
    }
}
fn gset_identity(value: Value) -> Value { value }
fn gset_truthy(value: &Value) -> bool { value.gset_truthy() }
fn gset_iterate(value: &Value) -> Vec<Value> { value.gset_iterate() }

fn gset_tuple_2(value: Value) -> (Value, Value) {
    match value {
        Value::List(values) if values.len() >= 2 => (values[0].clone(), values[1].clone()),
        _ => (Value::Null, Value::Null),
    }
}

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
