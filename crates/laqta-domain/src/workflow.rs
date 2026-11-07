//! Workflows (M0: only the identifier).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct WorkflowId(pub u32);
