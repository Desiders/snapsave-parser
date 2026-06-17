//! Live integration tests that hit `snapsave.app`.
//!
//! Ignored by default so `cargo test` stays offline and deterministic. Run them
//! explicitly with `cargo test -- --ignored` (or `just test-live`). They depend
//! on the remote service and a public post staying available, so occasional
//! failures are expected.

use snapsave_parser::{Error, SnapSave};

#[tokio::test]
#[ignore = "hits snapsave.app over the network"]
async fn downloads_instagram_reel() {
    let snap = SnapSave::new().expect("build parser");
    let data = snap
        .download("https://www.instagram.com/reel/CtjoC2BNsB2", None)
        .await
        .expect("download should succeed");

    assert!(!data.media.is_empty(), "expected at least one media entry");
    assert!(data.media[0].url.is_some(), "media entry should have a URL");
}

#[tokio::test]
#[ignore = "hits snapsave.app over the network"]
async fn rejects_invalid_url_without_network() {
    let snap = SnapSave::new().expect("build parser");
    let result = snap.download("https://www.example.com/not-a-post", None).await;
    assert!(matches!(result, Err(Error::InvalidUrl)));
}
