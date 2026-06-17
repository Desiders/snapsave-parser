//! Data shapes returned by [`SnapSave::download`](crate::SnapSave::download) and
//! the crate error type.
//!
//! The JSON produced by `serde` matches the `snapsave.app` result shape
//! (e.g. the media object key is `shouldRender`, and `type` is emitted
//! verbatim). `None` fields are omitted from the output.

use serde::{Deserialize, Serialize};

/// A single downloadable media entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapSaveDownloaderMedia {
    /// Resolution label (Facebook video table rows), e.g. `"720p (HD)"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    /// `true` when the URL must be rendered server-side before download.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub should_render: Option<bool>,
    /// Thumbnail/preview image for a video item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    /// `"image"` or `"video"`.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    /// Direct media URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// The data extracted from a successful download. Always carries at least one
/// media entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapSaveDownloaderData {
    /// Caption/description (Facebook, `TikTok`, Twitter).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Preview image URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Extracted media entries (non-empty on success).
    pub media: Vec<SnapSaveDownloaderMedia>,
}

/// Optional knobs for [`SnapSave::download`](crate::SnapSave::download).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapSaveDownloaderOptions {
    /// Number of retries on a failed request. Defaults to `1`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<u32>,
    /// Delay between retries, in milliseconds. Defaults to `500`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_delay: Option<u64>,
    /// Override the `User-Agent` header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
}

/// Everything that can make a download fail.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A network/HTTP transport failure.
    #[error(transparent)]
    Request(#[from] reqwest::Error),
    /// The response could not be decrypted or parsed (includes an inline
    /// `#alert` message surfaced by `snapsave.app`).
    #[error(transparent)]
    Decrypt(#[from] crate::decrypter::DecryptError),
    /// A built-in URL regex failed to compile.
    #[error(transparent)]
    Regex(#[from] regex::Error),
    /// A built-in CSS selector failed to compile.
    #[error(transparent)]
    Selector(#[from] scraper::error::SelectorErrorKind<'static>),
    /// The URL matched no supported platform.
    #[error("invalid URL")]
    InvalidUrl,
    /// The URL is for a platform whose extraction flow is not implemented yet.
    #[error("{0} is not supported yet")]
    Unsupported(crate::Platform),
    /// The response decoded successfully but contained no media.
    #[error("no media found in response")]
    Blank,
}
