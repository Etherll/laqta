//! Filled in by M3 task (effects: draw).

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Draw {}

impl Draw {
    pub fn name(&self) -> &'static str {
        match *self {}
    }
}
