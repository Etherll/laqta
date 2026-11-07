//! Model families (ADR 023): Laqta's implementation of one architecture each. A family says how
//! inputs are prepared, which outputs are read and how they become a result. Custom models are
//! checked against a family's signature.

pub mod lama;
pub mod matting;
pub mod o1;

use crate::catalog::Task;

/// The families this build implements. Catalog entries name one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// PP-OCR text detection (DBNet).
    O1Det,
    /// PP-OCR text recognition (CTC).
    O1Rec,
    /// Background matting with one parametric recipe: resize, normalise, read one output, activate,
    /// resize the alpha back. BiRefNet, BEN2 and U²-Net-p differ only in their numbers.
    MattingParametric,
    /// Big-LaMa inpainting on a 512 px crop around the mask.
    Lama,
}

/// How many output tensors a family accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    Exactly(usize),
    /// A model may export side outputs as well; the entry names the one the family reads.
    AtLeast(usize),
}

/// The tensor counts and ranks a family reads and writes. Every input must have `input_rank`, and
/// every output one of `output_ranks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    pub inputs: usize,
    pub input_rank: usize,
    pub outputs: Count,
    pub output_ranks: &'static [usize],
}

impl Family {
    /// Every family in this build, for the model manager's custom-model choice.
    pub const ALL: &'static [Family] = &[Family::O1Det, Family::O1Rec, Family::MattingParametric, Family::Lama];

    pub fn parse(name: &str) -> Option<Family> {
        match name {
            "o1-det" => Some(Family::O1Det),
            "o1-rec" => Some(Family::O1Rec),
            "matting-parametric" => Some(Family::MattingParametric),
            "lama" => Some(Family::Lama),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Family::O1Det => "o1-det",
            Family::O1Rec => "o1-rec",
            Family::MattingParametric => "matting-parametric",
            Family::Lama => "lama",
        }
    }

    /// The task a model of this family serves.
    pub fn task(self) -> Task {
        match self {
            Family::O1Det => Task::OcrDet,
            Family::O1Rec => Task::OcrRec,
            Family::MattingParametric => Task::Background,
            Family::Lama => Task::Inpaint,
        }
    }

    pub fn signature(self) -> Signature {
        match self {
            // Input [1, 3, H, W]; output the probability map [1, 1, H, W].
            Family::O1Det => Signature { inputs: 1, input_rank: 4, outputs: Count::Exactly(1), output_ranks: &[4] },
            // Input [1, 3, 48, W]; output [1, T, classes].
            Family::O1Rec => Signature { inputs: 1, input_rank: 4, outputs: Count::Exactly(1), output_ranks: &[3] },
            // Input [1, 3, S, S]; outputs [1, 1, H, W] or [1, H, W] (BiRefNet exports several).
            Family::MattingParametric => Signature { inputs: 1, input_rank: 4, outputs: Count::AtLeast(1), output_ranks: &[3, 4] },
            // Inputs image [1, 3, 512, 512] and mask [1, 1, 512, 512]; output [1, 3, 512, 512].
            Family::Lama => Signature { inputs: 2, input_rank: 4, outputs: Count::Exactly(1), output_ranks: &[4] },
        }
    }
}

/// The index of the output a family reads. A negative index counts from the end, so -1 is the last
/// output. Fails if the model has no such output.
pub fn resolve_output(index: i64, count: usize) -> Result<usize, String> {
    let resolved = if index < 0 { count as i64 + index } else { index };
    usize::try_from(resolved)
        .ok()
        .filter(|resolved| *resolved < count)
        .ok_or_else(|| format!("output {index} does not exist; the model has {count} outputs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for family in Family::ALL {
            assert_eq!(Family::parse(family.name()), Some(*family));
        }
        assert_eq!(Family::parse("u2net"), None);
    }

    #[test]
    fn each_family_serves_one_task() {
        assert_eq!(Family::O1Det.task(), Task::OcrDet);
        assert_eq!(Family::O1Rec.task(), Task::OcrRec);
        assert_eq!(Family::MattingParametric.task(), Task::Background);
        assert_eq!(Family::Lama.task(), Task::Inpaint);
    }

    #[test]
    fn signatures_match_the_families() {
        assert_eq!(Family::O1Det.signature().outputs, Count::Exactly(1));
        assert_eq!(Family::MattingParametric.signature().outputs, Count::AtLeast(1));
        assert_eq!(Family::MattingParametric.signature().output_ranks, &[3, 4]);
        let lama = Family::Lama.signature();
        assert_eq!((lama.inputs, lama.outputs), (2, Count::Exactly(1)));
    }

    #[test]
    fn a_negative_output_index_counts_from_the_end() {
        assert_eq!(resolve_output(-1, 3), Ok(2));
        assert_eq!(resolve_output(0, 7), Ok(0));
        assert_eq!(resolve_output(6, 7), Ok(6));
        assert!(resolve_output(7, 7).is_err());
        assert!(resolve_output(-4, 3).is_err());
        assert!(resolve_output(-1, 0).is_err());
    }
}
