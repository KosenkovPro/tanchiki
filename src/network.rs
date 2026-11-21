//! Сетевой слой: WebSocket-подключения, приём ввода и рассылка снимков мира.
//!
//! Протокол работы:
//!
//! 1. Клиент подключается к `/ws`.
//! 2. Сервер создаёт игрока и сразу отправляет `Welcome` с полной картой (RLE).
//! 3. Клиент присылает `Join` с никнеймом — игрок вступает в бой и получает танка.
//! 4. Каждые 33 мс клиент присылает `Input`, сервер отвечает снимком мира.
//!
//! Обработка любых ошибок: битый пакет, обрыв соединения, падение писателя —
//! ни один из этих случаев не должен ронять игровой цикл.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info};

use crate::game::Game;
use crate::protocol::{self, ClientMessage};

/// Размер очереди исходящих сообщений на соединение.
const OUT_QUEUE_SIZE: usize = 32;

/// Счётчик идентификаторов соединений.
static CONN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Состояние приложения, разделяемое между обработчиками.
#[derive(Clone)]
pub struct AppState {
    /// Авторитарное состояние мира.
    pub game: Arc<Mutex<Game>>,
}

/// Обработчик WebSocket-апгрейда.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Основной цикл: тикает мир и рассылает каждому игроку только видимую ему часть.
///
/// Тикрейт не зависит от точности планировщика ОС: игра догоняет накопившиеся
/// тики по накопленному времени, а мелкий интервал опроса даёт точность
/// пробуждения около миллисекунды.
pub async fn game_loop(state: AppState) {
    let (tick_duration, profile) = {
        let game = state.game.lock().await;
        (game.cfg.tick_duration(), game.cfg.profile)
    };

    enable_high_resolution_timer();

    let mut ticker = tokio::time::interval(Duration::from_millis(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut accumulator = Duration::ZERO;
    let mut last = Instant::now();

    loop {
        ticker.tick().await;

        let now = Instant::now();
        accumulator += now.duration_since(last);
        last = now;

        let mut steps = 0u32;
        while accumulator >= tick_duration && steps < MAX_CATCHUP_TICKS {
            accumulator -= tick_duration;
            steps += 1;
            run_tick(&state, profile).await;
        }

        // Долг накопился из-за долгой остановки — начинаем отсчёт заново,
        // иначе сервер пытался бы «догнать» сотни тиков подряд.
        if accumulator > tick_duration * MAX_CATCHUP_TICKS {
            accumulator = Duration::ZERO;
        }
    }
}

/// Один тик мира: симуляция плюс рассылка снимков.
async fn run_tick(state: &AppState, profile: bool) {
    let started = Instant::now();
    let mut game = state.game.lock().await;
    game.tick();
    let t_sim = started.elapsed();

    let ids = game.player_ids();
    let mut frames: Vec<(u32, Vec<u8>)> = Vec::with_capacity(ids.len());
    for id in &ids {
        if let Some(snapshot) = game.snapshot_for(*id) {
            frames.push((*id, protocol::encode_snapshot(&snapshot)));
        }
    }
    let t_snapshots = started.elapsed();

    let send_leaderboard = game.tick % 10 == 0;
    let tick = game.tick;
    let leaderboards: Vec<(u32, Vec<u8>)> = if send_leaderboard {
        ids.iter()
            .map(|id| {
                let entries = game.leaderboard_for(*id);
                (*id, protocol::encode_leaderboard(&entries))
            })
            .collect()
    } else {
        Vec::new()
    };

    for (id, bytes) in frames {
        game.send_to(id, bytes);
    }
    for (id, bytes) in leaderboards {
        game.send_to(id, bytes);
    }
    drop(game);

    if profile && tick % 60 == 0 {
        tracing::info!(
            players = ids.len(),
            sim = ?t_sim,
            snapshots = ?t_snapshots,
            total = ?started.elapsed(),
            "профиль сети"
        );
    }
}

// ---------------------------------------------------------------------------
// Точность системного таймера
// ---------------------------------------------------------------------------

/// Максимум тиков, догоняемых за один цикл.
const MAX_CATCHUP_TICKS: u32 = 5;

#[cfg(windows)]
#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}

/// По умолчанию Windows округляет любое ожидание до границы ~15.6 мс,
/// из-за чего `interval` на 33 мс реально срабатывает раз в ~46 мс, и игра
/// теряет треть тиков. Просим систему использовать точность 1 мс.
#[cfg(windows)]
fn enable_high_resolution_timer() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: вызов идемпотентен и безопасен в любой момент жизни процесса.
        let result = unsafe { timeBeginPeriod(1) };
        if result != 0 {
            tracing::debug!("системный таймер переведён на точность 1 мс");
        }
    });
}

/// Возвращает системному таймеру прежнюю точность при остановке сервера.
#[cfg(windows)]
pub fn restore_timer_resolution() {
    // SAFETY: вызов идемпотентен.
    unsafe {
        timeEndPeriod(1);
    }
}

#[cfg(not(windows))]
fn enable_high_resolution_timer() {}

#[cfg(not(windows))]
pub fn restore_timer_resolution() {}

/// Обслуживает одно WebSocket-соединение.
async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(OUT_QUEUE_SIZE);
    let conn_id = CONN_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Писатель: единственный, кто трогает исходящий сокет.
    let writer = tokio::spawn(async move {
        while let Some(bytes) = out_rx.recv().await {
            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // Регистрируем игрока и сразу отдаём карту — клиент может рисовать фон
    // ещё до нажатия «В бой».
    let player_id = {
        let mut game = state.game.lock().await;
        let id = game.add_player(conn_id, String::new(), out_tx.clone());
        debug!(conn_id, player_id = id, "игрок подключился");
        id
    };

    let mut joined = false;

    while let Some(message) = stream.next().await {
        let Ok(message) = message else {
            break;
        };
        match message {
            Message::Binary(bytes) => {
                match protocol::decode_client(&bytes) {
                    Ok(ClientMessage::Join { nickname }) => {
                        if joined {
                            continue;
                        }
                        joined = true;
                        let clean = protocol::sanitize_nickname(&nickname);
                        let mut game = state.game.lock().await;
                        game.join_player(player_id, clean);
                    }
                    Ok(ClientMessage::Input {
                        seq,
                        input,
                        aim,
                        ability,
                        target_x,
                        target_y,
                    }) => {
                        let mut game = state.game.lock().await;
                        game.apply_input(
                            conn_id,
                            seq,
                            input,
                            aim,
                            ability,
                            target_x,
                            target_y,
                        );
                    }
                    Ok(ClientMessage::Respawn) => {
                        let mut game = state.game.lock().await;
                        game.respawn_player(player_id);
                    }
                    Ok(ClientMessage::Ping { client_time }) => {
                        let server_time = now_millis();
                        let pong = protocol::encode_pong(client_time, server_time);
                        let _ = out_tx.try_send(pong);
                    }
                    Err(err) => {
                        // Битый пакет: сообщаем клиенту, но соединение не рвём.
                        let err_msg = protocol::encode_error(1, &err.to_string());
                        let _ = out_tx.try_send(err_msg);
                    }
                }
            }
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => {}
            _ => {}
        }
    }

    // Сначала убираем игрока — это отпускает его копию отправки. Затем
    // роняем свою копию, после чего писатель сам закроет сокет корректным
    // close-кадром (а не обрывом).
    {
        let mut game = state.game.lock().await;
        game.remove_player(conn_id);
    }
    drop(out_tx);

    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;

    info!(conn_id, player_id, "игрок отключился");
}

/// Текущее время в миллисекундах с эпохи Unix.
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
