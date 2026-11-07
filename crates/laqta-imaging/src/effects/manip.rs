//! Filled in by M3 task (effects: manip).

use laqta_domain::Bitmap;
use laqta_domain::effects::Manip;

use super::{EffectContext, EffectError};

pub(super) fn apply(_image: &Bitmap, effect: &Manip, _cx: &EffectContext<'_>) -> Result<Bitmap, EffectError> {
    match *effect {}
}
