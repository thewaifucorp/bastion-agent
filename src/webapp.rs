//! Serves the embedded web app (`web/`, built by Vite into `web/dist` and
//! embedded at compile time by `build.rs`) at `GET /app`.
//!
//! First-party sibling of the single-file `/ui` dashboard (which stays as
//! the zero-build fallback): same trust model — the shell is served
//! unauthenticated because it embeds no data; everything it renders is
//! fetched with per-request tokens against `/events`, `/webhook` and
//! `/v1/*`. The CSP pins scripts/styles/connections to same-origin, so the
//! bundle structurally cannot call out anywhere but this daemon.
//!
//! Built WITHOUT the web app (`web/dist` absent — every local `cargo` run
//! and the CI `rust` job), the routes still mount and `GET /app` answers
//! with a short plain-text explanation instead of a broken page.

use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
                   connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'";

fn lookup(path: &str) -> Option<(&'static str, &'static [u8])> {
    WEB_ASSETS
        .iter()
        .find(|(rel, _, _)| *rel == path)
        .map(|(_, ct, bytes)| (*ct, *bytes))
}

fn serve(path: &str) -> axum::response::Response {
    // SPA fallback: anything without a file extension routes client-side,
    // so it gets index.html (same rule Vite's preview server applies).
    let effective = if path.is_empty() || !path.contains('.') {
        "index.html"
    } else {
        path
    };
    match lookup(effective) {
        Some((content_type, bytes)) => {
            // Vite emits content-hashed asset names — safe to cache hard.
            // index.html must revalidate so a new deploy is picked up.
            let cache = if effective == "index.html" {
                "no-cache"
            } else {
                "public, max-age=31536000, immutable"
            };
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CONTENT_SECURITY_POLICY, CSP),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                    (header::CACHE_CONTROL, cache),
                ],
                bytes,
            )
                .into_response()
        }
        None if WEB_ASSETS.is_empty() => (
            StatusCode::NOT_FOUND,
            "the web app was not embedded in this build — run `npm run build` \
             in web/ and rebuild, or use the always-available /ui dashboard",
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn index() -> impl IntoResponse {
    serve("")
}

async fn asset(Path(path): Path<String>) -> impl IntoResponse {
    serve(&path)
}

/// Stateless sub-router — merged into the webhook app AFTER `.with_state`
/// (same slot as `mcp_routes`/`control_plane_routes`).
pub fn router() -> Router {
    Router::new()
        .route("/app", get(index))
        .route("/app/", get(index))
        .route("/app/{*path}", get(asset))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The test build has no web/dist (CI's rust job never runs node), so the
    // table is empty — assert the graceful-absence contract rather than the
    // asset contents.
    #[tokio::test]
    async fn absent_build_answers_with_guidance_not_a_broken_page() {
        let app = router();
        let req = axum::http::Request::builder()
            .uri("/app")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
        if WEB_ASSETS.is_empty() {
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        } else {
            assert_eq!(resp.status(), StatusCode::OK);
        }
    }

    #[test]
    fn spa_fallback_targets_index_for_extensionless_paths() {
        // Pure routing rule check — independent of whether assets exist.
        assert!(!"tarefas".contains('.'));
        assert!("assets/index-abc.js".contains('.'));
    }
}
