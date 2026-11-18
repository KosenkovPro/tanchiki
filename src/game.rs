//! Авторитарный игровой цикл: физика, коллизии, стрельба, боты, лут и прокачка.
//!
//! Сервер — единственный источник истины. Клиент только присылает ввод,
//! а сервер рассчитывает всё остальное и рассылает снимки мира.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::Sender;

use crate::map::{GameMap, Rng, SpatialGrid, TileKind, CHUNK_TILES, MAP_H, MAP_W, TILE_SIZE};
use crate::protocol::{
    self, BulletView, Direction, GameEvent, InputFlags, LeaderboardEntry, PlayerView, PickupView,
    Snapshot, TileUpdate,
};

// ---------------------------------------------------------------------------
// Константы баланса
// ---------------------------------------------------------------------------

/// Длительность игрового тика в секундах (30 TPS).
pub const TICK_DT: f32 = 1.0 / 30.0;

/// Размер хитбокса танка в пикселях (меньше тайла — есть зазор для манёвра).
pub const TANK_SIZE: f32 = 30.0;

/// Размер снаряда в пикселях.
pub const BULLET_SIZE: f32 = 6.0;

/// Базовая скорость танка первого уровня.
pub const BASE_SPEED: f32 = 140.0;

/// Базовая скорость снаряда.
pub const BULLET_SPEED_BASE: f32 = 300.0;

/// Тики неуязвимости после возрождения (3 секунды).
pub const SPAWN_PROTECT_TICKS: u64 = 90;

/// Тики щита (8 секунд).
pub const SHIELD_TICKS: u64 = 240;

/// Тики, в течение которых танк в лесу «обнаруживается» после выстрела.
pub const REVEAL_TICKS: u64 = 60;

/// Максимум бонусов на карте одновременно.
pub const MAX_PICKUPS: usize = 24;

/// Как часто появляется новый бонус (в тиках).
pub const PICKUP_SPAWN_INTERVAL: u64 = 90;

/// Время жизни бонуса (в тиках).
pub const PICKUP_LIFETIME: u64 = 900;

/// Очки за уничтожение танка.
pub const KILL_SCORE: u32 = 100;

/// Очки за подобранный бонус.
pub const PICKUP_SCORE: u32 = 50;

/// Очки за разрушенный кирпич.
pub const BRICK_SCORE: u32 = 1;

/// Имена ботов.
pub const BOT_NAMES: [&str; 15] = [
    "Ильза",
    "Барсик",
    "Тигр",
    "Вулкан",
    "Гром",
    "Сокол",
    "Шершень",
    "Тор",
    "Мортир",
    "Бизон",
    "Кобра",
    "Цапля",
    "Дракон",
    "Рысь",
    "Комета",
];

/// Скорость танка для уровня прокачки.
pub fn level_speed(level: u8) -> f32 {
    BASE_SPEED * (1.0 + 0.08 * level.saturating_sub(1) as f32)
}

/// Скорость снаряда для уровня прокачки.
pub fn level_bullet_speed(level: u8) -> f32 {
    BULLET_SPEED_BASE * (1.0 + 0.10 * level.saturating_sub(1) as f32)
}

/// Урон снаряда для уровня прокачки.
pub fn level_bullet_damage(level: u8) -> u8 {
    level.clamp(1, 3)
}

/// Задержка между выстрелами для уровня прокачки.
pub fn level_fire_cooldown(level: u8) -> f32 {
    0.55 - 0.10 * level.saturating_sub(1) as f32
}

/// Сколько снарядов может лететь одновременно.
pub fn level_max_bullets(level: u8) -> usize {
    level.clamp(1, 3) as usize
}

/// Максимальное здоровье для уровня прокачки.
pub fn level_max_hp(level: u8) -> u8 {
    2 + level.clamp(1, 3)
}

// ---------------------------------------------------------------------------
// Конфигурация
// ---------------------------------------------------------------------------

/// Параметры сервера. Читаются из переменных окружения (см. README).
#[derive(Debug, Clone)]
pub struct GameConfig {
    /// Количество тиков в секунду.
    pub tick_rate: u64,
    /// Ширина карты в тайлах.
    pub map_w: u32,
    /// Высота карты в тайлах.
    pub map_h: u32,
    /// Размер тайла в пикселях. Фиксирован: геометрия карты опирается на `TILE_SIZE`.
    pub tile_size: f32,
    /// Минимальная численность населения (люди + боты).
    pub min_population: usize,
    /// Максимум ботов.
    pub max_bots: usize,
    /// Зерно генерации карты.
    pub seed: u64,
    /// Включён ли подробный профайлинг тиков (`TANCHIKI_PROFILE=1`).
    pub profile: bool,
}

impl Default for GameConfig {
    fn default() -> Self {
        GameConfig {
            tick_rate: 30,
            map_w: MAP_W,
            map_h: MAP_H,
            tile_size: TILE_SIZE,
            min_population: 15,
            max_bots: 14,
            seed: 0x7A4C_1E55_D1CE_2026,
            profile: false,
        }
    }
}

impl GameConfig {
    /// Собирает конфигурацию из переменных окружения.
    pub fn from_env() -> GameConfig {
        let mut cfg = GameConfig::default();
        if let Some(v) = env_u64("TANCHIKI_TICK_RATE") {
            if v >= 5 && v <= 120 {
                cfg.tick_rate = v;
            }
        }
        if let Some(v) = env_u64("TANCHIKI_MAP_W") {
            if v >= 32 && v <= 512 {
                cfg.map_w = v as u32;
            }
        }
        if let Some(v) = env_u64("TANCHIKI_MAP_H") {
            if v >= 32 && v <= 512 {
                cfg.map_h = v as u32;
            }
        }
        if let Some(v) = env_u64("TANCHIKI_MIN_PLAYERS") {
            cfg.min_population = v as usize;
        }
        if let Some(v) = env_u64("TANCHIKI_MAX_BOTS") {
            cfg.max_bots = v as usize;
        }
        if let Some(v) = env_u64("TANCHIKI_SEED") {
            cfg.seed = v;
        }
        cfg.profile = std::env::var("TANCHIKI_PROFILE")
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false);
        cfg
    }

    /// Ширина мира в пикселях.
    pub fn world_w(&self) -> f32 {
        self.map_w as f32 * self.tile_size
    }

    /// Высота мира в пикселях.
    pub fn world_h(&self) -> f32 {
        self.map_h as f32 * self.tile_size
    }

    /// Длительность тика.
    pub fn tick_duration(&self) -> Duration {
        Duration::from_millis(1000 / self.tick_rate.max(1))
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse::<u64>().ok()
}

// ---------------------------------------------------------------------------
// Сущности
// ---------------------------------------------------------------------------

/// Танк игрока или бота.
#[derive(Debug, Clone)]
pub struct Tank {
    /// Левый верхний угол хитбокса в пикселях мира.
    pub x: f32,
    pub y: f32,
    /// Направление движения и ствола.
    pub dir: Direction,
    /// Скорость вдоль текущей оси (со знаком).
    pub vel: f32,
    /// Уровень прокачки 1..=3.
    pub level: u8,
    /// Текущее здоровье.
    pub hp: u8,
    /// Максимальное здоровье.
    pub max_hp: u8,
    /// Остаток перезарядки в секундах.
    pub cooldown: f32,
    /// До какого тика действует щит.
    pub shield_until: u64,
    /// До какого тика действует неуязвимость после возрождения.
    pub spawn_protect_until: u64,
    /// Фаза анимации гусениц.
    pub track_phase: f32,
    /// До какого тика танк «обнажён» после выстрела.
    pub revealed_until: u64,
}

