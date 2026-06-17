//! Extract direct media download URLs from Instagram and Facebook via `snapsave.app`.
//!
//! Construct a [`SnapSave`] once (it builds its HTTP client and compiles its
//! regexes/selectors up front) and reuse it across calls:
//!
//! ```no_run
//! # async fn run() -> Result<(), snapsave_parser::Error> {
//! use snapsave_parser::SnapSave;
//!
//! let snap = SnapSave::new()?;
//! match snap.download("https://www.instagram.com/reel/CtjoC2BNsB2", None).await {
//!     Ok(data) => println!("{} media", data.media.len()),
//!     Err(error) => eprintln!("{error}"),
//! }
//! # Ok(()) }
//! ```
//!
//! Instagram and Facebook are served through the `snapsave.app/action.php`
//! endpoint. `TikTok` and Twitter/X URLs are recognized as valid but their
//! extraction flows are not implemented yet ([`Error::Unsupported`] is
//! returned); the [`Platform`] dispatch is structured so adding them later is a
//! single new match arm.

pub mod decrypter;
pub mod types;
pub mod utils;

pub use decrypter::{DecryptError, decrypt_snap_save};
pub use types::{Error, SnapSaveDownloaderData, SnapSaveDownloaderMedia, SnapSaveDownloaderOptions};
pub use utils::{USER_AGENT, fix_thumbnail};

use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::time::Duration;

/// A supported (or recognized-but-unimplemented) media platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Instagram,
    Facebook,
    Tiktok,
    Twitter,
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Platform::Instagram => "Instagram",
            Platform::Facebook => "Facebook",
            Platform::Tiktok => "TikTok",
            Platform::Twitter => "Twitter (X)",
        })
    }
}

// Rust's `regex` crate has no `/g` flag (irrelevant for `is_match`) and no
// look-around, so the Facebook pattern drops the original trailing `(?!/)` —
// it is redundant, the following `[^/?#&]+` already forbids `/` at that spot.
const FACEBOOK_PATTERN: &str = r"^https?://(?:www\.|web\.|m\.)?facebook\.com/(watch(\?v=|/\?v=)[0-9]+|reel/[0-9]+|[a-zA-Z0-9.\-_]+/(videos|posts)/[0-9]+|[0-9]+/(videos|posts)/[0-9]+|[a-zA-Z0-9]+/(videos|posts)/[0-9]+|share/(v|r)/[a-zA-Z0-9]+/?)([^/?#&]+).*$|^https://fb\.watch/[a-zA-Z0-9]+$";
const INSTAGRAM_PATTERN: &str =
    r"^https?://(?:www\.)?instagram\.com/(?:[^/]+/)?(?:p|reel|reels|tv|stories|share)/([^/?#&]+).*";
const TIKTOK_PATTERN: &str =
    r"^https?://(?:www\.|m\.|vm\.|vt\.)?tiktok\.com/(?:@[^/]+/(?:video|photo)/\d+|v/\d+|t/[\w]+|[\w]+)/?";
const TWITTER_PATTERN: &str = r"^https://(?:x|twitter)\.com(?:/(?:i/web|[^/]+)/status/(\d+)(?:.*)?)?$";
const WWW_PATTERN: &str = r"^(https?://)([^./]+\.[^./]+)(/.*)?$";
const PROGRESS_PATTERN: &str = r"(?i)get_progressApi";
const PROGRESS_ARG_PATTERN: &str = r"get_progressApi\('(.*?)'\)";

fn build_client(proxy: Option<&str>) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder();
    builder = match proxy {
        Some(proxy) => builder.proxy(reqwest::Proxy::all(proxy)?),
        None => builder.no_proxy(),
    };
    builder.build()
}

/// Compiled regexes used for URL detection, normalization, and render-link parsing.
struct Regexes {
    facebook: Regex,
    instagram: Regex,
    tiktok: Regex,
    twitter: Regex,
    www: Regex,
    progress: Regex,
    progress_arg: Regex,
}

