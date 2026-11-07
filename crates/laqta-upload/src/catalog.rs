//! The bundled `.sxcu` catalog (ADR 004): one file per simple host under `catalog/`, embedded at
//! build time. Each entry's id is `sxcu:<file stem>`.

use crate::sxcu::CustomUploader;

/// Every bundled file as (file stem, JSON text).
pub const CATALOG: &[(&str, &str)] = &[
    ("chevereto", include_str!("../catalog/chevereto.sxcu")),
    ("vgy-me", include_str!("../catalog/vgy-me.sxcu")),
    ("imagebin", include_str!("../catalog/imagebin.sxcu")),
    ("img1", include_str!("../catalog/img1.sxcu")),
    ("pastebin", include_str!("../catalog/pastebin.sxcu")),
    ("paste-ee", include_str!("../catalog/paste-ee.sxcu")),
    ("hastebin", include_str!("../catalog/hastebin.sxcu")),
    ("onetimesecret", include_str!("../catalog/onetimesecret.sxcu")),
    ("pastie", include_str!("../catalog/pastie.sxcu")),
    ("upaste", include_str!("../catalog/upaste.sxcu")),
    ("slexy", include_str!("../catalog/slexy.sxcu")),
    ("paste2", include_str!("../catalog/paste2.sxcu")),
    ("pomf", include_str!("../catalog/pomf.sxcu")),
    ("uguu", include_str!("../catalog/uguu.sxcu")),
    ("transfer-sh", include_str!("../catalog/transfer-sh.sxcu")),
    ("filebin", include_str!("../catalog/filebin.sxcu")),
    ("plik", include_str!("../catalog/plik.sxcu")),
    ("lambda", include_str!("../catalog/lambda.sxcu")),
    ("s-ul", include_str!("../catalog/s-ul.sxcu")),
    ("img-fish", include_str!("../catalog/img-fish.sxcu")),
    ("puush", include_str!("../catalog/puush.sxcu")),
    ("immich", include_str!("../catalog/immich.sxcu")),
    ("seafile", include_str!("../catalog/seafile.sxcu")),
    ("streamable", include_str!("../catalog/streamable.sxcu")),
    ("yourls", include_str!("../catalog/yourls.sxcu")),
    ("polr", include_str!("../catalog/polr.sxcu")),
    ("kutt", include_str!("../catalog/kutt.sxcu")),
    ("zero-width", include_str!("../catalog/zero-width.sxcu")),
    ("is-gd", include_str!("../catalog/is-gd.sxcu")),
    ("v-gd", include_str!("../catalog/v-gd.sxcu")),
    ("tinyurl", include_str!("../catalog/tinyurl.sxcu")),
    ("turl", include_str!("../catalog/turl.sxcu")),
    ("2-gp", include_str!("../catalog/2-gp.sxcu")),
    ("qr-net", include_str!("../catalog/qr-net.sxcu")),
    ("vurl", include_str!("../catalog/vurl.sxcu")),
    ("nlcm", include_str!("../catalog/nlcm.sxcu")),
];

/// The bundled uploaders in table order, with their ids. A file that does not parse is logged and
/// left out; the catalog tests make sure none is.
pub fn catalog() -> Vec<(String, CustomUploader)> {
    CATALOG
        .iter()
        .filter_map(|(stem, json)| match CustomUploader::parse(json) {
            Ok(uploader) => Some((format!("sxcu:{stem}"), uploader)),
            Err(error) => {
                tracing::warn!(stem, %error, "skipping a bundled .sxcu file");
                None
            }
        })
        .collect()
}
