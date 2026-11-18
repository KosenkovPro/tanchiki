//! Точка входа: HTTP + WebSocket сервер «Танчики».
//!
//! Весь проект запускается одной командой:
//!
//! ```text
//! cargo run
//! ```
//!
//! Статика вшивается в бинарник через `rust-embed`, поэтому релизная сборка
//! не требует отдельной папки с файлами.

mod game;
mod map;
mod network;
mod protocol;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::Router;
use rust_embed::RustEmbed;
use tokio::sync::Mutex;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::game::{Game, GameConfig};
use crate::network::AppState;

/// Встроенные статические файлы клиента.
#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

/// Отдаёт встроенный файл клиента.
async fn serve_static(uri: Uri) -> Response {
    let mut path = uri.path().trim_start_matches('/').to_string();
    if path.is_empty() {
        path = "index.html".to_string();
    }
    // Запрещаем выход за пределы каталога.
    if path.contains("..") {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    }

    match Assets::get(path.as_str()) {
        Some(file) => {
            let mime = file.metadata.mimetype().to_string();
            (
                [(header::CONTENT_TYPE, mime)],
                file.data.into_owned(),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Проверка живости сервера.
async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let cfg = GameConfig::from_env();
    let bind: SocketAddr = std::env::var("TANCHIKI_BIND")
        .unwrap_or_else(|_| "0.0.0.0:3000".to_string())
        .parse()?;

    info!(
        tick_rate = cfg.tick_rate,
        map = format!("{}x{}", cfg.map_w, cfg.map_h),
        tile_size = cfg.tile_size,
        min_population = cfg.min_population,
        max_bots = cfg.max_bots,
        "запуск сервера «Танчики»"
    );

    let game = Game::new(cfg);
    let state = AppState {
        game: Arc::new(Mutex::new(game)),
    };

    // Игровой цикл живёт отдельной задачей и тикает с фиксированной частотой.
    tokio::spawn(network::game_loop(state.clone()));

    let app = Router::new()
        .route("/ws", any(network::ws_handler))
        .route("/healthz", get(healthz))
        .fallback(serve_static)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!("сервер слушает на http://{bind}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    network::restore_timer_resolution();
    info!("сервер остановлен");
    Ok(())
}

/// Завершает сервер по Ctrl+C.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("получен сигнал остановки");
}
