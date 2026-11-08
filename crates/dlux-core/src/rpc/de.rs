//! Serde helpers for aria2's JSON encoding.
//!
//! aria2 encodes every number as a JSON string (`"totalLength": "1048576"`) and every boolean as
//! the string `"true"` / `"false"`. These helpers accept both the string form and the native JSON
//! form so the types stay robust against other aria2-compatible servers.

use serde::de::{self, Deserializer, Visitor};
use std::fmt;

/// Deserialize a `u64` from a string or a number. Empty strings become `0`.
pub fn u64_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = u64;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an unsigned integer or a string containing one")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u64, E> {
            Ok(v)
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<u64, E> {
            u64::try_from(v).map_err(|_| E::custom(format!("negative number {v}")))
        }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<u64, E> {
            if v.is_finite() && v >= 0.0 {
                Ok(v as u64)
            } else {
                Err(E::custom(format!("invalid number {v}")))
            }
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<u64, E> {
            let v = v.trim();
            if v.is_empty() {
                return Ok(0);
            }
            v.parse()
                .map_err(|_| E::custom(format!("invalid number string {v:?}")))
        }
        fn visit_unit<E: de::Error>(self) -> Result<u64, E> {
            Ok(0)
        }
    }
    d.deserialize_any(V)
}

/// Deserialize a `u32` from a string or a number.
pub fn u32_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let v = u64_lenient(d)?;
    u32::try_from(v).map_err(|_| de::Error::custom(format!("number {v} out of range")))
}

/// Deserialize an optional `u32` (e.g. `errorCode`), treating a missing/empty value as `None`.
pub fn opt_u32_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    #[derive(serde::Deserialize)]
    struct Wrap(#[serde(deserialize_with = "u32_lenient")] u32);
    let v: Option<serde_json::Value> = serde::Deserialize::deserialize(d)?;
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(v) => serde_json::from_value::<Wrap>(v)
            .map(|w| Some(w.0))
            .map_err(de::Error::custom),
    }
}

/// Deserialize a boolean from `"true"` / `"false"` strings or native booleans.
pub fn bool_lenient<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = bool;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a boolean or the string \"true\"/\"false\"")
        }
        fn visit_bool<E: de::Error>(self, v: bool) -> Result<bool, E> {
            Ok(v)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<bool, E> {
            match v.trim() {
                "true" => Ok(true),
                "false" | "" => Ok(false),
                other => Err(E::custom(format!("invalid boolean string {other:?}"))),
            }
        }
        fn visit_unit<E: de::Error>(self) -> Result<bool, E> {
            Ok(false)
        }
    }
    d.deserialize_any(V)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct T {
        #[serde(default, deserialize_with = "u64_lenient")]
        n: u64,
        #[serde(default, deserialize_with = "bool_lenient")]
        b: bool,
        #[serde(default, deserialize_with = "opt_u32_lenient")]
        o: Option<u32>,
    }

    fn parse(s: &str) -> T {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn numbers_from_strings_and_numbers() {
        assert_eq!(parse(r#"{"n":"123"}"#).n, 123);
        assert_eq!(parse(r#"{"n":456}"#).n, 456);
        assert_eq!(parse(r#"{"n":""}"#).n, 0);
        assert_eq!(parse(r#"{}"#).n, 0);
        assert!(serde_json::from_str::<T>(r#"{"n":"abc"}"#).is_err());
        assert!(serde_json::from_str::<T>(r#"{"n":-1}"#).is_err());
    }

    #[test]
    fn booleans_from_strings() {
        assert!(parse(r#"{"b":"true"}"#).b);
        assert!(!parse(r#"{"b":"false"}"#).b);
        assert!(parse(r#"{"b":true}"#).b);
        assert!(!parse(r#"{}"#).b);
    }

    #[test]
    fn optional_numbers() {
        assert_eq!(parse(r#"{"o":"7"}"#).o, Some(7));
        assert_eq!(parse(r#"{"o":""}"#).o, None);
        assert_eq!(parse(r#"{"o":null}"#).o, None);
        assert_eq!(parse(r#"{}"#).o, None);
    }
}