impl Regexes {
    fn new() -> Result<Self, regex::Error> {
        Ok(Self {
            facebook: Regex::new(FACEBOOK_PATTERN)?,
            instagram: Regex::new(INSTAGRAM_PATTERN)?,
            tiktok: Regex::new(TIKTOK_PATTERN)?,
            twitter: Regex::new(TWITTER_PATTERN)?,
            www: Regex::new(WWW_PATTERN)?,
            progress: Regex::new(PROGRESS_PATTERN)?,
            progress_arg: Regex::new(PROGRESS_ARG_PATTERN)?,
        })
    }

    /// TikTok/Twitter are checked first; any other recognized URL is served by
    /// the snapsave.app flow. The patterns are mutually exclusive.
    fn detect(&self, url: &str) -> Option<Platform> {
        if self.tiktok.is_match(url) {
            Some(Platform::Tiktok)
        } else if self.twitter.is_match(url) {
            Some(Platform::Twitter)
        } else if self.instagram.is_match(url) {
            Some(Platform::Instagram)
        } else if self.facebook.is_match(url) {
            Some(Platform::Facebook)
        } else {
            None
        }
    }

    /// Normalize a URL before submitting it: Twitter/X is left as-is, Instagram
    /// query strings are stripped, and a bare two-label host gains a `www.`.
    fn normalize_url(&self, url: &str) -> String {
        if self.twitter.is_match(url) {
            return url.to_string();
        }
        let mut normalized = url.to_string();
        if self.instagram.is_match(&normalized)
            && let Some(query_start) = normalized.find('?')
        {
            normalized.truncate(query_start);
        }
        // The pattern is anchored start-to-end, so the replacement rewrites the
        // whole string when it matches and leaves it untouched otherwise.
        if utils::needs_www(&normalized) {
            normalized = self.www.replace(&normalized, "${1}www.${2}${3}").into_owned();
        }
        normalized
    }

    fn matches_progress(&self, url: &str) -> bool {
        self.progress.is_match(url)
    }

    /// Extract the `<token>` from a `get_progressApi('<token>')` onclick handler.
    fn progress_token(&self, url: &str) -> Option<String> {
        self.progress_arg
            .captures(url)?
            .get(1)
            .map(|group| group.as_str().to_string())
    }
}

/// Compiled CSS selectors for the download-section markup.
struct Selectors {
    table: Selector,
    figure: Selector,
    figure_img: Selector,
    video_des: Selector,
    tbody_tr: Selector,
    td: Selector,
    anchor: Selector,
    button: Selector,
    card: Selector,
    card_body: Selector,
    download_items: Selector,
    item_thumb_img: Selector,
    item_btn: Selector,
    span: Selector,
}

impl Selectors {
    fn new() -> Result<Self, scraper::error::SelectorErrorKind<'static>> {
        Ok(Self {
            table: Selector::parse("table.table")?,
            figure: Selector::parse("article.media > figure")?,
            figure_img: Selector::parse("article.media > figure img")?,
            video_des: Selector::parse("span.video-des")?,
            tbody_tr: Selector::parse("tbody > tr")?,
            td: Selector::parse("td")?,
            anchor: Selector::parse("a")?,
            button: Selector::parse("button")?,
            card: Selector::parse("div.card")?,
            card_body: Selector::parse("div.card-body")?,
            download_items: Selector::parse("div.download-items")?,
            item_thumb_img: Selector::parse("div.download-items__thumb > img")?,
            item_btn: Selector::parse("div.download-items__btn")?,
            span: Selector::parse("span")?,
        })
    }
}

/// A reusable snapsave parser. It owns its HTTP client and compiled
/// regexes/selectors, all built once at construction.
pub struct SnapSave {
    client: reqwest::Client,
    regexes: Regexes,
    selectors: Selectors,
}

impl SnapSave {
    /// Build a parser with a default HTTP client (no proxy).
    ///
    /// # Errors
    /// Returns an error if the HTTP client or a built-in pattern fails to build.
    pub fn new() -> Result<Self, Error> {
        Self::build(None)
    }

