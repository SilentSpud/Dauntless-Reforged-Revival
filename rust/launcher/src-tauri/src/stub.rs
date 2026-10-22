//! What a module answers until the row that fills it lands (MIGRATION_PLAN.md section 5.1, RA2). RA2
//! registers every command and every planned module up front, so each later row fills only its own
//! files; until then a stub answers this error, naming the row. The last row to fill a stub removes this
//! module with its last use.

use std::fmt;

use crate::bridge::types::{ActionResult, ErrorCode, LauncherError};

/// A stub's answer: the row of `MIGRATION_PLAN.md` that fills it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotImplemented {
    pub row: &'static str,
}

pub const fn not_implemented(row: &'static str) -> NotImplemented {
    NotImplemented { row }
}

impl NotImplemented {
    /// The `detail` the page receives: `not_implemented:RA13`.
    pub fn detail(self) -> String {
        format!("not_implemented:{}", self.row)
    }
}

impl fmt::Display for NotImplemented {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not implemented yet ({})", self.row)
    }
}

impl std::error::Error for NotImplemented {}

/// The page shows an unknown error; the detail says which row is missing.
impl From<NotImplemented> for LauncherError {
    fn from(stub: NotImplemented) -> Self {
        Self {
            code: ErrorCode::Unknown,
            detail: Some(stub.detail()),
        }
    }
}

impl From<NotImplemented> for ActionResult {
    fn from(stub: NotImplemented) -> Self {
        Self::Err(stub.into())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_page_reads_a_stub_as_an_unknown_error_naming_the_row() {
        let answer = ActionResult::from(not_implemented("RA12"));
        assert_eq!(
            serde_json::to_value(answer).unwrap(),
            json!({ "ok": false, "error": { "code": "unknown", "detail": "not_implemented:RA12" } })
        );
        assert_eq!(not_implemented("RA7").to_string(), "not implemented yet (RA7)");
    }
}
