//! Filled in by M3 task (effects: adjust).

use laqta_domain::Bitmap;
use laqta_domain::effects::Adjust;

use super::{EffectContext, EffectError};

pub(super) fn apply(_image: &Bitmap, effect: &Adjust, _cx: &EffectContext<'_>) -> Result<Bitmap, EffectError> {
    match *effect {}
}
