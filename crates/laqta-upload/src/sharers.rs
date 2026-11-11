//! URL sharing (ADR 014). Each service is a template with `{url}` replaced by the percent-encoded
//! URL. Email opens the user's mail client through `mailto:`.

use std::sync::Arc;

use laqta_ports::{BoxFuture, Sharer, UploadError};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// Everything except the RFC 3986 unreserved characters is encoded, so the URL survives inside a
/// query value and cannot add parameters of its own.
const QUERY_VALUE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

/// The services as (id, template).
const TEMPLATES: &[(&str, &str)] = &[
    ("email", "mailto:?subject=&body={url}"),
    ("pushbullet", "https://www.pushbullet.com/?link={url}"),
    ("facebook", "https://www.facebook.com/sharer/sharer.php?u={url}"),
    ("reddit", "https://www.reddit.com/submit?url={url}"),
    ("pinterest", "https://pinterest.com/pin/create/button/?url={url}"),
    ("tumblr", "https://www.tumblr.com/share/link?url={url}"),
    ("linkedin", "https://www.linkedin.com/sharing/share-offsite/?url={url}"),
    ("vk", "https://vk.com/share.php?url={url}"),
    ("google-lens", "https://lens.google.com/uploadbyurl?url={url}"),
    ("bing-visual-search", "https://www.bing.com/images/search?view=detailv2&iss=sbi&q=imgurl:{url}"),
];

/// Every bundled sharer.
pub fn sharers() -> Vec<Arc<dyn Sharer>> {
    TEMPLATES.iter().map(|&(id, template)| Arc::new(Template { id, template }) as Arc<dyn Sharer>).collect()
}

/// A sharer that fills one URL template.
struct Template {
    id: &'static str,
    template: &'static str,
}

impl Sharer for Template {
    fn id(&self) -> &str {
        self.id
    }

    fn share_url(&self, url: &str) -> BoxFuture<'_, Result<Option<String>, UploadError>> {
        let encoded = utf8_percent_encode(url, QUERY_VALUE).to_string();
        let page = self.template.replace("{url}", &encoded);
        Box::pin(async move { Ok(Some(page)) })
    }
}
