//! Exact fixed-point decimal values for prices, rates, amounts, and credit
//! quantities. Binary floats are never used for money; decimals serialize as
//! canonical strings so JSON and SQLite round trips stay exact.

use rust_decimal::Decimal;
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExactDecimal(Decimal);

impl ExactDecimal {
    pub const ZERO: Self = Self(Decimal::ZERO);

    pub fn new(value: Decimal) -> Self {
        Self(value)
    }

    pub fn get(self) -> Decimal {
        self.0
    }

    /// Parse a decimal string without lossy normalization or exponent forms.
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }
        Decimal::from_str_exact(input).ok().map(Self)
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.0.checked_sub(other.0).map(Self)
    }

    pub fn checked_mul(self, other: Self) -> Option<Self> {
        self.0.checked_mul(other.0).map(Self)
    }
}

impl fmt::Display for ExactDecimal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Decimal's Display preserves the exact value and scale, which is the
        // canonical string form we persist.
        fmt::Display::fmt(&self.0, formatter)
    }
}

impl Serialize for ExactDecimal {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ExactDecimal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Only string payloads are accepted: a JSON number would have already
        // been parsed through a binary float and lost exactness.
        let text = String::deserialize(deserializer)?;
        ExactDecimal::parse(&text).ok_or_else(|| D::Error::custom("invalid exact decimal string"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_round_trips_stay_exact() {
        for text in ["0", "0.00", "12.34", "0.000001", "123456789.987654321"] {
            let value = ExactDecimal::parse(text).expect("parse");
            let json = serde_json::to_string(&value).expect("serialize");
            assert_eq!(json, format!("\"{text}\""));
            let back: ExactDecimal = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, value);
            assert_eq!(back.to_string(), text);
        }
    }

    #[test]
    fn json_numbers_are_rejected_to_avoid_binary_float() {
        assert!(serde_json::from_str::<ExactDecimal>("12.34").is_err());
        assert!(serde_json::from_str::<ExactDecimal>("0").is_err());
    }

    #[test]
    fn arithmetic_is_checked_and_exact() {
        let rate = ExactDecimal::parse("2.50").expect("rate");
        let quantity = ExactDecimal::parse("0.16384").expect("quantity");
        let product = rate.checked_mul(quantity).expect("product");
        assert_eq!(product.to_string(), "0.4096000");

        let sum = ExactDecimal::parse("0.1")
            .and_then(|tenth| tenth.checked_add(ExactDecimal::parse("0.2")?))
            .expect("sum");
        assert_eq!(sum.to_string(), "0.3");
    }

    #[test]
    fn overflow_returns_none_instead_of_wrapping() {
        let max = ExactDecimal::new(Decimal::MAX);
        assert!(max.checked_add(ExactDecimal::parse("1").unwrap()).is_none());
    }

    #[test]
    fn rejects_empty_and_malformed_input() {
        assert!(ExactDecimal::parse("").is_none());
        assert!(ExactDecimal::parse("   ").is_none());
        assert!(ExactDecimal::parse("not-a-number").is_none());
    }
}
