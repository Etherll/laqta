//! Small card images for the library and history views. A card used to hand the original capture
//! to `img`, which decodes it at full size on every window's first paint. Here each path is
//! decoded and scaled once through `App::thumbnail_file` (file read and decode off the GPUI
//! thread) and kept for the process, shared by every window.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, Context, EntityId, Global, RenderImage};

use crate::runtime::UiState;

/// Longest side of a card image, in pixels: sharp on a 2x display for the 150 px grid cards.
const MAX_SIDE: u32 = 400;
/// Entries kept before the cache drops its ready images; at most 400x400 RGBA (640 KB) each.
const CAPACITY: usize = 256;

enum Entry {
    /// Being decoded; these views repaint when it lands.
    Loading(Vec<EntityId>),
    Ready(Arc<RenderImage>),
    /// Not an image the codecs read, or the file is gone: the card shows its placeholder.
    Failed,
}

#[derive(Default)]
pub(crate) struct Thumbnails {
    entries: HashMap<PathBuf, Entry>,
}

impl Global for Thumbnails {}

/// The card image for `path`, or `None` while it loads (or when it can't be read). The first call
/// for a path starts the decode; the view that asked repaints when it's ready.
pub(crate) fn get<V: 'static>(path: &Path, cx: &mut Context<V>) -> Option<Arc<RenderImage>> {
    let viewer = cx.entity_id();
    let cache = cx.default_global::<Thumbnails>();
    match cache.entries.get_mut(path) {
        Some(Entry::Ready(image)) => return Some(image.clone()),
        Some(Entry::Failed) => return None,
        Some(Entry::Loading(waiting)) => {
            if !waiting.contains(&viewer) {
                waiting.push(viewer);
            }
            return None;
        }
        None => {}
    }
    if cache.entries.len() >= CAPACITY {
        cache.entries.retain(|_, e| matches!(e, Entry::Loading(_)));
    }
    cache.entries.insert(path.to_path_buf(), Entry::Loading(vec![viewer]));
    let app = cx.global::<UiState>().services.app.clone();
    let path = path.to_path_buf();
    cx.spawn(async move |_, cx| {
        let thumbnail = app.thumbnail_file(path.clone(), MAX_SIDE).await;
        let image = thumbnail.ok().and_then(|bitmap| crate::toast::render_image(&bitmap));
        cx.update(|cx| finish(path, image, cx));
    })
    .detach();
    None
}

fn finish(path: PathBuf, image: Option<Arc<RenderImage>>, cx: &mut App) {
    let entry = match image {
        Some(image) => Entry::Ready(image),
        None => Entry::Failed,
    };
    let previous = cx.default_global::<Thumbnails>().entries.insert(path, entry);
    if let Some(Entry::Loading(waiting)) = previous {
        for viewer in waiting {
            cx.notify(viewer);
        }
    }
}
