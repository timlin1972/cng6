//! `GET /api/worldclock`: 本地時區與城市清單(時間由瀏覽器自己算)

use axum::Json;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::plugins::worldclock::{self, Shared, Zone};

#[derive(Serialize)]
struct Info {
    local: String,
    zones: Vec<Zone>,
}

pub async fn info(State(shared): State<Shared>) -> Response {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(Info {
            local: worldclock::local_tz().name().to_string(),
            zones: shared.zones(),
        }),
    )
        .into_response()
}
