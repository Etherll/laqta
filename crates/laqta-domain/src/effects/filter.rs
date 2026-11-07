//! Filled in by M3 task (effects: filter).

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Filter {}

impl Filter {
    pub fn name(&self) -> &'static str {
        match *self {}
    }
}
