//! `GET /api/weather`: 所有城市的天氣(目前、逐小時、7 天、警示)

use axum::Json;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::plugins::weather::{City, Shared};

#[derive(Serialize)]
struct Info {
    version: u64,
    cities: Vec<City>,
}

pub async fn info(State(shared): State<Shared>) -> Response {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(Info {
            version: shared.version(),
            cities: shared.cities(),
        }),
    )
        .into_response()
}