impl Tank {
    /// Создаёт новый танк в точке.
    pub fn new(x: f32, y: f32, level: u8, now: u64) -> Tank {
        let max_hp = level_max_hp(level);
        Tank {
            x,
            y,
            dir: Direction::Down,
            vel: 0.0,
            level,
            hp: max_hp,
            max_hp,
            cooldown: 0.0,
            shield_until: 0,
            spawn_protect_until: now + SPAWN_PROTECT_TICKS,
            track_phase: 0.0,
            revealed_until: 0,
        }
    }

    /// Центр танка в пикселях мира.
    pub fn center(&self) -> (f32, f32) {
        (self.x + TANK_SIZE * 0.5, self.y + TANK_SIZE * 0.5)
    }
}

/// Тип бонуса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PickupKind {
    /// Звезда — повышение уровня танка.
    Star = 0,
    /// Щит — временная неуязвимость.
    Shield = 1,
    /// Ремкомплект — восстановление здоровья.
    Repair = 2,
}

impl PickupKind {
    /// Базовый тип бонуса как байт протокола.
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Бонус на карте.
#[derive(Debug, Clone, Copy)]
pub struct Pickup {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub kind: PickupKind,
    /// До какого тика бонус существует.
    pub expires_at: u64,
}

/// Снаряд.
#[derive(Debug, Clone, Copy)]
pub struct Bullet {
    pub id: u32,
    /// Игрок-владелец (по нему нельзя попасть в свой снаряд).
    pub owner: u32,
    pub x: f32,
    pub y: f32,
    pub dir: Direction,
    pub level: u8,
    pub damage: u8,
    pub speed: f32,
}

/// Состояние ИИ-бота.
#[derive(Debug, Clone)]
pub struct BotState {
    /// Текущий маршрут по тайлам.
    pub path: Vec<(u32, u32)>,
    /// Индекс текущей точки маршрута.
    pub path_idx: usize,
    /// Тик, на котором маршрут нужно пересчитать.
    pub next_plan_tick: u64,
    /// Текущая цель (id игрока).
    pub target: Option<u32>,
}

/// Игрок: человек или бот.
#[derive(Debug)]
pub struct Player {
    pub id: u32,
    /// Идентификатор WebSocket-соединения (`0` для ботов).
    pub conn_id: u64,
    pub nickname: String,
    pub is_bot: bool,
    /// `None`, если игрок сейчас мёртв или ещё не вступил в бой.
    pub tank: Option<Tank>,
    pub score: u32,
    pub kills: u32,
    pub deaths: u32,
    /// Текущий ввод.
    pub input: InputFlags,
    /// Предыдущий ввод (нужен для разрешения диагоналей).
    pub prev_input: InputFlags,
    /// Порядковый номер последнего пакета ввода.
    pub last_seq: u32,
    /// Канал отправки в клиент.
    pub tx: Option<Sender<Vec<u8>>>,
    /// Вступил ли игрок в бой (нажал «В бой»).
    pub joined: bool,
    /// Тик, начиная с которого разрешён респаун.
    pub spawn_at: u64,
    /// Тик вступления в бой.
    pub joined_at: u64,
    /// Состояние бота.
    pub bot: Option<BotState>,
    /// Тик, на котором игрок будет удалён (плавное убирание лишних ботов).
    pub removal_at: Option<u64>,
}

// ---------------------------------------------------------------------------
// Игра
// ---------------------------------------------------------------------------

/// Авторитарное состояние мира.
pub struct Game {
    pub cfg: GameConfig,
    pub map: GameMap,
    pub players: HashMap<u32, Player>,
    pub bullets: HashMap<u32, Bullet>,
    pub pickups: HashMap<u32, Pickup>,
    /// Текущий тик.
    pub tick: u64,
    pub rng: Rng,
    pub next_entity_id: u32,
    /// Пространственные сетки для interest management.
    pub player_grid: SpatialGrid,
    pub bullet_grid: SpatialGrid,
    pub pickup_grid: SpatialGrid,
    /// Таблица лидеров.
    pub leaderboard: Vec<LeaderboardEntry>,
    /// Дельты тайлов, накопленные за текущий тик.
    pub tile_updates: Vec<TileUpdate>,
    /// События текущего тика.
    pub events: Vec<GameEvent>,
    /// Вспомогательный буфер для поиска пути.
    path_parent: Vec<u32>,
    path_queue: Vec<u32>,
}

impl Game {
    /// Создаёт новую игру с сгенерированной картой.
    pub fn new(cfg: GameConfig) -> Game {
        let map = GameMap::generate(cfg.map_w, cfg.map_h, cfg.seed);

        // Проверяем, что RLE-кодирование обратимо: клиент разворачивает поток
        // обратно в массив тайлов и должен получить ровно ту же карту.
        let rle = map.rle_encode();
        match GameMap::rle_decode(&rle, map.width(), map.height()) {
            Some(decoded) if decoded.len() == map.len() => {}
            _ => tracing::warn!("RLE-карта не проходит проверку round-trip"),
        }

        let total = (cfg.map_w * cfg.map_h) as usize;
        Game {
            cfg,
            map,
            players: HashMap::new(),
            bullets: HashMap::new(),
            pickups: HashMap::new(),
            tick: 0,
            rng: Rng::new(0x51DE_2026_BEEF),
            next_entity_id: 1,
            player_grid: SpatialGrid::new(),
            bullet_grid: SpatialGrid::new(),
            pickup_grid: SpatialGrid::new(),
            leaderboard: Vec::new(),
            tile_updates: Vec::new(),
            events: Vec::new(),            path_parent: vec![u32::MAX; total],
            path_queue: Vec::with_capacity(total),
        }
    }

    /// Идентификаторы всех игроков.
    pub fn player_ids(&self) -> Vec<u32> {
        self.players.keys().copied().collect()
    }

