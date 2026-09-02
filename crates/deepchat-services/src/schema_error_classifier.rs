//! Stable classification of SQLite schema error messages.

use std::fmt;

/// Stable categories which may justify a non-destructive schema repair attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaErrorReason {
    MissingTable,
    MissingColumn,
    ColumnCountMismatch,
    TypeMismatch,
}

impl SchemaErrorReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingTable => "missing-table",
            Self::MissingColumn => "missing-column",
            Self::ColumnCountMismatch => "column-count-mismatch",
            Self::TypeMismatch => "type-mismatch",
        }
    }
}

/// A recognized schema-error category.
///
/// The matched identity is intentionally retained only as an internal dedupe key;
/// callers can expose `reason` without leaking driver message contents.
#[derive(Clone, PartialEq, Eq)]
pub struct SchemaErrorClassification {
    pub reason: SchemaErrorReason,
    dedupe_key: String,
}

impl fmt::Debug for SchemaErrorClassification {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SchemaErrorClassification")
            .field("reason", &self.reason.as_str())
            .finish()
    }
}

impl SchemaErrorClassification {
    #[allow(dead_code)]
    pub(crate) fn dedupe_key(&self) -> &str {
        &self.dedupe_key
    }
}

/// Classify the frozen SQLite message patterns without retaining the raw message.
pub fn classify_schema_error(message: &str) -> Option<SchemaErrorClassification> {
    let lower = message.to_ascii_lowercase();
    for (needle, reason) in [
        ("no such table:", SchemaErrorReason::MissingTable),
        ("has no column named", SchemaErrorReason::MissingColumn),
        ("no such column:", SchemaErrorReason::MissingColumn),
    ] {
        if let Some(offset) = lower.find(needle) {
            let identity = parse_identity(&message[offset + needle.len()..])?;
            return Some(classification(reason, identity));
        }
    }

    let table_offset = lower.find("table ")?;
    let rest = &message[table_offset + "table ".len()..];
    let (identity, rest) = parse_identity_with_rest(rest)?;
    let rest_lower = rest.to_ascii_lowercase();
    if column_count_suffix(&rest_lower) {
        return Some(classification(
            SchemaErrorReason::ColumnCountMismatch,
            identity,
        ));
    }
    None
}

fn classification(reason: SchemaErrorReason, identity: &str) -> SchemaErrorClassification {
    SchemaErrorClassification {
        reason,
        dedupe_key: format!("{}:{identity}", reason.as_str()),
    }
}

fn parse_identity(input: &str) -> Option<&str> {
    parse_identity_with_rest(input).map(|(identity, _)| identity)
}

fn parse_identity_with_rest(input: &str) -> Option<(&str, &str)> {
    let input = input.trim_start();
    if let Some(rest) = input.strip_prefix('"') {
        let end = rest.find('"')?;
        let identity = &rest[..end];
        if identity.is_empty() || !identity.bytes().all(valid_identity_byte) {
            return None;
        }
        return Some((identity, &rest[end + 1..]));
    }
    let end = input
        .find(|character: char| {
            !character.is_ascii_alphanumeric() && character != '_' && character != '-'
        })
        .unwrap_or(input.len());
    let identity = &input[..end];
    (!identity.is_empty()).then_some((identity, &input[end..]))
}

fn valid_identity_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn column_count_suffix(rest: &str) -> bool {
    let words = rest.split_ascii_whitespace().collect::<Vec<_>>();
    matches!(
        words.as_slice(),
        ["has", columns, column_word, "but", values, value_word, "were", "supplied", ..]
            if columns.bytes().all(|byte| byte.is_ascii_digit())
                && values.bytes().all(|byte| byte.is_ascii_digit())
                && matches!(*column_word, "column" | "columns")
                && matches!(*value_word, "value" | "values")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_frozen_patterns_and_rejects_near_misses() {
        for (message, reason) in [
            (
                "no such table: \"agent-notes\"",
                SchemaErrorReason::MissingTable,
            ),
            (
                "table foo has 2 columns but 3 values were supplied",
                SchemaErrorReason::ColumnCountMismatch,
            ),
            (
                "x HAS NO COLUMN NAMED field-name",
                SchemaErrorReason::MissingColumn,
            ),
            (
                "no such column: \"field-name\"",
                SchemaErrorReason::MissingColumn,
            ),
        ] {
            assert_eq!(classify_schema_error(message).unwrap().reason, reason);
        }
        for message in [
            "no table: foo",
            "no such table:",
            "table foo has two columns but 3 values were supplied",
        ] {
            assert!(classify_schema_error(message).is_none(), "{message}");
        }
    }
}
