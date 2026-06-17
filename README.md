# snapsave-parser

Extract direct media download URLs from **Instagram** and **Facebook** by querying
[`snapsave.app`](https://snapsave.app). An async Rust library that takes a post/reel
URL and returns the underlying video/image URLs (plus thumbnails and, for Facebook,
per-resolution variants).

`snapsave.app` answers with an obfuscated, packed JavaScript payload rather than plain
HTML; this crate unpacks that payload and parses the resulting markup for you.

## Features

- Instagram posts, reels, IGTV, and stories
- Facebook videos, reels, posts, and `share` links
- HTTP / HTTPS / SOCKS5 proxy support
- Configurable retries and `User-Agent`
- `serde`-serializable result types (JSON matches the `snapsave.app` shape)

## Installation

```toml
[dependencies]
snapsave-parser = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

The API is async and runtime-agnostic; the examples use Tokio.

## Usage

Construct a `SnapSave` once and reuse it — it owns the HTTP client and compiled
patterns, so repeated `download` calls reuse the connection pool:

```rust
use snapsave_parser::SnapSave;

#[tokio::main]
async fn main() -> Result<(), snapsave_parser::Error> {
    let snap = SnapSave::new()?;
    match snap.download("https://www.instagram.com/p/C51YHfWJwHK/", None).await {
        Ok(data) => {
            for media in data.media {
                println!("{:?} -> {}", media.r#type, media.url.unwrap_or_default());
            }
        }
        Err(error) => eprintln!("failed: {error}"),
    }
    Ok(())
}
```

### Options

```rust
use snapsave_parser::SnapSaveDownloaderOptions;

let options = SnapSaveDownloaderOptions {
    retry: Some(3),          // extra attempts on failure (default: 1)
    retry_delay: Some(500),  // ms between attempts (default: 500)
    user_agent: None,        // override the default UA (per request)
};
// snap.download("https://www.facebook.com/watch?v=1234567890123456", Some(options)).await;
```

### Proxy

The proxy is a client-level setting, so it's chosen at construction (`http://`,
`https://`, or `socks5://`):

```rust
use snapsave_parser::SnapSave;

let snap = SnapSave::with_proxy("socks5://127.0.0.1:1080")?;
```

## Result shape

`download` returns `Result<SnapSaveDownloaderData, Error>`. On success the data
serializes with `serde` to the `snapsave.app` media shape:

```json
{
  "media": [
    {
      "url": "https://d.rapidcdn.app/v2?token=…",
      "thumbnail": "https://d.rapidcdn.app/thumb?token=…",
      "type": "video"
    }
  ]
}
```

A Facebook success additionally carries `description`, `preview`, and per-row
`resolution` (with `shouldRender: true` for variants that must be rendered server-side
before download).

The types involved:

| Type | Shape |
| --- | --- |
| `SnapSaveDownloaderData` | `description: Option`, `preview: Option`, `media: Vec<…Media>` |
| `SnapSaveDownloaderMedia` | `url`, `type` (`"image"`/`"video"`), `thumbnail`, `resolution`, `should_render` |
| `Error` | `Request` / `Decrypt` / `Regex` / `Selector` (wrapped source errors), `InvalidUrl`, `Unsupported(Platform)`, `Blank` |

## Supported platforms

| Platform | Status |
| --- | --- |
| Instagram | ✅ supported |
| Facebook | ✅ supported |
| TikTok | recognized, returns `Err(Error::Unsupported(Platform::Tiktok))` |
| Twitter / X | recognized, returns `Err(Error::Unsupported(Platform::Twitter))` |

Unrecognized URLs return `Err(Error::InvalidUrl)`. The `Platform` dispatch is
structured so additional flows can be added without changing the core.

## Lower-level API

The pipeline stages are also exposed for advanced use (e.g. caching raw responses):

- `decrypt_snap_save(raw: &str) -> Result<String, DecryptError>` — unpack a raw response into HTML.
- `SnapSave::parse_html(&self, html) -> Result<SnapSaveDownloaderData, Error>` — parse decoded HTML.
- `SnapSave::detect(&self, url)` / `SnapSave::normalize_url(&self, url)` — URL helpers.
- `fix_thumbnail`, `USER_AGENT` — request helpers.

## Development

```sh
just fmt              # cargo +nightly fmt --all
just lint             # cargo clippy --all-features -- -W clippy::pedantic
just test             # cargo test (offline unit tests)
just test-integration # cargo test -- --ignored (hits snapsave.app)
```

The default test suite is fully offline: URL parsing, normalization, and the full
decrypt/parse pipeline are covered with synthetic data. Live integration tests in
`tests/live.rs` are `#[ignore]`d and run only via `just test-live`.

## License

MIT
