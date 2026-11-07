//! Model families (ADR 023): Laqta's implementation of one architecture each. A family says how
//! inputs are prepared, which outputs are read and how they become a result. Custom models are
//! checked against a family's signature.

pub mod o1;

use crate::catalog::Task;

/// The families this build implements. Catalog entries name one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// PP-OCR text detection (DBNet).
    O1Det,
    /// PP-OCR text recognition (CTC).
    O1Rec,
}

/// The tensor counts and ranks a family reads and writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    pub inputs: usize,
    pub input_rank: usize,
    pub outputs: usize,
    pub output_rank: usize,
}

impl Family {
    pub fn parse(name: &str) -> Option<Family> {
        match name {
            "o1-det" => Some(Family::O1Det),
            "o1-rec" => Some(Family::O1Rec),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Family::O1Det => "o1-det",
            Family::O1Rec => "o1-rec",
        }
    }

    /// The task a model of this family serves.
    pub fn task(self) -> Task {
        match self {
            Family::O1Det => Task::OcrDet,
            Family::O1Rec => Task::OcrRec,
        }
    }

    pub fn signature(self) -> Signature {
        match self {
            // Input [1, 3, H, W]; output the probability map [1, 1, H, W].
            Family::O1Det => Signature { inputs: 1, input_rank: 4, outputs: 1, output_rank: 4 },
            // Input [1, 3, 48, W]; output [1, T, classes].
            Family::O1Rec => Signature { inputs: 1, input_rank: 4, outputs: 1, output_rank: 3 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for family in [Family::O1Det, Family::O1Rec] {
            assert_eq!(Family::parse(family.name()), Some(family));
        }
        assert_eq!(Family::parse("u2net"), None);
    }

    #[test]
    fn each_family_serves_one_task() {
        assert_eq!(Family::O1Det.task(), Task::OcrDet);
        assert_eq!(Family::O1Rec.task(), Task::OcrRec);
    }
}
