//! URL sharers: each template fills `{url}` with the percent-encoded URL, so the page receives the
//! URL as one value.
#![allow(clippy::unwrap_used)]

use futures_lite::future::block_on;
use laqta_upload::sharers::sharers;
use url::Url;

/// A URL with a space, a non-ASCII letter, a query, a fragment and a second parameter inside it.
const PAGE: &str = "https://example.com/a b?x=1&y=é#frag";
const ENCODED: &str = "https%3A%2F%2Fexample.com%2Fa%20b%3Fx%3D1%26y%3D%C3%A9%23frag";

fn share(id: &str, url: &str) -> String {
    let all = sharers();
    let sharer = all.iter().find(|sharer| sharer.id() == id).unwrap_or_else(|| panic!("no sharer {id}"));
    block_on(sharer.share_url(url)).unwrap().expect("a page to open")
}

#[test]
fn the_m2_services_are_registered() {
    let ids: Vec<String> = sharers().iter().map(|sharer| sharer.id().to_string()).collect();
    assert_eq!(
        ids,
        ["email", "pushbullet", "facebook", "reddit", "pinterest", "tumblr", "linkedin", "vk", "google-lens", "bing-visual-search",]
    );
}

#[test]
fn reddit_encodes_the_whole_url_as_one_value() {
    assert_eq!(share("reddit", PAGE), format!("https://www.reddit.com/submit?url={ENCODED}"));
}

#[test]
fn facebook_and_bing_use_their_own_parameter_names() {
    assert_eq!(share("facebook", PAGE), format!("https://www.facebook.com/sharer/sharer.php?u={ENCODED}"));
    assert_eq!(share("bing-visual-search", PAGE), format!("https://www.bing.com/images/search?view=detailv2&iss=sbi&q=imgurl:{ENCODED}"));
}

#[test]
fn email_puts_the_url_in_the_body() {
    assert_eq!(share("email", PAGE), format!("mailto:?subject=&body={ENCODED}"));
}

#[test]
fn every_sharer_hands_the_url_back_intact() {
    for sharer in sharers() {
        let shared = block_on(sharer.share_url(PAGE)).unwrap().expect("a page to open");
        let parsed = Url::parse(&shared).unwrap_or_else(|error| panic!("{}: {error}", sharer.id()));
        assert!(parsed.query_pairs().any(|(_, value)| value.contains(PAGE)), "{} does not carry the URL: {shared}", sharer.id());
    }
}

#[test]
fn a_url_with_ampersands_cannot_add_parameters() {
    let shared = share("reddit", "https://example.com/?a=1&b=2");
    let parsed = Url::parse(&shared).unwrap();
    let pairs: Vec<(String, String)> = parsed.query_pairs().into_owned().collect();
    assert_eq!(pairs, vec![("url".to_string(), "https://example.com/?a=1&b=2".to_string())]);
}