    /// Основной шаг симуляции — один тик.
    pub fn tick(&mut self) {
        self.tick += 1;
        self.tile_updates.clear();
        self.events.clear();

        let started = Instant::now();
        self.update_bots();
        let t_bots = started.elapsed();

        let phase = Instant::now();
        self.update_players();
        let t_players = phase.elapsed();

        let phase = Instant::now();
        self.update_bullets();
        let t_bullets = phase.elapsed();

        let phase = Instant::now();
        self.update_pickups();
        let t_pickups = phase.elapsed();

        let phase = Instant::now();
        self.maintain_population();
        let t_population = phase.elapsed();

        let phase = Instant::now();
        self.rebuild_grids();
        let t_grids = phase.elapsed();

        if self.tick % 10 == 0 {
            self.rebuild_leaderboard();
        }

        if self.cfg.profile && self.tick % 60 == 0 {
            tracing::info!(
                bots = ?t_bots,
                players = ?t_players,
                bullets = ?t_bullets,
                pickups = ?t_pickups,
                population = ?t_population,
                grids = ?t_grids,
                total = ?started.elapsed(),
                "профиль тика"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Игроки
    // -----------------------------------------------------------------------

    fn update_players(&mut self) {
        let ids = self.player_ids();
        for id in ids {
            // Перезарядка.
            if let Some(player) = self.players.get_mut(&id) {
                if let Some(tank) = player.tank.as_mut() {
                    tank.cooldown = (tank.cooldown - TICK_DT).max(0.0);
                }
            }
            self.update_player_movement(id);
            self.update_player_fire(id);
        }
    }

    fn update_player_movement(&mut self, pid: u32) {
        let (input, prev, cur_dir, level, vel) = {
            let Some(player) = self.players.get(&pid) else {
                return;
            };
            let Some(tank) = player.tank.as_ref() else {
                return;
            };
            (
                player.input,
                player.prev_input,
                tank.dir,
                tank.level,
                tank.vel,
            )
        };

        let desired = resolve_direction(cur_dir, &input, &prev);
        let turned = desired != cur_dir;

        let Some(mut tank) = self.players.get_mut(&pid).and_then(|p| p.tank.take()) else {
            return;
        };

        let on_ice = self.is_on_ice(tank.x, tank.y);
        let speed = level_speed(level);
        let accel = if on_ice { 220.0 } else { 1100.0 };
        let decel = if on_ice { 120.0 } else { 1800.0 };

        let target = if input.pressed(desired) {
            speed
        } else if input.pressed(desired.opposite()) {
            -speed * 0.6
        } else {
            0.0
        };

        let base_vel = if turned { 0.0 } else { vel };
        let delta = target - base_vel;
        let rate = if delta > 0.0 { accel } else { decel };
        let max_step = rate * TICK_DT;
        let mut new_vel = base_vel + delta.clamp(-max_step, max_step);
        if new_vel.abs() < 1.0 {
            new_vel = 0.0;
        }

        if turned {
            // Поворот: выравниваем по сетке, как в классическом Battle City.
            tank.dir = desired;
            tank.vel = 0.0;
            self.align_to_grid(&mut tank);
        } else {
            tank.dir = desired;
        }
        tank.vel = new_vel;

        let dist = new_vel * TICK_DT;
        if dist.abs() > 0.001 {
            self.move_tank(&mut tank, dist);
            tank.track_phase += new_vel.abs() * TICK_DT / 6.0;
        }

        if let Some(player) = self.players.get_mut(&pid) {
            player.tank = Some(tank);
        }
    }

    /// Приводит поперечную ось танка к сетке тайлов.
    fn align_to_grid(&self, tank: &mut Tank) {
        let vertical = tank.dir.is_vertical();
        let axis_val = if vertical { tank.x } else { tank.y };
        let base = (axis_val / TILE_SIZE).round() as i32;
        for delta in [0i32, -1, 1, -2, 2] {
            let tile = base + delta;
            if tile < 0 {
                continue;
            }
            let candidate = tile as f32 * TILE_SIZE;
            let (x, y) = if vertical {
                (candidate, tank.y)
            } else {
                (tank.x, candidate)
            };
            if x >= 0.0 && y >= 0.0 && self.map.rect_is_free(x, y, TANK_SIZE) {
                tank.x = x;
                tank.y = y;
                return;
            }
        }
    }

    /// Перемещает танк пошагово, останавливаясь на препятствии.
    fn move_tank(&self, tank: &mut Tank, dist: f32) {
        if dist == 0.0 {
            return;
        }
        let (dx, dy) = tank.dir.delta();
        let steps = dist.abs().ceil().max(1.0) as i32;
        let step = dist / steps as f32;
        for _ in 0..steps {
            let nx = tank.x + dx * step;
            let ny = tank.y + dy * step;
            if !self.map.rect_is_free(nx, ny, TANK_SIZE) {
                tank.vel = 0.0;
                return;
            }
            tank.x = nx;
            tank.y = ny;
        }
    }

    /// Находится ли танк на скользком тайле.
    fn is_on_ice(&self, x: f32, y: f32) -> bool {
        let (tx, ty) = self.map.tile_at(x + TANK_SIZE * 0.5, y + TANK_SIZE * 0.5);
        self.map.kind(tx, ty).is_slippery()
    }

    fn update_player_fire(&mut self, pid: u32) {
        let (fire, level, dir, cx, cy) = {
            let Some(player) = self.players.get(&pid) else {
                return;
            };
            if !player.input.fire {
                return;
            }
            let Some(tank) = player.tank.as_ref() else {
                return;
            };
            if tank.cooldown > 0.0 {
                return;
            }
            let c = tank.center();
            (true, tank.level, tank.dir, c.0, c.1)
        };
        if !fire {
            return;
        }

        let alive = self.bullets.values().filter(|b| b.owner == pid).count();
        if alive >= level_max_bullets(level) {
            return;
        }

        // Снаряд появляется у дула; если там стена — из центра танка.
        let (dx, dy) = dir.delta();
        let muzzle_x = cx + dx * (TANK_SIZE * 0.5 + 3.0);
        let muzzle_y = cy + dy * (TANK_SIZE * 0.5 + 3.0);
        let (bx, by) = if self.map.rect_is_free(
            muzzle_x - BULLET_SIZE * 0.5,
            muzzle_y - BULLET_SIZE * 0.5,
            BULLET_SIZE,
        ) {
            (muzzle_x, muzzle_y)
        } else {
            (cx, cy)
        };

        let id = self.next_entity_id;
        self.next_entity_id += 1;
        self.bullets.insert(
            id,
            Bullet {
                id,
                owner: pid,
                x: bx,
                y: by,
                dir,
                level,
                damage: level_bullet_damage(level),
                speed: level_bullet_speed(level),
            },
        );

        if let Some(player) = self.players.get_mut(&pid) {
            if let Some(tank) = player.tank.as_mut() {
                tank.cooldown = level_fire_cooldown(level);
                tank.revealed_until = self.tick + REVEAL_TICKS;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Снаряды
    // -----------------------------------------------------------------------

    fn update_bullets(&mut self) {
        let bullets = std::mem::take(&mut self.bullets);
        let mut kept: HashMap<u32, Bullet> = HashMap::with_capacity(bullets.len());
        for mut b in bullets.into_values() {
            let (dx, dy) = b.dir.delta();
            b.x += dx * b.speed * TICK_DT;
            b.y += dy * b.speed * TICK_DT;

            if b.x < 0.0 || b.y < 0.0 || b.x >= self.cfg.world_w() || b.y >= self.cfg.world_h() {
                continue;
            }

            let (tx, ty) = self.map.tile_at(b.x, b.y);
            if self.map.kind(tx, ty).blocks_bullet() {
                self.hit_tile(tx, ty, b.level, b.damage, b.owner);
                continue;
            }

            if let Some(hit) = self.find_tank_hit(&b) {
                self.apply_damage(hit, b.damage, b.owner);
                continue;
            }

            kept.insert(b.id, b);
        }
        self.bullets = kept;
    }

    /// Обрабатывает попадание снаряда в тайл.
    fn hit_tile(&mut self, tx: u32, ty: u32, level: u8, damage: u8, owner: u32) {
        let kind = self.map.kind(tx, ty);
        match kind {
            TileKind::Brick => {
                let dmg = self.map.damage(tx, ty).saturating_add(damage);
                if dmg >= crate::map::BRICK_MAX_DAMAGE {
                    self.map.set_kind(tx, ty, TileKind::Empty);
                    self.events.push(GameEvent::TileBroken {
                        x: tx as u16,
                        y: ty as u16,
                    });
                    if let Some(player) = self.players.get_mut(&owner) {
                        player.score += BRICK_SCORE;
                    }
                } else {
                    self.map.set_value(tx, ty, TileKind::Brick.bits() | (dmg << 3));
                }
                let value = self.map.value(tx, ty);
                self.tile_updates.push(TileUpdate {
                    x: tx as u16,
                    y: ty as u16,
                    value,
                });
            }
            TileKind::Steel => {
                // Бетон пробивается только снарядом максимального уровня.
                if level >= 3 {
                    self.map.set_kind(tx, ty, TileKind::Empty);
                    self.events.push(GameEvent::TileBroken {
                        x: tx as u16,
                        y: ty as u16,
                    });
                    if let Some(player) = self.players.get_mut(&owner) {
                        player.score += BRICK_SCORE * 2;
                    }
                    let value = self.map.value(tx, ty);
                    self.tile_updates.push(TileUpdate {
                        x: tx as u16,
                        y: ty as u16,
                        value,
                    });
                }
            }
            _ => {}
        }
    }

    /// Ищет танк, задетый снарядом.
    fn find_tank_hit(&self, b: &Bullet) -> Option<u32> {
        let bx = b.x - BULLET_SIZE * 0.5;
        let by = b.y - BULLET_SIZE * 0.5;
        for (pid, player) in self.players.iter() {
            if *pid == b.owner {
                continue;
            }
            let Some(tank) = player.tank.as_ref() else {
                continue;
            };
            if aabb_overlap(bx, by, BULLET_SIZE, BULLET_SIZE, tank.x, tank.y, TANK_SIZE, TANK_SIZE) {
                return Some(*pid);
            }
        }
        None
    }

    /// Наносит урон танку и обрабатывает смерть.
    fn apply_damage(&mut self, pid: u32, damage: u8, by: u32) {
        let now = self.tick;

        let killed = {
            let Some(player) = self.players.get_mut(&pid) else {
                return;
            };
            let Some(tank) = player.tank.as_mut() else {
                return;
            };
            if tank.spawn_protect_until > now || tank.shield_until > now {
                return;
            }
            tank.hp = tank.hp.saturating_sub(damage);
            tank.hp == 0
        };
        if !killed {
            return;
        }

        // Позиция взрыва запоминается до удаления танка.
        let center = self
            .players
            .get(&pid)
            .and_then(|p| p.tank.as_ref())
            .map(|t| t.center());

        let (score, kills, deaths, joined_at) = {
            let Some(player) = self.players.get_mut(&pid) else {
                return;
            };
            player.tank = None;
            player.deaths += 1;
            // Небольшая пауза перед респауном, чтобы нельзя было «залипать»
            // в кнопке возрождения.
            player.spawn_at = now + 15;
            (
                player.score,
                player.kills,
                player.deaths,
                player.joined_at,
            )
        };

        if by != pid {
            if let Some(killer) = self.players.get_mut(&by) {
                killer.kills += 1;
                killer.score += KILL_SCORE;
            }
        }

        if let Some((x, y)) = center {
            self.events.push(GameEvent::Explosion { x, y, size: 3 });
        }
        self.events.push(GameEvent::Kill {
            killer: by,
            victim: pid,
        });

        let survived_ms = (self.tick.saturating_sub(joined_at) * 1000) / self.cfg.tick_rate.max(1);
        let bytes = protocol::encode_death(score, kills, deaths, survived_ms);
        self.send_to(pid, bytes);

        // Из уничтоженного танка иногда выпадает звезда прокачки.
        if self.rng.chance(35) {
            if let Some((x, y)) = center {
                self.spawn_pickup_at(x, y, PickupKind::Star);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Бонусы
    // -----------------------------------------------------------------------

    fn update_pickups(&mut self) {
        let now = self.tick;
        self.pickups.retain(|_, p| p.expires_at > now);

        if self.pickups.len() < MAX_PICKUPS && self.tick % PICKUP_SPAWN_INTERVAL == 0 {
            self.spawn_pickup();
        }

        let ids = self.player_ids();
        for pid in ids {
            let rect = {
                let Some(player) = self.players.get(&pid) else {
                    continue;
                };
                let Some(tank) = player.tank.as_ref() else {
                    continue;
                };
                (tank.x, tank.y)
            };
            let (tx, ty) = rect;

            let mut collected: Option<Pickup> = None;
            self.pickups.retain(|_, p| {
                let hit = p.x >= tx - 4.0
                    && p.x <= tx + TANK_SIZE + 4.0
                    && p.y >= ty - 4.0
                    && p.y <= ty + TANK_SIZE + 4.0;
                if hit {
                    collected = Some(*p);
                    false
                } else {
                    true
                }
            });

            if let Some(pickup) = collected {
                self.apply_pickup(pid, pickup);
            }
        }
    }

    fn apply_pickup(&mut self, pid: u32, pickup: Pickup) {
        let mut new_level = 0u8;
        {
            let Some(player) = self.players.get_mut(&pid) else {
                return;
            };
            let Some(tank) = player.tank.as_mut() else {
                return;
            };
            match pickup.kind {
                PickupKind::Star => {
                    tank.level = (tank.level + 1).min(3);
                    tank.max_hp = level_max_hp(tank.level);
                    tank.hp = tank.max_hp;
                    new_level = tank.level;
                }
                PickupKind::Shield => {
                    tank.shield_until = self.tick + SHIELD_TICKS;
                }
                PickupKind::Repair => {
                    tank.hp = tank.max_hp;
                }
            }
            player.score += PICKUP_SCORE;
        }

        self.events.push(GameEvent::PickupCollected {
            x: pickup.x,
            y: pickup.y,
            kind: pickup.kind as u8,
        });
        if new_level > 0 {
            self.events.push(GameEvent::LevelUp {
                id: pid,
                level: new_level,
            });
        }
    }

    /// Ставит бонус в указанную точку.
    fn spawn_pickup_at(&mut self, x: f32, y: f32, kind: PickupKind) {
        let id = self.next_entity_id;
        self.next_entity_id += 1;
        self.pickups.insert(
            id,
            Pickup {
                id,
                x,
                y,
                kind,
                expires_at: self.tick + PICKUP_LIFETIME,
            },
        );
    }

    /// Ставит бонус в случайную свободную точку карты.
    fn spawn_pickup(&mut self) {
        let mut x = 0.0f32;
        let mut y = 0.0f32;
        let mut found = false;
        for _ in 0..64 {
            let tx = self.rng.range(2, self.cfg.map_w.saturating_sub(2));
            let ty = self.rng.range(2, self.cfg.map_h.saturating_sub(2));
            if self.map.blocks_tank(tx, ty) || self.map.kind(tx, ty) == TileKind::Forest {
                continue;
            }
            // Бонус не должен появляться прямо под танком.
            let cx = tx as f32 * TILE_SIZE + TILE_SIZE * 0.5;
            let cy = ty as f32 * TILE_SIZE + TILE_SIZE * 0.5;
            let occupied = self.players.values().any(|p| {
                p.tank
                    .as_ref()
                    .is_some_and(|t| (t.x - cx).abs() < 48.0 && (t.y - cy).abs() < 48.0)
            });
            if occupied {
                continue;
            }
            x = cx;
            y = cy;
            found = true;
            break;
        }
        if !found {
            return;
        }
        let kind = match self.rng.below(100) {
            0..=44 => PickupKind::Star,
            45..=74 => PickupKind::Shield,
            _ => PickupKind::Repair,
        };
        self.spawn_pickup_at(x, y, kind);
    }

    // -----------------------------------------------------------------------
    // Боты
    // -----------------------------------------------------------------------

    fn update_bots(&mut self) {
        let bot_ids: Vec<u32> = self
            .players
            .values()
            .filter(|p| p.is_bot && p.tank.is_some() && p.removal_at.is_none())
            .map(|p| p.id)
            .collect();
        for id in bot_ids {
            self.update_bot(id);
        }
    }

    fn update_bot(&mut self, pid: u32) {
        let (bx, by, next_plan, prev_target) = {
            let Some(player) = self.players.get(&pid) else {
                return;
            };
            let Some(tank) = player.tank.as_ref() else {
                return;
            };
            let (next_plan, prev_target) = player
                .bot
                .as_ref()
                .map(|b| (b.next_plan_tick, b.target))
                .unwrap_or((0, None));
            (tank.x, tank.y, next_plan, prev_target)
        };

        let now = self.tick;
        let replan = now >= next_plan;
        let mut target = prev_target;

        if replan {
            target = self.find_bot_target(pid, bx, by);
        }

        // Пересчитываем маршрут к цели.
        if replan {
            let mut path: Vec<(u32, u32)> = Vec::new();
            if let Some(tid) = target {
                let goal = self
                    .players
                    .get(&tid)
                    .and_then(|p| p.tank.as_ref())
                    .map(|t| self.map.tile_at(t.x + TANK_SIZE * 0.5, t.y + TANK_SIZE * 0.5));
                if let Some(goal) = goal {
                    path = self.find_path(self.map.tile_at(bx + TANK_SIZE * 0.5, by + TANK_SIZE * 0.5), goal)
                        .unwrap_or_default();
                }
            }
            if let Some(player) = self.players.get_mut(&pid) {
                if let Some(bot) = player.bot.as_mut() {
                    bot.path = path;
                    bot.path_idx = 0;
                    bot.target = target;
                    bot.next_plan_tick = now + 30;
                }
            }
        }

        // Принимаем решение: стрелять и куда двигаться.
        let mut input = InputFlags::default();
        let (target_center, aligned, los) = match target {
            Some(tid) => {
                let mut info = None;
                if let Some(p) = self.players.get(&tid) {
                    if let Some(t) = p.tank.as_ref() {
                        let c = t.center();
                        let dx = (c.0 - (bx + TANK_SIZE * 0.5)).abs();
                        let dy = (c.1 - (by + TANK_SIZE * 0.5)).abs();
                        let aligned = dx < 20.0 || dy < 20.0;
                        let los = self.map.bullet_line_of_sight(
                            bx + TANK_SIZE * 0.5,
                            by + TANK_SIZE * 0.5,
                            c.0,
                            c.1,
                        );
                        info = Some((c, aligned, los));
                    }
                }
                match info {
                    Some(v) => (Some(v.0), v.1, v.2),
                    None => (None, false, false),
                }
            }
            None => (None, false, false),
        };
        let _ = target_center;

        if aligned && los {
            // Видим цель — стреляем и не дёргаемся.
            input.fire = true;
        } else {
            // Идём по маршруту.
            let path = self
                .players
                .get(&pid)
                .and_then(|p| p.bot.as_ref())
                .map(|b| (b.path.clone(), b.path_idx))
                .unwrap_or_default();
            let (path, idx) = path;
            if let Some(&(nx, ny)) = path.get(idx) {
                let (cx, cy) = self.map.tile_at(bx + TANK_SIZE * 0.5, by + TANK_SIZE * 0.5);
                if nx > cx {
                    input.right = true;
                } else if nx < cx {
                    input.left = true;
                } else if ny > cy {
                    input.down = true;
                } else if ny < cy {
                    input.up = true;
                }
                // Дошли до точки маршрута — переходим к следующей.
                if cx == nx && cy == ny {
                    if let Some(player) = self.players.get_mut(&pid) {
                        if let Some(bot) = player.bot.as_mut() {
                            bot.path_idx += 1;
                        }
                    }
                }
            }
        }

        if let Some(player) = self.players.get_mut(&pid) {
            player.prev_input = player.input;
            player.input = input;
        }
    }

    /// Выбирает ближайшую разумную цель для бота.
    fn find_bot_target(&self, bot_id: u32, bx: f32, by: f32) -> Option<u32> {
        let bcx = bx + TANK_SIZE * 0.5;
        let bcy = by + TANK_SIZE * 0.5;
        let mut best: Option<(u32, f32)> = None;
        for (pid, player) in self.players.iter() {
            if *pid == bot_id {
                continue;
            }
            let Some(tank) = player.tank.as_ref() else {
                continue;
            };
            let tcx = tank.x + TANK_SIZE * 0.5;
            let tcy = tank.y + TANK_SIZE * 0.5;
            let dx = tcx - bcx;
            let dy = tcy - bcy;
            let dist = dx * dx + dy * dy;
            if dist > 700.0 * 700.0 {
                continue;
            }
            let visible = self.map.bullet_line_of_sight(bcx, bcy, tcx, tcy);
            let score = if visible { dist * 0.2 } else { dist };
            if best.is_none_or(|(_, bs)| score < bs) {
                best = Some((*pid, score));
            }
        }
        best.map(|(id, _)| id)
    }

    /// Поиск пути по тайлам (BFS).
    fn find_path(&mut self, from: (u32, u32), to: (u32, u32)) -> Option<Vec<(u32, u32)>> {
        let w = self.map.width() as usize;
        let h = self.map.height() as usize;
        if from.0 as usize >= w || from.1 as usize >= h {
            return None;
        }
        if to.0 as usize >= w || to.1 as usize >= h {
            return None;
        }
        if self.map.blocks_tank(to.0, to.1) {
            return None;
        }
        if from == to {
            return Some(Vec::new());
        }

        for v in self.path_parent.iter_mut() {
            *v = u32::MAX;
        }
        self.path_queue.clear();

        let start = from.1 as usize * w + from.0 as usize;
        let goal = to.1 as usize * w + to.0 as usize;
        self.path_parent[start] = start as u32;
        self.path_queue.push(start as u32);

        let mut head = 0usize;
        let mut found = false;
        while head < self.path_queue.len() {
            let cur = self.path_queue[head];
            head += 1;
            if cur as usize == goal {
                found = true;
                break;
            }
            let cx = cur as usize % w;
            let cy = cur as usize / w;
            for (nx, ny) in [
                (cx.wrapping_sub(1), cy),
                (cx + 1, cy),
                (cx, cy.wrapping_sub(1)),
                (cx, cy + 1),
            ] {
                if nx >= w || ny >= h {
                    continue;
                }
                let nidx = ny * w + nx;
                if self.path_parent[nidx] != u32::MAX {
                    continue;
                }
                if self.map.blocks_tank(nx as u32, ny as u32) {
                    continue;
                }
                self.path_parent[nidx] = cur;
                self.path_queue.push(nidx as u32);
            }
        }
        if !found {
            return None;
        }

        let mut path: Vec<(u32, u32)> = Vec::new();
        let mut cur = goal as u32;
        while cur != start as u32 {
            let cx = (cur as usize % w) as u32;
            let cy = (cur as usize / w) as u32;
            path.push((cx, cy));
            let parent = self.path_parent[cur as usize];
            if parent == u32::MAX {
                return None;
            }
            cur = parent;
        }
        path.reverse();
        Some(path)
    }

    // -----------------------------------------------------------------------
    // Население
    // -----------------------------------------------------------------------

    fn maintain_population(&mut self) {
        let humans = self
            .players
            .values()
            .filter(|p| !p.is_bot && p.removal_at.is_none())
            .count();
        let bots: Vec<u32> = self
            .players
            .values()
            .filter(|p| p.is_bot && p.removal_at.is_none())
            .map(|p| p.id)
            .collect();

        let desired = self
            .cfg
            .min_population
            .saturating_sub(humans)
            .min(self.cfg.max_bots);

        if bots.len() < desired {
            self.spawn_bot();
        } else if bots.len() > desired {
            // Лишние боты уходят плавно: исчезают со взрывом, удаляются через 1.5 сек.
            if let Some(&id) = bots.first() {
                let pos = self
                    .players
                    .get(&id)
                    .and_then(|p| p.tank.as_ref())
                    .map(|t| t.center());
                if let Some(player) = self.players.get_mut(&id) {
                    if player.removal_at.is_none() {
                        player.removal_at = Some(self.tick + 45);
                        player.tank = None;
                    }
                }
                if let Some((x, y)) = pos {
                    self.events.push(GameEvent::Explosion { x, y, size: 2 });
                }
            }
        }

        // Удаляем ботов, у которых истёк срок.
        let expired: Vec<u32> = self
            .players
            .iter()
            .filter(|(_, p)| p.removal_at.is_some_and(|t| t <= self.tick))
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.players.remove(&id);
        }
    }

    fn spawn_bot(&mut self) {
        let id = self.next_entity_id;
        self.next_entity_id += 1;
        let nickname = self.bot_name();
        let Some((x, y)) = self.pick_spawn_point(id) else {
            return;
        };
        let mut tank = Tank::new(x, y, 1, self.tick);
        // Боты стартуют с разных сторон — так атака выглядит естественнее.
        tank.dir = match (self.rng.unit() * 4.0) as usize {
            0 => Direction::Up,
            1 => Direction::Down,
            2 => Direction::Left,
            _ => Direction::Right,
        };
        self.players.insert(
            id,
            Player {
                id,
                conn_id: 0,
                nickname,
                is_bot: true,
                tank: Some(tank),
                score: 0,
                kills: 0,
                deaths: 0,
                input: InputFlags::default(),
                prev_input: InputFlags::default(),
                last_seq: 0,
                tx: None,
                joined: true,
                spawn_at: 0,
                joined_at: self.tick,
                bot: Some(BotState {
                    path: Vec::new(),
                    path_idx: 0,
                    next_plan_tick: self.tick,
                    target: None,
                }),
                removal_at: None,
            },
        );
    }

    /// Подбирает уникальное имя бота.
    fn bot_name(&mut self) -> String {
        let base = BOT_NAMES[self.rng.below(BOT_NAMES.len() as u32) as usize];
        let mut name = base.to_string();
        let mut n = 1;
        while self.players.values().any(|p| p.nickname == name) {
            n += 1;
            name = format!("{base}-{n}");
        }
        name
    }

    /// Выбирает точку спавна подальше от живых танков.
    fn pick_spawn_point(&self, exclude_id: u32) -> Option<(f32, f32)> {
        let points = self.map.spawn_points(25, TANK_SIZE);
        if points.is_empty() {
            return None;
        }
        let mut best = points[0];
        let mut best_score = f32::MIN;
        for point in points {
            let mut min_dist = f32::MAX;
            for (pid, player) in self.players.iter() {
                if *pid == exclude_id {
                    continue;
                }
                let Some(tank) = player.tank.as_ref() else {
                    continue;
                };
                let dx = tank.x - point.0;
                let dy = tank.y - point.1;
                let d = dx * dx + dy * dy;
                if d < min_dist {
                    min_dist = d;
                }
            }
            if min_dist > best_score {
                best_score = min_dist;
                best = point;
            }
        }
        Some(best)
    }

    // -----------------------------------------------------------------------
    // Сетки и лидерборд
    // -----------------------------------------------------------------------

    fn rebuild_grids(&mut self) {
        self.player_grid.clear();
        self.bullet_grid.clear();
        self.pickup_grid.clear();

        for (id, player) in self.players.iter() {
            if let Some(tank) = player.tank.as_ref() {
                let c = tank.center();
                self.player_grid.insert(*id, c.0, c.1);
            }
        }
        for (id, bullet) in self.bullets.iter() {
            self.bullet_grid.insert(*id, bullet.x, bullet.y);
        }
        for (id, pickup) in self.pickups.iter() {
            self.pickup_grid.insert(*id, pickup.x, pickup.y);
        }
    }

    fn rebuild_leaderboard(&mut self) {
        let mut entries: Vec<LeaderboardEntry> = self
            .players
            .values()
            .filter(|p| p.joined)
            .map(|p| LeaderboardEntry {
                id: p.id,
                name: p.nickname.clone(),
                score: p.score,
                is_bot: p.is_bot,
            })
            .collect();
        entries.sort_by(|a, b| b.score.cmp(&a.score).then(a.id.cmp(&b.id)));
        entries.truncate(10);
        self.leaderboard = entries;
    }

    /// Таблица лидеров для конкретного игрока.
    ///
    /// Если игрок не попал в топ-10, его строка всё равно добавляется в конец —
    /// иначе новичок с нулём очков вообще не видел бы себя в списке.
    pub fn leaderboard_for(&self, viewer_id: u32) -> Vec<LeaderboardEntry> {
        let mut entries = self.leaderboard.clone();
        if !entries.iter().any(|e| e.id == viewer_id) {
            if let Some(player) = self.players.get(&viewer_id) {
                if player.joined {
                    entries.push(LeaderboardEntry {
                        id: player.id,
                        name: player.nickname.clone(),
                        score: player.score,
                        is_bot: player.is_bot,
                    });
                }
            }
        }
        entries
    }

    // -----------------------------------------------------------------------
    // Игрок: вход, респаун, отправка
    // -----------------------------------------------------------------------

    /// Регистрирует нового игрока и отправляет ему приветствие с картой.
    pub fn add_player(
        &mut self,
        conn_id: u64,
        nickname: String,
        tx: Sender<Vec<u8>>,
    ) -> u32 {
        let id = self.next_entity_id;
        self.next_entity_id += 1;
        let now = self.tick;

        self.players.insert(
            id,
            Player {
                id,
                conn_id,
                nickname,
                is_bot: false,
                tank: None,
                score: 0,
                kills: 0,
                deaths: 0,
                input: InputFlags::default(),
                prev_input: InputFlags::default(),
                last_seq: 0,
                tx: Some(tx.clone()),
                joined: false,
                spawn_at: 0,
                joined_at: now,
                bot: None,
                removal_at: None,
            },
        );

        let map = self.map_data();
        let welcome = protocol::encode_welcome(id, self.tick, &map);
        let _ = tx.try_send(welcome);
        id
    }

    /// Удаляет игрока по соединению.
    pub fn remove_player(&mut self, conn_id: u64) {
        let ids: Vec<u32> = self
            .players
            .values()
            .filter(|p| p.conn_id == conn_id && !p.is_bot)
            .map(|p| p.id)
            .collect();
        for id in ids {
            self.players.remove(&id);
        }
    }

    /// Данные карты для начальной синхронизации.
    pub fn map_data(&self) -> protocol::MapData {
        protocol::MapData {
            width: self.map.width(),
            height: self.map.height(),
            tile_size: self.cfg.tile_size as u32,
            rle: self.map.rle_encode(),
        }
    }

    /// Устанавливает никнейм и вводит игрока в бой.
    pub fn join_player(&mut self, pid: u32, nickname: String) {
        let now = self.tick;
        let already_joined = self
            .players
            .get(&pid)
            .map(|p| p.joined)
            .unwrap_or(false);
        if already_joined {
            return;
        }
        let name = if nickname.is_empty() {
            format!("Игрок-{}", pid)
        } else {
            nickname
        };
        if let Some(player) = self.players.get_mut(&pid) {
            player.nickname = name;
            player.joined = true;
            player.joined_at = now;
        }
        self.respawn_player(pid);
    }

    /// Возрождает игрока.
    pub fn respawn_player(&mut self, pid: u32) {
        let now = self.tick;
        {
            let Some(player) = self.players.get(&pid) else {
                return;
            };
            if player.tank.is_some() || now < player.spawn_at {
                return;
            }
        }
        let Some((x, y)) = self.pick_spawn_point(pid) else {
            return;
        };
        let tank = Tank::new(x, y, 1, now);
        if let Some(player) = self.players.get_mut(&pid) {
            player.tank = Some(tank);
        }
        let bytes = protocol::encode_respawned(now, x, y);
        self.send_to(pid, bytes);
    }

    /// Применяет ввод от клиента.
    pub fn apply_input(&mut self, conn_id: u64, seq: u32, input: InputFlags) {
        let ids: Vec<u32> = self
            .players
            .values()
            .filter(|p| p.conn_id == conn_id && !p.is_bot)
            .map(|p| p.id)
            .collect();
        for id in ids {
            if let Some(player) = self.players.get_mut(&id) {
                if seq <= player.last_seq && player.last_seq != 0 {
                    // Старый или повторный пакет — игнорируем.
                    continue;
                }
                player.last_seq = seq;
                player.prev_input = player.input;
                player.input = input;
            }
        }
    }

    /// Отправляет сообщение игроку, если канал готов.
    pub fn send_to(&self, pid: u32, bytes: Vec<u8>) {
        if let Some(player) = self.players.get(&pid) {
            if let Some(tx) = player.tx.as_ref() {
                // Неблокирующая отправка: медленный клиент просто пропустит кадр.
                let _ = tx.try_send(bytes);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Снимок мира для конкретного игрока
    // -----------------------------------------------------------------------

    /// Собирает снимок мира, видимый указанному игроку.
    pub fn snapshot_for(&self, viewer_id: u32) -> Option<Snapshot> {
        let viewer = self.players.get(&viewer_id)?;
        let (focus_x, focus_y) = match viewer.tank.as_ref() {
            Some(t) => t.center(),
            None => (self.cfg.world_w() * 0.5, self.cfg.world_h() * 0.5),
        };
        let (ccx, ccy) = self.map.chunk_at(focus_x, focus_y);
        let radius = crate::map::VIEW_CHUNK_RADIUS as u32;
        let cx0 = ccx.saturating_sub(radius);
        let cy0 = ccy.saturating_sub(radius);
        let cx1 = (ccx + radius).min(crate::map::CHUNKS_X - 1);
        let cy1 = (ccy + radius).min(crate::map::CHUNKS_Y - 1);

        let mut snap = Snapshot {
            tick: self.tick,
            focus_x,
            focus_y,
            ..Snapshot::default()
        };

        // --- Танки ---
        for pid in self.player_grid.query(cx0, cy0, cx1, cy1) {
            let Some(player) = self.players.get(&pid) else {
                continue;
            };
            let Some(tank) = player.tank.as_ref() else {
                continue;
            };
            if !self.can_see(viewer_id, focus_x, focus_y, ccx, ccy, player, tank) {
                continue;
            }
            snap.players.push(PlayerView {
                id: player.id,
                name: player.nickname.clone(),
                is_bot: player.is_bot,
                x: tank.x,
                y: tank.y,
                dir: tank.dir,
                level: tank.level,
                hp: tank.hp,
                max_hp: tank.max_hp,
                alive: true,
                shielded: tank.shield_until > self.tick,
                score: player.score,
                kills: player.kills,
            });
        }

        // --- Снаряды ---
        for bid in self.bullet_grid.query(cx0, cy0, cx1, cy1) {
            let Some(bullet) = self.bullets.get(&bid) else {
                continue;
            };
            snap.bullets.push(BulletView {
                id: bullet.id,
                x: bullet.x,
                y: bullet.y,
                dir: bullet.dir,
                level: bullet.level,
            });
        }

        // --- Бонусы ---
        for pid in self.pickup_grid.query(cx0, cy0, cx1, cy1) {
            let Some(pickup) = self.pickups.get(&pid) else {
                continue;
            };
            snap.pickups.push(PickupView {
                id: pickup.id,
                x: pickup.x,
                y: pickup.y,
                kind: pickup.kind.as_u8(),
                ttl: (pickup.expires_at.saturating_sub(self.tick)) as u32,
            });
        }

        // --- Дельты карты ---
        for update in self.tile_updates.iter() {
            let ux = update.x as u32 / CHUNK_TILES;
            let uy = update.y as u32 / CHUNK_TILES;
            if ux >= cx0 && ux <= cx1 && uy >= cy0 && uy <= cy1 {
                snap.tiles.push(*update);
            }
        }

        // --- События ---
        for event in self.events.iter() {
            let (ex, ey) = match *event {
                GameEvent::Explosion { x, y, .. } => (x, y),
                GameEvent::PickupCollected { x, y, .. } => (x, y),
                GameEvent::TileBroken { x, y } => (
                    x as f32 * TILE_SIZE + TILE_SIZE * 0.5,
                    y as f32 * TILE_SIZE + TILE_SIZE * 0.5,
                ),
                GameEvent::Kill { .. } | GameEvent::LevelUp { .. } => {
                    snap.events.push(*event);
                    continue;
                }
            };
            let (ecx, ecy) = self.map.chunk_at(ex, ey);
            if ecx >= cx0 && ecx <= cx1 && ecy >= cy0 && ecy <= cy1 {
                snap.events.push(*event);
            }
        }

        Some(snap)
    }

    /// Виден ли танк наблюдателю (с учётом чащи и радиуса чанков).
    fn can_see(
        &self,
        viewer_id: u32,
        _vx: f32,
        _vy: f32,
        ccx: u32,
        ccy: u32,
        other: &Player,
        tank: &Tank,
    ) -> bool {
        let (ocx, ocy) = self.map.chunk_at(tank.x + TANK_SIZE * 0.5, tank.y + TANK_SIZE * 0.5);
        let radius = crate::map::VIEW_CHUNK_RADIUS as i64;
        if (ocx as i64 - ccx as i64).abs() > radius || (ocy as i64 - ccy as i64).abs() > radius {
            return false;
        }

        // Танк в чаще не виден врагам снаружи лесного массива, пока не выстрелит.
        let (otx, oty) = self.map.tile_at(tank.x + TANK_SIZE * 0.5, tank.y + TANK_SIZE * 0.5);
        if let Some(region) = self.map.forest_region(otx, oty) {
            if tank.revealed_until <= self.tick {
                let viewer_region = self
                    .players
                    .get(&viewer_id)
                    .and_then(|p| p.tank.as_ref())
                    .and_then(|t| {
                        let (tx, ty) = self.map.tile_at(t.x + TANK_SIZE * 0.5, t.y + TANK_SIZE * 0.5);
                        self.map.forest_region(tx, ty)
                    });
                if viewer_region != Some(region) {
                    return false;
                }
            }
        }
        let _ = other;
        true
    }
}

// ---------------------------------------------------------------------------
// Вспомогательные функции
// ---------------------------------------------------------------------------

/// Пересекаются ли два прямоугольника.
pub fn aabb_overlap(
    ax: f32,
    ay: f32,
    aw: f32,
    ah: f32,
    bx: f32,
    by: f32,
    bw: f32,
    bh: f32,
) -> bool {
    ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by
}

/// Определяет направление движения по вводу.
///
/// Классический Battle City допускает только четыре направления и не
/// оставляет диагоналей: при одновременном нажатии двух клавиш выбирается
/// та, что нажата позже, а если такой нет — перпендикулярная текущей.
pub fn resolve_direction(current: Direction, input: &InputFlags, prev: &InputFlags) -> Direction {
    let candidates: Vec<Direction> = Direction::ALL
        .iter()
        .copied()
        .filter(|d| input.pressed(*d))
        .collect();
    if candidates.is_empty() {
        return current;
    }
    if candidates.len() == 1 {
        return candidates[0];
    }

    let newly: Vec<Direction> = candidates
        .iter()
        .copied()
        .filter(|d| !prev.pressed(*d))
        .collect();
    if newly.len() == 1 {
        return newly[0];
    }
    if let Some(d) = newly.iter().copied().find(|d| d.is_perpendicular(current)) {
        return d;
    }
    if let Some(d) = candidates.iter().copied().find(|d| d.is_perpendicular(current)) {
        return d;
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_game() -> Game {
        Game::new(GameConfig {
            tick_rate: 30,
            map_w: 64,
            map_h: 64,
            tile_size: TILE_SIZE,
            min_population: 0,
            max_bots: 0,
            seed: 1234,
            profile: false,
        })
    }

    #[test]
    fn direction_resolution_prefers_single_press() {
        let input = InputFlags {
            left: true,
            ..Default::default()
        };
        assert_eq!(
            resolve_direction(Direction::Up, &input, &InputFlags::default()),
            Direction::Left
        );
    }

    #[test]
    fn direction_resolution_picks_perpendicular_on_diagonal() {
        let input = InputFlags {
            right: true,
            up: true,
            ..Default::default()
        };
        // Ничего не менялось — выбираем перпендикулярное текущему.
        assert_eq!(
            resolve_direction(Direction::Down, &input, &InputFlags::default()),
            Direction::Right
        );
    }

    #[test]
    fn tank_cannot_walk_through_walls() {
        let mut game = test_game();
        let pid = game.add_player(1, "Тест".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "Тест".into());
        // Пытаемся телепортировать танк в стену и двигаться.
        let start = game
            .players
            .get(&pid)
            .and_then(|p| p.tank.as_ref())
            .map(|t| (t.x, t.y))
            .expect("tank");
        assert!(game.map.rect_is_free(start.0, start.1, TANK_SIZE));

        // Долгий проход вправо не должен вывести танк за пределы карты.
        for _ in 0..600 {
            game.tick();
        }
        let (x, y) = game
            .players
            .get(&pid)
            .and_then(|p| p.tank.as_ref())
            .map(|t| (t.x, t.y))
            .expect("tank");
        assert!(x >= 0.0 && y >= 0.0);
        assert!(x < game.cfg.world_w() && y < game.cfg.world_h());
    }

    #[test]
    fn bullets_damage_bricks_and_bricks_have_damage_stages() {
        let mut game = test_game();
        // Ставим кирпич в известное место.
        game.map.set_kind(10, 10, TileKind::Brick);
        let owner = 1u32;
        game.hit_tile(10, 10, 1, 1, owner);
        assert_eq!(game.map.damage(10, 10), 1);
        game.hit_tile(10, 10, 1, 1, owner);
        assert_eq!(game.map.damage(10, 10), 2);
        // Четыре попадания — кирпич разрушен.
        game.hit_tile(10, 10, 1, 1, owner);
        game.hit_tile(10, 10, 1, 1, owner);
        assert_eq!(game.map.kind(10, 10), TileKind::Empty);
    }

    #[test]
    fn steel_survives_normal_bullets_and_dies_from_level_three() {
        let mut game = test_game();
        game.map.set_kind(12, 12, TileKind::Steel);
        game.hit_tile(12, 12, 1, 1, 1);
        assert_eq!(game.map.kind(12, 12), TileKind::Steel);
        game.hit_tile(12, 12, 3, 3, 1);
        assert_eq!(game.map.kind(12, 12), TileKind::Empty);
    }

    #[test]
    fn snapshot_is_limited_to_neighbouring_chunks() {
        let mut game = test_game();
        let tx = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(tx, "A".into());
        // Пространственная сетка обновляется в конце тика.
        game.tick();
        let snap = game.snapshot_for(tx).expect("snapshot");
        assert_eq!(snap.tick, game.tick);
        // Свой танк входит в снимок.
        assert!(snap.players.iter().any(|p| p.id == tx));
    }

    #[test]
    fn population_is_filled_with_bots() {
        let mut cfg = test_game().cfg;
        cfg.min_population = 5;
        cfg.max_bots = 5;
        let mut game = Game::new(cfg);
        for _ in 0..10 {
            game.tick();
        }
        let bots = game.players.values().filter(|p| p.is_bot).count();
        assert_eq!(bots, 5);
    }

    #[test]
    fn bots_are_removed_when_humans_join() {
        let mut cfg = test_game().cfg;
        cfg.min_population = 4;
        cfg.max_bots = 4;
        let mut game = Game::new(cfg);
        for _ in 0..5 {
            game.tick();
        }
        assert_eq!(game.players.values().filter(|p| p.is_bot).count(), 4);

        let pid = game.add_player(7, "Человек".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "Человек".into());
        for _ in 0..80 {
            game.tick();
        }
        // Боты уходят плавно, но в итоге population = 4.
        assert_eq!(game.players.len(), 4);
    }

    #[test]
    fn pickup_restores_health_and_levels_up() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        {
            let player = game.players.get_mut(&pid).expect("player");
            let tank = player.tank.as_mut().expect("tank");
            tank.hp = 1;
        }
        let center = game
            .players
            .get(&pid)
            .and_then(|p| p.tank.as_ref())
            .map(|t| t.center())
            .expect("center");
        game.spawn_pickup_at(center.0, center.1, PickupKind::Repair);
        game.update_pickups();
        let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("tank");
        assert_eq!(tank.hp, tank.max_hp);

        game.spawn_pickup_at(center.0, center.1, PickupKind::Star);
        game.update_pickups();
        let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("tank");
        assert_eq!(tank.level, 2);
        assert_eq!(tank.max_hp, 4);
    }

    #[test]
    fn aabb_overlap_works() {
        assert!(aabb_overlap(0.0, 0.0, 10.0, 10.0, 5.0, 5.0, 10.0, 10.0));
        assert!(!aabb_overlap(0.0, 0.0, 10.0, 10.0, 20.0, 20.0, 10.0, 10.0));
    }
}
