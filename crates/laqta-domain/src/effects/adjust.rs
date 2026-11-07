//! Filled in by M3 task (effects: adjust).

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Adjust {}

impl Adjust {
    pub fn name(&self) -> &'static str {
        match *self {}
    }
}
