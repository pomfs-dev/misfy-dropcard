//! Demo sessions. Every visitor gets a cookie with a random session id; their bookings and their
//! demo clock belong to that session, so several judges can try the demo at the same time
//! without seeing each other's bookings.

use axum::extract::{Request, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::error::AppError;
use crate::AppState;

pub const COOKIE_NAME: &str = "dc_sid";
const MAX_AGE_SECS: u64 = 60 * 60 * 24 * 45;

/// The current visitor's demo session id (inserted into request extensions by [`layer`]).
#[derive(Clone, Copy, Debug)]
pub struct DemoSession(pub Uuid);

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

fn set_cookie(id: Uuid, secure: bool) -> HeaderValue {
    let mut value = format!("{COOKIE_NAME}={id}; Path=/; Max-Age={MAX_AGE_SECS}; HttpOnly; SameSite=Lax");
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).expect("cookie header is ASCII")
}

/// Middleware for `/api`: makes sure the visitor has a session row and a cookie.
pub async fn layer(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let existing = cookie_value(req.headers(), COOKIE_NAME).and_then(|v| Uuid::parse_str(&v).ok());
    let (id, is_new) = match existing {
        Some(id) => (id, false),
        None => (Uuid::new_v4(), true),
    };

    // Also recreates the row if the database was reset while the browser kept its cookie.
    let ensured = sqlx::query(
        "INSERT INTO demo_sessions (id) VALUES ($1)
         ON CONFLICT (id) DO UPDATE SET last_seen_at = now()
         WHERE demo_sessions.last_seen_at < now() - interval '5 minutes'",
    )
    .bind(id)
    .execute(&state.db)
    .await;
    if let Err(e) = ensured {
        return AppError::from(e).into_response();
    }

    req.extensions_mut().insert(DemoSession(id));
    let mut res = next.run(req).await;
    if is_new {
        res.headers_mut().append(SET_COOKIE, set_cookie(id, state.cfg.cookie_secure));
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cookie_among_others() {
        let mut h = HeaderMap::new();
        h.insert(COOKIE, HeaderValue::from_static("a=1; dc_sid=abc; b=2"));
        assert_eq!(cookie_value(&h, COOKIE_NAME).as_deref(), Some("abc"));
        assert_eq!(cookie_value(&h, "missing"), None);
    }
}
