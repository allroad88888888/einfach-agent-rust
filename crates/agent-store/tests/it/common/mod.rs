//! Test value type implementing `AtomValue` for store behavior tests.
//! Mirrors the subset of `einfach-core::Value` actually used in the upstream tests.

use std::fmt;

use einfach_store::AtomValue;

/// Test value covering `Number`, `Text`, and `Boolean` variants
/// used in upstream behavior tests (store_twin, family_twin, depend_primitive).
///
/// NaN handling: the upstream tests do not compare NaN, so standard
/// floating-point PartialEq (NaN != NaN) is correct.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum TestValue {
    Number(f64),
    Text(String),
    Boolean(bool),
}

impl PartialEq for TestValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TestValue::Number(a), TestValue::Number(b)) => a == b,
            (TestValue::Text(a), TestValue::Text(b)) => a == b,
            (TestValue::Boolean(a), TestValue::Boolean(b)) => a == b,
            _ => false,
        }
    }
}

impl fmt::Display for TestValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TestValue::Number(n) => write!(f, "{}", n),
            TestValue::Text(s) => write!(f, "\"{}\"", s),
            TestValue::Boolean(b) => write!(f, "{}", b),
        }
    }
}

impl AtomValue for TestValue {
    fn null() -> Self {
        TestValue::Number(0.0) // Null falls back to Number(0.0)
    }
}

impl TestValue {
    /// Extract as f64, panics if not Number variant
    #[allow(dead_code)]
    pub fn as_number(&self) -> Option<f64> {
        match self {
            TestValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Extract as String, panics if not Text variant
    #[allow(dead_code)]
    pub fn as_text(&self) -> Option<String> {
        match self {
            TestValue::Text(s) => Some(s.clone()),
            _ => None,
        }
    }

    /// Extract as bool, panics if not Boolean variant
    #[allow(dead_code)]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            TestValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }
}

// Shorthand constructors for test convenience
pub fn num(v: f64) -> TestValue {
    TestValue::Number(v)
}

#[allow(dead_code)]
pub fn txt(v: &str) -> TestValue {
    TestValue::Text(v.to_string())
}

#[allow(dead_code)]
pub fn bool(v: bool) -> TestValue {
    TestValue::Boolean(v)
}