    /// Build a parser whose client routes every request through `proxy`
    /// (`http://`, `https://`, or `socks5://`).
    ///
    /// # Errors
    /// Returns an error if the proxy/client or a built-in pattern fails to build.
    pub fn with_proxy(proxy: &str) -> Result<Self, Error> {
        Self::build(Some(proxy))
    }

    fn build(proxy: Option<&str>) -> Result<Self, Error> {
        Ok(Self {
            client: build_client(proxy)?,
            regexes: Regexes::new()?,
            selectors: Selectors::new()?,
        })
    }

    /// Detect the platform of `url`, or `None` if it matches no known pattern.
    #[must_use]
    pub fn detect(&self, url: &str) -> Option<Platform> {
        self.regexes.detect(url)
    }

    /// Normalize a URL the way the snapsave.app flow expects.
    #[must_use]
    pub fn normalize_url(&self, url: &str) -> String {
        self.regexes.normalize_url(url)
    }

    /// Fetch and parse the media behind `url`.
    ///
    /// # Errors
    /// - [`Error::InvalidUrl`] / [`Error::Unsupported`] if `url` is unrecognized
    ///   or for a not-yet-implemented platform,
    /// - [`Error::Request`] / [`Error::Decrypt`] on transport or decode failure,
    /// - [`Error::Blank`] if the response decoded but yielded no media.
    pub async fn download(
        &self,
        url: &str,
        options: Option<SnapSaveDownloaderOptions>,
    ) -> Result<SnapSaveDownloaderData, Error> {
        let normalized = match self.regexes.detect(url) {
            None => return Err(Error::InvalidUrl),
            Some(platform @ (Platform::Tiktok | Platform::Twitter)) => return Err(Error::Unsupported(platform)),
            Some(Platform::Instagram | Platform::Facebook) => self.regexes.normalize_url(url),
        };
        let opts = options.unwrap_or_default();
        let html = self.fetch(&normalized, &opts).await?;
        let decoded = decrypt_snap_save(&html)?;
        self.parse_html(&decoded)
    }

    /// POST to the action endpoint, retrying up to `opts.retry` extra times.
    async fn fetch(&self, normalized_url: &str, opts: &SnapSaveDownloaderOptions) -> Result<String, Error> {
        let user_agent = opts.user_agent.as_deref().unwrap_or(USER_AGENT);
        let retry = opts.retry.unwrap_or(1);
        let retry_delay = opts.retry_delay.unwrap_or(500);

        let mut attempt: u32 = 0;
        loop {
            match self.post_action(normalized_url, user_agent).await {
                Ok(text) => return Ok(text),
                Err(err) => {
                    if attempt >= retry {
                        return Err(err);
                    }
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(retry_delay)).await;
                }
            }
        }
    }

    async fn post_action(&self, normalized_url: &str, user_agent: &str) -> Result<String, Error> {
        let text = self
            .client
            .post("https://snapsave.app/action.php")
            .query(&[("lang", "en")])
            .header("accept", "*/*")
            .header("origin", "https://snapsave.app")
            .header("referer", "https://snapsave.app/")
            .header("user-agent", user_agent)
            .form(&[("url", normalized_url)])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(text)
    }

    /// Parse decoded download-section HTML (the output of [`decrypt_snap_save`])
    /// into data, applying the same CSS-branch logic as [`download`](Self::download).
    ///
    /// # Errors
    /// Returns [`Error::Blank`] if the markup contains no media.
    pub fn parse_html(&self, decoded: &str) -> Result<SnapSaveDownloaderData, Error> {
        parse(decoded, &self.selectors, &self.regexes)
    }
}

