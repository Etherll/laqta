//! Filled in by M3 task (effects: draw).

use laqta_domain::Bitmap;
use laqta_domain::effects::Draw;

use super::{EffectContext, EffectError};

pub(super) fn apply(_image: &Bitmap, effect: &Draw, _cx: &EffectContext<'_>) -> Result<Bitmap, EffectError> {
    match *effect {}
}
