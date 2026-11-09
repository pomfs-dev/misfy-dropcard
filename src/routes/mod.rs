use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{middleware, Router};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::error::AppError;
use crate::{session, webhooks, AppState};

pub mod bookings;
pub mod config;
pub mod console;
pub mod demo;
pub mod venues;

pub fn router(state: AppState) -> Router {
    let web_dir = state.cfg.web_dir.clone();
    let index = web_dir.join("index.html");

    let api = Router::new()
        .route("/config", get(config::config))
        .route("/session", get(demo::session))
        .route("/demo/clock/advance", post(demo::advance_clock))
        .route("/demo/clock/reset", post(demo::reset_clock))
        .route("/demo/reset", post(demo::reset_session))
        .route("/venues", get(venues::list))
        .route("/venues/{slug}", get(venues::card))
        .route("/venues/{slug}/schedule", get(venues::schedule))
        .route("/bookings/{id}", get(bookings::get_booking))
        .route("/bookings/{id}/request", post(bookings::request))
        .route("/bookings/{id}/cancel", post(bookings::cancel))
        .route("/bookings/{id}/deposit-order", post(bookings::deposit_order))
        .route("/shows/{id}/ticket-order", post(bookings::ticket_order))
        .route("/paypal/orders/{paypal_order_id}/capture", post(bookings::capture))
        .route("/console/bookings/{id}", patch(console::patch))
        .route("/console/bookings/{id}/approve", post(console::approve))
        .route("/console/bookings/{id}/decline", post(console::decline))
        .route("/console/{slug}/inbox", get(console::inbox))
        .route("/console/{slug}/payments", get(console::payments))
        .route("/console/{slug}/payouts", get(console::payouts_list))
        .route("/console/{slug}/settlement/run", post(console::run_settlement))
        .route("/console/{slug}/agent-runs", get(console::agent_runs))
        .route("/console/{slug}/dashboard", get(console::dashboard))
        // Everything above runs inside a visitor's demo session.
        .route_layer(middleware::from_fn_with_state(state.clone(), session::layer))
        // PayPal calls this one directly: no session cookie.
        .route("/paypal/webhook", post(webhooks::receive))
        .fallback(|| async { AppError::NotFound });

    Router::new()
        .route("/healthz", get(health))
        .nest("/api", api)
        // The frontend is a single-page app: unknown paths get index.html.
        .fallback_service(ServeDir::new(web_dir).not_found_service(ServeFile::new(index)))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health(State(st): State<AppState>) -> (StatusCode, &'static str) {
    match sqlx::query("SELECT 1").execute(&st.db).await {
        Ok(_) => (StatusCode::OK, "ok"),
        Err(e) => {
            tracing::error!(error = %e, "health check failed");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable")
        }
    }
}
