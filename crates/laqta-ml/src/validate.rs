//! Custom-model validation (ADR 023). A custom model is saved only if its inputs and outputs match
//! the family the user chose.

use std::path::Path;

use ort::session::Session;
use ort::value::{TensorElementType, ValueType};

use crate::error::MlError;
use crate::families::{Count, Family, Signature};
use crate::runtime::loaded_environment;

/// Checks that the model at `path` has the input and output counts and ranks that `family` expects.
/// The model is loaded on the CPU, so the check does not need a GPU. ONNX Runtime must already be
/// loaded by a `Runtime` in this process (`Runtime::ensure_ort`).
pub fn validate_custom(path: &Path, family: &str) -> Result<(), MlError> {
    let family = Family::parse(family).ok_or_else(|| MlError::UnknownFamily(family.to_owned()))?;
    let environment = loaded_environment()?;
    let mut builder = Session::builder(environment)
        .map_err(|error| MlError::Ort(error.to_string()))?
        .with_intra_threads(1)
        .map_err(|error| MlError::Ort(error.to_string()))?;
    let session = builder.commit_from_file(path).map_err(|error| MlError::Ort(format!("{}: {error}", path.display())))?;
    let inputs: Vec<&ValueType> = session.inputs().iter().map(|outlet| outlet.dtype()).collect();
    let outputs: Vec<&ValueType> = session.outputs().iter().map(|outlet| outlet.dtype()).collect();
    check_signature(family, family.signature(), &inputs, &outputs)
}

/// Compares a model's tensors with a family's signature. Counts come first, then every input and every
/// output must be float32 with an accepted rank.
fn check_signature(family: Family, want: Signature, inputs: &[&ValueType], outputs: &[&ValueType]) -> Result<(), MlError> {
    let fail = |detail: String| MlError::Shape { family: family.name().to_owned(), detail };
    if inputs.len() != want.inputs {
        return Err(fail(format!("it has {} inputs, expected {}", inputs.len(), want.inputs)));
    }
    let count_ok = match want.outputs {
        Count::Exactly(count) => outputs.len() == count,
        Count::AtLeast(count) => outputs.len() >= count,
    };
    if !count_ok {
        return Err(fail(format!("it has {} outputs, expected {}", outputs.len(), describe(want.outputs))));
    }
    for input in inputs {
        check_tensor(input, &[want.input_rank], "input").map_err(fail)?;
    }
    for output in outputs {
        check_tensor(output, want.output_ranks, "output").map_err(fail)?;
    }
    Ok(())
}

fn describe(count: Count) -> String {
    match count {
        Count::Exactly(count) => count.to_string(),
        Count::AtLeast(count) => format!("at least {count}"),
    }
}

fn check_tensor(value: &ValueType, ranks: &[usize], role: &str) -> Result<(), String> {
    let Some(shape) = value.tensor_shape() else {
        return Err(format!("an {role} is not a tensor"));
    };
    if value.tensor_type() != Some(TensorElementType::Float32) {
        return Err(format!("an {role} is not float32"));
    }
    if !ranks.contains(&shape.len()) {
        return Err(format!("an {role} has rank {}, expected one of {ranks:?}", shape.len()));
    }
    Ok(())
}
