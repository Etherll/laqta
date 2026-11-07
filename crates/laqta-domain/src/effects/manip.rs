//! Filled in by M3 task (effects: manip).

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Manip {}

impl Manip {
    pub fn name(&self) -> &'static str {
        match *self {}
    }
}
