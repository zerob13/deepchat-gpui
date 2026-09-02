//! Closed, connection-scoped FTS5 capability probing.

use rusqlite::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtsTokenizer {
    Trigram,
    Unicode61,
}

impl FtsTokenizer {
    pub(crate) const fn sql_name(self) -> &'static str {
        match self {
            Self::Trigram => "trigram",
            Self::Unicode61 => "unicode61",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FtsCapability {
    Available(FtsTokenizer),
    Unavailable,
}

impl FtsCapability {
    pub fn tokenizer(self) -> Option<FtsTokenizer> {
        match self {
            Self::Available(tokenizer) => Some(tokenizer),
            Self::Unavailable => None,
        }
    }
}

pub(crate) trait CapabilityProbe {
    fn probe(&self, conn: &Connection, tokenizer: FtsTokenizer) -> bool;
}

pub(crate) struct SqliteCapabilityProbe;

impl CapabilityProbe for SqliteCapabilityProbe {
    fn probe(&self, conn: &Connection, tokenizer: FtsTokenizer) -> bool {
        let (create, drop) = match tokenizer {
            FtsTokenizer::Trigram => (
                "CREATE VIRTUAL TABLE temp.deepchat_fts_probe_trigram USING fts5(c, tokenize='trigram')",
                "DROP TABLE IF EXISTS temp.deepchat_fts_probe_trigram",
            ),
            FtsTokenizer::Unicode61 => (
                "CREATE VIRTUAL TABLE temp.deepchat_fts_probe_unicode61 USING fts5(c, tokenize='unicode61')",
                "DROP TABLE IF EXISTS temp.deepchat_fts_probe_unicode61",
            ),
        };
        let available = conn.execute_batch(create).is_ok();
        let _ = conn.execute_batch(drop);
        available
    }
}

pub(crate) fn detect_capability(conn: &Connection, probe: &dyn CapabilityProbe) -> FtsCapability {
    if probe.probe(conn, FtsTokenizer::Trigram) {
        FtsCapability::Available(FtsTokenizer::Trigram)
    } else if probe.probe(conn, FtsTokenizer::Unicode61) {
        FtsCapability::Available(FtsTokenizer::Unicode61)
    } else {
        FtsCapability::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    struct OrderedProbe {
        outcomes: [bool; 2],
        calls: RefCell<Vec<FtsTokenizer>>,
    }

    impl CapabilityProbe for OrderedProbe {
        fn probe(&self, _conn: &Connection, tokenizer: FtsTokenizer) -> bool {
            let index = self.calls.borrow().len();
            self.calls.borrow_mut().push(tokenizer);
            self.outcomes[index]
        }
    }

    #[test]
    fn probe_stops_at_trigram_success() {
        let conn = Connection::open_in_memory().unwrap();
        let probe = OrderedProbe {
            outcomes: [true, true],
            calls: RefCell::new(Vec::new()),
        };
        assert_eq!(
            detect_capability(&conn, &probe),
            FtsCapability::Available(FtsTokenizer::Trigram)
        );
        assert_eq!(*probe.calls.borrow(), vec![FtsTokenizer::Trigram]);
    }

    #[test]
    fn probe_falls_back_to_unicode61_then_unavailable() {
        let conn = Connection::open_in_memory().unwrap();
        for (outcomes, expected) in [
            (
                [false, true],
                FtsCapability::Available(FtsTokenizer::Unicode61),
            ),
            ([false, false], FtsCapability::Unavailable),
        ] {
            let probe = OrderedProbe {
                outcomes,
                calls: RefCell::new(Vec::new()),
            };
            assert_eq!(detect_capability(&conn, &probe), expected);
            assert_eq!(
                *probe.calls.borrow(),
                vec![FtsTokenizer::Trigram, FtsTokenizer::Unicode61]
            );
        }
    }
}
