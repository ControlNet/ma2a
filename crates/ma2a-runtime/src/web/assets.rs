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
    let Some((_, bytes)) = assets.files.iter().find(|(name, _)| *name == requested) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let extension = std::path::Path::new(requested)
        .extension()
        .and_then(std::ffi::OsStr::to_str);
    let content_type = match extension {
        Some(value) if value.eq_ignore_ascii_case("html") => "text/html; charset=utf-8",
        Some(value) if value.eq_ignore_ascii_case("js") => "text/javascript; charset=utf-8",
        Some(value) if value.eq_ignore_ascii_case("css") => "text/css; charset=utf-8",
        Some(value) if value.eq_ignore_ascii_case("svg") => "image/svg+xml",
        Some(_) | None => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, content_type)], bytes.to_vec()).into_response()
}