/// Parse the decoded download-section HTML into data, selecting the media shape
/// (download-items, video table, cards, or a single link) by structure.
fn parse(decoded: &str, selectors: &Selectors, regexes: &Regexes) -> Result<SnapSaveDownloaderData, Error> {
    let document = Html::parse_document(decoded);
    let root = document.root_element();

    let mut data = SnapSaveDownloaderData::default();
    let mut media: Vec<SnapSaveDownloaderMedia> = Vec::new();

    let has_table = exists(&root, &selectors.table);
    let has_figure = exists(&root, &selectors.figure);

    if has_table || has_figure {
        data.description = Some(trimmed_text(&root, &selectors.video_des));
        data.preview = first_attr(&root, &selectors.figure_img, "src");

        if has_table {
            media.extend(
                root.select(&selectors.tbody_tr)
                    .map(|row| parse_table_row(row, selectors, regexes)),
            );
        } else if exists(&root, &selectors.card) {
            media.extend(root.select(&selectors.card).map(|card| parse_card(card, selectors)));
        } else {
            media.push(parse_single_link(root, selectors));
        }
    } else if exists(&root, &selectors.download_items) {
        media.extend(
            root.select(&selectors.download_items)
                .map(|item| parse_download_item(item, selectors)),
        );
    }

    if media.is_empty() {
        return Err(Error::Blank);
    }
    data.media = media;
    Ok(data)
}

/// A Facebook video-table row: resolution + (possibly render-gated) URL.
fn parse_table_row(row: ElementRef, selectors: &Selectors, regexes: &Regexes) -> SnapSaveDownloaderMedia {
    let cells: Vec<ElementRef> = row.select(&selectors.td).collect();
    let resolution = cells
        .first()
        .map(|cell| cell.text().collect::<String>())
        .unwrap_or_default();
    let mut url = cells
        .get(2)
        .and_then(|cell| first_attr(cell, &selectors.anchor, "href"))
        .or_else(|| {
            cells
                .get(2)
                .and_then(|cell| first_attr(cell, &selectors.button, "onclick"))
        });

    let should_render = url.as_deref().is_some_and(|link| regexes.matches_progress(link));
    // A render-gated link wraps the real token in `get_progressApi('<token>')`. If
    // the token is present, build the absolute render URL; otherwise keep the
    // original URL (a missing token would otherwise yield ".../snapsave.appundefined").
    if should_render && let Some(token) = url.as_deref().and_then(|link| regexes.progress_token(link)) {
        url = Some(format!("https://snapsave.app{token}"));
    }

    SnapSaveDownloaderMedia {
        should_render: should_render.then_some(true),
        r#type: Some(media_type(!resolution.is_empty())),
        resolution: Some(resolution),
        url,
        ..Default::default()
    }
}

/// A `div.card` entry (Facebook image/album cards).
fn parse_card(card: ElementRef, selectors: &Selectors) -> SnapSaveDownloaderMedia {
    let body = card.select(&selectors.card_body).next();
    let label = body
        .map(|node| trimmed_text(&node, &selectors.anchor))
        .unwrap_or_default();
    let url = body.and_then(|node| first_attr(&node, &selectors.anchor, "href"));
    link_media(url, &label)
}

/// The single anchor/button fallback within a `table.table`/`figure` page.
fn parse_single_link(root: ElementRef, selectors: &Selectors) -> SnapSaveDownloaderMedia {
    let url = first_attr(&root, &selectors.anchor, "href").or_else(|| first_attr(&root, &selectors.button, "onclick"));
    let label = trimmed_text(&root, &selectors.anchor);
    link_media(url, &label)
}

/// A `div.download-items` entry (the Instagram path).
fn parse_download_item(item: ElementRef, selectors: &Selectors) -> SnapSaveDownloaderMedia {
    let thumbnail = first_attr(&item, &selectors.item_thumb_img, "src");
    let button = item.select(&selectors.item_btn).next();
    let download_url = button.and_then(|node| first_attr(&node, &selectors.anchor, "href"));
    let label = button
        .map(|node| trimmed_text(&node, &selectors.span))
        .unwrap_or_default();

    if label == "Download Photo" {
        SnapSaveDownloaderMedia {
            url: thumbnail,
            r#type: Some(media_type(false)),
            ..Default::default()
        }
    } else {
        SnapSaveDownloaderMedia {
            url: download_url,
            thumbnail: thumbnail.as_deref().map(fix_thumbnail),
            r#type: Some(media_type(true)),
            ..Default::default()
        }
    }
}

