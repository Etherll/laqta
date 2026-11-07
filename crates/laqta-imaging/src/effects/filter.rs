//! Filled in by M3 task (effects: filter).

use laqta_domain::Bitmap;
use laqta_domain::effects::Filter;

use super::{EffectContext, EffectError};

pub(super) fn apply(_image: &Bitmap, effect: &Filter, _cx: &EffectContext<'_>) -> Result<Bitmap, EffectError> {
    match *effect {}
}
