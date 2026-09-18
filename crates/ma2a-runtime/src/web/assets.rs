use axum::{
    http::{StatusCode, header},
    response::{IntoResponse as _, Response},
};

/// Immutable embedded frontend assets.
#[derive(Clone, Copy, Debug)]
pub struct WebAssets {
    files: &'static [(&'static str, &'static [u8])],
}

impl WebAssets {
    /// Creates an embedded asset collection.
    #[must_use]
    pub const fn new(files: &'static [(&'static str, &'static [u8])]) -> Self {
        Self { files }
    }
}

pub(super) fn asset_response(assets: &WebAssets, requested: &str) -> Response {
    let requested_asset = assets.files.iter().find(|(name, _)| *name == requested);
    let fallback = !requested.contains('.') && !requested.starts_with("assets/");
    let Some((name, bytes)) = requested_asset.or_else(|| {
        fallback
            .then(|| assets.files.iter().find(|(name, _)| *name == "index.html"))
            .flatten()
    }) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let extension = std::path::Path::new(name)
        .extension()
        .and_then(std::ffi::OsStr::to_str);
    let content_type = match extension {
        Some(value) if value.eq_ignore_ascii_case("html") => "text/html; charset=utf-8",
        Some(value) if value.eq_ignore_ascii_case("js") => "text/javascript; charset=utf-8",
        Some(value) if value.eq_ignore_ascii_case("css") => "text/css; charset=utf-8",
        Some(value) if value.eq_ignore_ascii_case("svg") => "image/svg+xml",
        Some(value) if value.eq_ignore_ascii_case("woff2") => "font/woff2",
        Some(_) | None => "application/octet-stream",
    };
    let cache_control = if *name == "index.html" {
        "no-cache"
    } else if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache_control),
        ],
        bytes.to_vec(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::header};

    use super::{WebAssets, asset_response};

    static FILES: &[(&str, &[u8])] = &[
        ("index.html", b"<main>console</main>"),
        ("assets/app-a1b2c3.js", b"export{}"),
        ("assets/mono-d4e5f6.woff2", b"wOF2"),
    ];

    #[test]
    fn serves_self_hosted_fonts_with_a_font_media_type() {
        let response = asset_response(&WebAssets::new(FILES), "assets/mono-d4e5f6.woff2");

        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static("font/woff2"))
        );
    }

    #[tokio::test]
    async fn falls_back_to_index_for_a_deep_spa_route() {
        let response = asset_response(&WebAssets::new(FILES), "spaces/operations");

        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&header::HeaderValue::from_static("no-cache"))
        );
        let body = to_bytes(response.into_body(), 1_024).await.expect("body");
        assert_eq!(&body[..], b"<main>console</main>");
    }

    #[test]
    fn serves_hashed_assets_with_immutable_caching() {
        let response = asset_response(&WebAssets::new(FILES), "assets/app-a1b2c3.js");

        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&header::HeaderValue::from_static(
                "public, max-age=31536000, immutable"
            ))
        );
    }

    #[test]
    fn does_not_fall_back_for_a_missing_asset_path() {
        let response = asset_response(&WebAssets::new(FILES), "assets/missing.js");

        assert_eq!(response.status(), 404);
    }
}