/// Build a `{ url, type }` entry, choosing `image` for a "Download Photo" label.
fn link_media(url: Option<String>, label: &str) -> SnapSaveDownloaderMedia {
    SnapSaveDownloaderMedia {
        url,
        r#type: Some(media_type(label != "Download Photo")),
        ..Default::default()
    }
}

fn media_type(is_video: bool) -> String {
    if is_video { "video" } else { "image" }.to_string()
}

fn exists(scope: &ElementRef, selector: &Selector) -> bool {
    scope.select(selector).next().is_some()
}

/// First matching element's attribute (cheerio `.attr`).
fn first_attr(scope: &ElementRef, selector: &Selector, attr: &str) -> Option<String> {
    scope.select(selector).next()?.value().attr(attr).map(str::to_string)
}

/// Concatenated text of all matching elements (cheerio `.text()`).
fn text_concat(scope: &ElementRef, selector: &Selector) -> String {
    scope.select(selector).flat_map(|element| element.text()).collect()
}

/// Trimmed concatenated text of all matching elements.
fn trimmed_text(scope: &ElementRef, selector: &Selector) -> String {
    text_concat(scope, selector).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser() -> SnapSave {
        SnapSave::new().expect("patterns and client build")
    }

    #[test]
    fn detect_classifies_platforms() {
        let snap = parser();
        assert_eq!(
            snap.detect("https://www.instagram.com/reel/CtjoC2BNsB2"),
            Some(Platform::Instagram)
        );
        assert_eq!(
            snap.detect("https://www.facebook.com/watch?v=1234567890123456"),
            Some(Platform::Facebook)
        );
        assert_eq!(
            snap.detect("https://www.tiktok.com/@u/video/1234567890123456789"),
            Some(Platform::Tiktok)
        );
        assert_eq!(
            snap.detect("https://x.com/u/status/1234567890123456789"),
            Some(Platform::Twitter)
        );
        assert_eq!(snap.detect("https://example.com/foo"), None);
    }

    #[test]
    fn detect_matches_every_facebook_shape() {
        let snap = parser();
        let urls = [
            "https://facebook.com/watch?v=1234567890123456",
            "https://www.facebook.com/watch?v=1234567890123456",
            "https://www.facebook.com/watch/?v=1234567890123456",
            "https://www.facebook.com/reel/1234567890123456",
            "https://www.facebook.com/reel/1234567890123456?locale=es_LA",
            "https://web.facebook.com/reel/1234567890123456",
            "https://www.facebook.com/user_name/videos/1234567890123456",
            "https://www.facebook.com/user_name/posts/1234567890123456",
            "https://www.facebook.com/1234567890123456/videos/1234567890123456",
            "https://www.facebook.com/1234567890123456/posts/1234567890123456",
            "https://www.facebook.com/Page_Name/videos/1234567890123456",
            "https://www.facebook.com/Page_Name/posts/1234567890123456",
            "https://fb.watch/abCDefghI9",
            "https://www.facebook.com/share/v/ab0cdEfGHIJKMnlO/",
            "https://www.facebook.com/share/r/ab0cdEfGHIJKMnlO/",
        ];
        for url in urls {
            assert_eq!(snap.detect(url), Some(Platform::Facebook), "facebook: {url}");
        }
    }

    #[test]
    fn detect_matches_every_instagram_shape() {
        let snap = parser();
        let urls = [
            "https://instagram.com/p/CcDeFg9hiJK",
            "https://www.instagram.com/p/CcDeFg9hiJK",
            "https://www.instagram.com/p/CcDeFg9hiJK/",
            "https://www.instagram.com/reel/1234567890123456",
            "https://www.instagram.com/reel/1234567890123456/",
            "https://www.instagram.com/reels/1234567890123456",
            "https://www.instagram.com/reels/1234567890123456/",
            "https://www.instagram.com/tv/CcDeFg9hiJK",
            "https://www.instagram.com/tv/CcDeFg9hiJK/",
            "https://www.instagram.com/stories/user_name",
            "https://www.instagram.com/stories/user_name/",
            "https://www.instagram.com/stories/user_name/1234567890123456",
            "https://www.instagram.com/stories/user_name/1234567890123456/",
            "https://instagram.com/user_name/p/CcDeFg9hiJK",
            "https://www.instagram.com/user_name/reel/1234567890123456/",
        ];
        for url in urls {
            assert_eq!(snap.detect(url), Some(Platform::Instagram), "instagram: {url}");
        }
    }

    #[test]
    fn detect_matches_every_tiktok_shape() {
        let snap = parser();
        let urls = [
            "https://tiktok.com/@user_name/video/1234567890123456789",
            "https://www.tiktok.com/@user_name/video/1234567890123456789/",
            "https://www.tiktok.com/@user_name/video/1234567890123456789?is_from_webapp=1&sender_device=pc",
            "https://www.tiktok.com/@user_name/photo/1234567890123456789/",
            "https://www.tiktok.com/@user_name/photo/1234567890123456789?is_from_webapp=1&sender_device=pc",
            "https://www.tiktok.com/t/ZTFErV3e5",
            "https://www.tiktok.com/t/ZTFErV3e5/",
            "https://vm.tiktok.com/ZAbc8d911",
            "https://vm.tiktok.com/ZAbc8d911/",
            "https://m.tiktok.com/v/1234567890123456789",
            "https://m.tiktok.com/v/1234567890123456789/",
            "https://vt.tiktok.com/ZAbc8d911",
            "https://vt.tiktok.com/ZAbc8d911/",
        ];
        for url in urls {
            assert_eq!(snap.detect(url), Some(Platform::Tiktok), "tiktok: {url}");
        }
    }

    #[test]
    fn detect_matches_every_twitter_shape() {
        let snap = parser();
        let urls = [
            "https://twitter.com/user_name/status/1234567890123456789",
            "https://x.com/user_name/status/1234567890123456789",
            "https://twitter.com/i/web/status/1234567890123456789",
            "https://x.com/i/web/status/1234567890123456789",
            "https://twitter.com/user_name/status/1234567890123456789/photo/1",
            "https://x.com/user_name/status/1234567890123456789/photo/1",
        ];
        for url in urls {
            assert_eq!(snap.detect(url), Some(Platform::Twitter), "twitter: {url}");
        }
    }

    #[test]
    fn detect_rejects_unrelated_urls() {
        let snap = parser();
        let none = [
            "https://www.example.com/invalid",
            "https://instagram.com/",
            "https://instagram.com/explore/tags/foo",
            "ftp://facebook.com/watch?v=1",
            "https://youtube.com/watch?v=abc",
            "not a url",
        ];
        for url in none {
            assert_eq!(snap.detect(url), None, "should reject: {url}");
        }
    }

    #[test]
    fn normalize_url_cases() {
        let snap = parser();
        assert_eq!(snap.normalize_url("https://tiktok.com"), "https://www.tiktok.com");
        assert_eq!(snap.normalize_url("https://www.tiktok.com"), "https://www.tiktok.com");
        assert_eq!(snap.normalize_url("https://vm.tiktok.com"), "https://vm.tiktok.com");
        assert_eq!(snap.normalize_url("https://x.com"), "https://x.com");
        assert_eq!(
            snap.normalize_url("https://www.instagram.com/reel/CtjoC2BNsB2?igsh=abc"),
            "https://www.instagram.com/reel/CtjoC2BNsB2"
        );
        assert_eq!(
            snap.normalize_url("https://instagram.com/p/CcDeFg9hiJK"),
            "https://www.instagram.com/p/CcDeFg9hiJK"
        );
        // Three-label hosts don't match the www-insertion pattern.
        assert_eq!(
            snap.normalize_url("https://web.facebook.com/reel/1"),
            "https://web.facebook.com/reel/1"
        );
    }

    #[test]
    fn parses_instagram_download_items_video() {
        let html = r#"
            <div class="download-items">
              <div class="download-items__thumb"><img src="https://cdn/thumb.jpg"></div>
              <div class="download-items__btn"><a href="https://cdn/video.mp4"><span>Download video</span></a></div>
            </div>"#;
        let data = parser().parse_html(html).expect("success");
        assert_eq!(data.media.len(), 1);
        assert_eq!(data.media[0].url.as_deref(), Some("https://cdn/video.mp4"));
        assert_eq!(data.media[0].thumbnail.as_deref(), Some("https://cdn/thumb.jpg"));
        assert_eq!(data.media[0].r#type.as_deref(), Some("video"));
        // download-items never sets description/preview
        assert_eq!(data.description, None);
    }

    #[test]
    fn parses_download_items_photo_uses_thumbnail_as_url() {
        let html = r#"
            <div class="download-items">
              <div class="download-items__thumb"><img src="https://cdn/photo.jpg"></div>
              <div class="download-items__btn"><a href="https://cdn/ignored"><span>Download Photo</span></a></div>
            </div>"#;
        let data = parser().parse_html(html).expect("success");
        assert_eq!(data.media[0].url.as_deref(), Some("https://cdn/photo.jpg"));
        assert_eq!(data.media[0].r#type.as_deref(), Some("image"));
        assert_eq!(data.media[0].thumbnail, None);
    }

    #[test]
    fn download_items_video_fixes_snapinsta_thumbnail() {
        let html = r#"
            <div class="download-items">
              <div class="download-items__thumb"><img src="https://snapinsta.app/photo.php?photo=https%3A%2F%2Fy.com%2Ft.jpg"></div>
              <div class="download-items__btn"><a href="https://cdn/v.mp4"><span>Download video</span></a></div>
            </div>"#;
        let data = parser().parse_html(html).expect("success");
        assert_eq!(data.media[0].thumbnail.as_deref(), Some("https://y.com/t.jpg"));
    }

    #[test]
    fn parses_facebook_table_with_description_preview_and_render() {
        let html = r#"
            <span class="video-des">  My caption  </span>
            <article class="media"><figure><img src="https://cdn/preview.jpg"></figure></article>
            <table class="table"><tbody>
              <tr><td>720p (HD)</td><td></td><td><a href="https://cdn/v720.mp4">dl</a></td></tr>
              <tr><td>1080p</td><td></td><td><button onclick="get_progressApi('/render.php?token=abc')">x</button></td></tr>
            </tbody></table>"#;
        let data = parser().parse_html(html).expect("success");
        assert_eq!(data.description.as_deref(), Some("My caption"));
        assert_eq!(data.preview.as_deref(), Some("https://cdn/preview.jpg"));
        assert_eq!(data.media.len(), 2);

        assert_eq!(data.media[0].resolution.as_deref(), Some("720p (HD)"));
        assert_eq!(data.media[0].url.as_deref(), Some("https://cdn/v720.mp4"));
        assert_eq!(data.media[0].r#type.as_deref(), Some("video"));
        assert_eq!(data.media[0].should_render, None);

        assert_eq!(data.media[1].resolution.as_deref(), Some("1080p"));
        assert_eq!(data.media[1].should_render, Some(true));
        assert_eq!(
            data.media[1].url.as_deref(),
            Some("https://snapsave.app/render.php?token=abc")
        );
    }

    #[test]
    fn table_render_without_token_keeps_original_url() {
        let html = r#"
            <table class="table"><tbody>
              <tr><td>720p</td><td></td><td><button onclick="get_progressApi()">x</button></td></tr>
            </tbody></table>"#;
        let data = parser().parse_html(html).expect("success");
        assert_eq!(data.media[0].should_render, Some(true));
        assert_eq!(data.media[0].url.as_deref(), Some("get_progressApi()"));
    }

    #[test]
    fn table_row_without_resolution_is_image() {
        let html = r#"
            <table class="table"><tbody>
              <tr><td></td><td></td><td><a href="https://cdn/p.jpg">dl</a></td></tr>
            </tbody></table>"#;
        let data = parser().parse_html(html).expect("success");
        assert_eq!(data.media[0].r#type.as_deref(), Some("image"));
        assert_eq!(data.media[0].resolution.as_deref(), Some(""));
    }

    #[test]
    fn parses_card_branch() {
        let html = r#"
            <article class="media"><figure><img src="https://cdn/p.jpg"></figure></article>
            <div class="card"><div class="card-body"><a href="https://cdn/photo.jpg">Download Photo</a></div></div>
            <div class="card"><div class="card-body"><a href="https://cdn/clip.mp4">Download Video</a></div></div>"#;
        let data = parser().parse_html(html).expect("success");
        assert_eq!(data.media.len(), 2);
        assert_eq!(data.media[0].url.as_deref(), Some("https://cdn/photo.jpg"));
        assert_eq!(data.media[0].r#type.as_deref(), Some("image"));
        assert_eq!(data.media[1].url.as_deref(), Some("https://cdn/clip.mp4"));
        assert_eq!(data.media[1].r#type.as_deref(), Some("video"));
    }

    #[test]
    fn parses_single_link_fallback() {
        let html = r#"
            <article class="media"><figure><img src="https://cdn/p.jpg"></figure></article>
            <a href="https://cdn/only.mp4">Download Video</a>"#;
        let data = parser().parse_html(html).expect("success");
        assert_eq!(data.media.len(), 1);
        assert_eq!(data.media[0].url.as_deref(), Some("https://cdn/only.mp4"));
        assert_eq!(data.media[0].r#type.as_deref(), Some("video"));
    }

    #[test]
    fn empty_html_is_blank_data() {
        assert!(matches!(
            parser().parse_html("<div>nothing useful</div>"),
            Err(Error::Blank)
        ));
    }

    #[test]
    fn media_serializes_camel_case_and_skips_none() {
        let media = SnapSaveDownloaderMedia {
            should_render: Some(true),
            r#type: Some("video".to_string()),
            url: Some("https://x/v".to_string()),
            ..Default::default()
        };
        let value = serde_json::to_value(&media).unwrap();
        assert_eq!(value["shouldRender"], serde_json::json!(true));
        assert_eq!(value["type"], serde_json::json!("video"));
        assert!(value.get("resolution").is_none());
        assert!(value.get("thumbnail").is_none());
    }

    #[test]
    fn data_serializes_to_media_json_and_round_trips() {
        let data = SnapSaveDownloaderData {
            media: vec![SnapSaveDownloaderMedia {
                url: Some("https://x/v".to_string()),
                r#type: Some("video".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let value = serde_json::to_value(&data).unwrap();
        assert!(value.get("media").is_some());
        assert!(value.get("description").is_none(), "None fields are skipped");

        let json = serde_json::to_string(&data).unwrap();
        let back: SnapSaveDownloaderData = serde_json::from_str(&json).unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn error_display_matches_platform() {
        assert_eq!(Error::InvalidUrl.to_string(), "invalid URL");
        assert_eq!(
            Error::Unsupported(Platform::Tiktok).to_string(),
            "TikTok is not supported yet"
        );
        assert_eq!(
            Error::Unsupported(Platform::Twitter).to_string(),
            "Twitter (X) is not supported yet"
        );
    }

    #[tokio::test]
    async fn invalid_url_short_circuits() {
        let result = parser().download("https://www.example.com/invalid", None).await;
        assert!(matches!(result, Err(Error::InvalidUrl)));
    }

    #[tokio::test]
    async fn unsupported_platforms_are_reported() {
        let snap = parser();

        let tiktok = snap
            .download("https://www.tiktok.com/@u/video/1234567890123456789", None)
            .await;
        assert!(matches!(tiktok, Err(Error::Unsupported(Platform::Tiktok))));

        let twitter = snap.download("https://x.com/u/status/1234567890123456789", None).await;
        assert!(matches!(twitter, Err(Error::Unsupported(Platform::Twitter))));
    }
}
