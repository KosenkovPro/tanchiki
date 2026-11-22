//! Авторитарный игровой цикл: физика, коллизии, стрельба, боты, лут и прокачка.
//!
//! Сервер — единственный источник истины. Клиент только присылает ввод,
//! а сервер рассчитывает всё остальное и рассылает снимки мира.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::Sender;

use crate::map::{
    layer_elev, ramp_dir, ramp_high_layer, ramp_index, ramp_low_layer, GameMap, Rng, SpatialGrid,
    TileKind, CHUNK_TILES, ELEV_GROUND, ELEV_UPPER, LAYER_GROUND, LAYER_PIT, LAYER_UPPER, MAP_H,
    MAP_W, RAMP_DIR_NONE, RAMP_LEN, TILE_SIZE,
};
use crate::protocol::{
    self, BulletView, Direction, EffectView, GameEvent, InputFlags, LeaderboardEntry, PlayerView,
    PickupView, Snapshot, SoundView, TileUpdate,
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
pub const BOT_NAMES: [&str; 24] = [
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
    "Метеор",
    "Барс",
    "Грифон",
    "Змей",
    "Орел",
    "Самурай",
    "Скорпион",
    "Ураган",
    "Ястреб",
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

/// Задержка между выстрелами для уровня прокачки, в секундах.
///
/// Базовый танк стреляет заметно медленнее, чем раньше (1.65 с против
/// 0.55 с), но с каждой собранной звездой перезарядка сокращается.
pub fn level_fire_cooldown(level: u8) -> f32 {
    1.65 - 0.30 * level.saturating_sub(1) as f32
}

/// Сколько стволов у танка: на втором уровне появляется вторая пушка.
pub fn level_guns(level: u8) -> usize {
    if level >= 2 {
        2
    } else {
        1
    }
}

/// Расстояние между стволами двойной пушки (в пикселях).
pub const GUN_OFFSET: f32 = 7.0;

/// Сколько снарядов может лететь одновременно.
///
/// Предел не должен быть жёстче перезарядки: время жизни снаряда (до
/// попадания в стену) местами больше когорты, и тогда темп стрельбы
/// задавался бы не перезарядкой, а этим ограничением. Поэтому в полёте
/// всегда держится запас снарядов — темп задаёт `level_fire_cooldown`.
pub fn level_max_bullets(level: u8) -> usize {
    level_guns(level) * 3
}

/// Максимальное здоровье для уровня прокачки.
pub fn level_max_hp(level: u8) -> u8 {
    2 + level.clamp(1, 3)
}

/// Поражает ли настильный снаряд слоя `bullet_layer` танк слоя `target_layer`.
///
/// Правила высоты:
///  - свой слой — всегда;
///  - с возвышенности (`+1`) достаёт землю (`0`): снаряды не цепляются за
///    края платформ, и сверху видно землю;
///  - в ямку (`-1`) настильный огонь **не достаёт** ни с земли, ни сверху:
///    попасть можно только навесным оружием (миномёт, авиаудар);
///  - из ямки настильный огонь бьёт только внутри своей низины.
pub fn flat_shot_hits(bullet_layer: u8, target_layer: u8) -> bool {
    let bullet_elev = layer_elev(bullet_layer);
    let target_elev = layer_elev(target_layer);
    bullet_elev == target_elev
        || (bullet_elev == ELEV_UPPER && target_elev == ELEV_GROUND)
}

// ---------------------------------------------------------------------------
// Обзор (туман войны)
// ---------------------------------------------------------------------------

/// Дальность обзора в пикселях: столько видно по конусу взгляда.
pub const VISION_RANGE: f32 = 900.0;

/// Половина угла обзора в радианах: 60°, то есть конус в 120°.
pub const VISION_HALF_ANGLE: f32 = std::f32::consts::PI / 3.0;

/// Косая косинуса половины угла обзора — порог угла между взглядом и целью.
///
/// Ровно 0.5 — это cos(60°). Связь с углом проверяется в отладочной сборке,
/// чтобы случайная правка одного из двух чисел не исказила конус.
pub const VISION_COS: f32 = 0.5;

// ---------------------------------------------------------------------------
// Способности
// ---------------------------------------------------------------------------

/// Кулдаун миномёта (в тиках).
pub const MORTAR_COOLDOWN: u64 = 180;

/// Кулдаун авиаподдержки (в тиках).
pub const AIR_STRIKE_COOLDOWN: u64 = 450;

/// Во сколько раз миномётный снаряд медленнее прямого выстрела из пушки.
///
/// Требование ТЗ: «скорость полёта выстрела из гранатомёта должна быть в два
/// раза медленнее, чем скорость прямого выстрела из пушки». Поэтому время
/// полёта считается от дистанции и скорости пули, а не задаётся константой.
pub const MORTAR_SPEED_RATIO: f32 = 0.5;

/// Минимальное время полёта миномёта (в тиках) — для выстрела в упор.
pub const MORTAR_MIN_FLIGHT_TICKS: u64 = 15;

/// Максимальное время полёта миномёта (в тиках) — ограничение на дальность.
pub const MORTAR_MAX_FLIGHT_TICKS: u64 = 300;

/// Дальность слышимости выстрелов и взрывов (в пикселях).
///
/// Звук — единственный канал, который намеренно не скрывается конусом
/// обзора: по нему игрок слышит, с какой стороны атакуют.
pub const SOUND_HEAR_RANGE: f32 = 1800.0;

/// Звук выстрела.
pub const SOUND_SHOT: u8 = 0;
/// Звук взрыва.
pub const SOUND_EXPLOSION: u8 = 1;
/// Звук попадания.
pub const SOUND_HIT: u8 = 2;

/// Задержка удара авиаподдержки (в тиках).
pub const AIR_STRIKE_DELAY: u64 = 75;

/// Радиус поражения миномёта.
pub const MORTAR_RADIUS: f32 = 48.0;

/// Радиус поражения авиаудара.
pub const AIR_STRIKE_RADIUS: f32 = 66.0;

/// Урон миномёта.
pub const MORTAR_DAMAGE: u8 = 2;

/// Урон авиаудара.
pub const AIR_STRIKE_DAMAGE: u8 = 3;

/// Максимальная дальность применения способности от танка.
pub const ABILITY_RANGE: f32 = 1400.0;

// ---------------------------------------------------------------------------
// Сложность ботов
// ---------------------------------------------------------------------------

/// Уровень сложности ИИ-бота.
///
/// Главное правило: **бот всегда медленнее игрока**. Раньше боты делили с
/// игроком одну и ту же скорость, да ещё и качались от звёзд — отсюда
/// ощущение, что враги быстрее и сильнее. Теперь у каждого уровня свои
/// скорость, меткость, реакция и темп стрельбы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BotSkill {
    /// Новичок: медленный, мажет, реагирует с задержкой.
    Easy = 0,
    /// Средний: заметно медленнее игрока, стреляет регулярно.
    Normal = 1,
    /// Сложный: быстрый, почти не мажет — но всё равно медленнее игрока.
    Hard = 2,
}

impl BotSkill {
    /// Множитель скорости: всегда меньше единицы.
    pub fn speed_mult(self) -> f32 {
        match self {
            BotSkill::Easy => 0.55,
            BotSkill::Normal => 0.72,
            BotSkill::Hard => 0.88,
        }
    }

    /// Ошибка наводки в шестнадцатых долях поворота башни.
    pub fn aim_error(self) -> i32 {
        match self {
            BotSkill::Easy => 3,
            BotSkill::Normal => 2,
            BotSkill::Hard => 1,
        }
    }

    /// Множитель перезарядки: чем больше, тем реже стреляет бот.
    pub fn fire_mult(self) -> f32 {
        match self {
            BotSkill::Easy => 1.8,
            BotSkill::Normal => 1.4,
            BotSkill::Hard => 1.15,
        }
    }

    /// Задержка реакции на цель (в тиках).
    pub fn reaction_ticks(self) -> u64 {
        match self {
            BotSkill::Easy => 36,
            BotSkill::Normal => 20,
            BotSkill::Hard => 10,
        }
    }

    /// Дальность обнаружения цели (в пикселях).
    pub fn sight_range(self) -> f32 {
        match self {
            BotSkill::Easy => 520.0,
            BotSkill::Normal => 700.0,
            BotSkill::Hard => 860.0,
        }
    }

    /// Человекочитаемое название уровня.
    pub fn label(self) -> &'static str {
        match self {
            BotSkill::Easy => "лёгкий",
            BotSkill::Normal => "средний",
            BotSkill::Hard => "сложный",
        }
    }

    /// Разбирает название уровня из переменной окружения.
    pub fn from_name(name: &str) -> Option<BotSkill> {
        match name.trim().to_ascii_lowercase().as_str() {
            "easy" | "лёгкий" | "легкий" | "0" => Some(BotSkill::Easy),
            "normal" | "средний" | "1" => Some(BotSkill::Normal),
            "hard" | "сложный" | "2" => Some(BotSkill::Hard),
            _ => None,
        }
    }
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
    /// Целевая численность ботов на карте.
    ///
    /// Боты держат карту живой: двадцать машин патрулируют и сходятся к
    /// игроку неспешно, каждый со своим темпом.
    pub bot_target: usize,
    /// Уровень сложности ботов.
    ///
    /// `None` — смешанный состав: часть ботов лёгкие, часть сложные.
    pub bot_skill: Option<BotSkill>,
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
            min_population: 20,
            max_bots: 24,
            bot_target: 20,
            bot_skill: None,
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
        if let Some(v) = env_u64("TANCHIKI_BOT_COUNT") {
            cfg.bot_target = v as usize;
        }
        if let Ok(v) = std::env::var("TANCHIKI_BOT_SKILL") {
            // `mixed` (по умолчанию) — состав ботов разный.
            if !v.trim().eq_ignore_ascii_case("mixed") && !v.trim().is_empty() {
                cfg.bot_skill = BotSkill::from_name(&v);
            }
        }
        match cfg.bot_skill {
            Some(skill) => tracing::info!(
                skill = skill.label(),
                "уровень сложности ботов задан переменной окружения"
            ),
            None => tracing::info!("сложность ботов смешанная: лёгкие, средние и сложные"),
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
    /// Направление, в которую «смотрит» танк: по нему наводятся стволы.
    ///
    /// У человека совпадает с направлением движения, у бота — с направлением
    /// на цель (бот может идти прямо и стрелять по диагонали).
    pub dir: Direction,
    /// Направление башни: 16 направлений (шаг 22.5°).
    ///
    /// У человека башня смотрит туда, куда стоит курсор мыши, и не зависит
    /// от направления корпуса: можно ехать вбок и целиться в другую сторону.
    pub aim: u8,
    /// Направление движения: скорость направлена вдоль него.
    pub move_dir: Direction,
    /// Скорость вдоль текущего направления движения.
    pub vel: f32,
    /// Уровень прокачки 1..=3.
    pub level: u8,
    /// Ярус (этаж по высоте), на котором находится танк.
    pub deck: u8,
    /// Направление полосы пандуса, по которой едет танк.
    ///
    /// `RAMP_DIR_NONE` — танк не на пандусе. Направление хранится как в тайле
    /// (0 — вправо), поэтому «нет полосы» вынесено в отдельное значение.
    pub ramp_dir: u8,
    /// Текущий номер тайла в полосе пандуса: 0 — низ, 2 — верх.
    pub ramp_index: u8,
    /// Номер тайла полосы, на котором меняется ярус: -1 — ещё не определено.
    pub ramp_target: i8,
    /// Менялся ли уже ярус в этом заезде по пандусу.
    ///
    /// Нужен, чтобы танк, остановившийся на верхнем тайле полосы, не
    /// щёлкнул обратно на соседнем тике.
    pub ramp_switched: bool,
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
            aim: 4,
            move_dir: Direction::Down,
            vel: 0.0,
            level,
            deck: crate::map::LAYER_GROUND,
            ramp_dir: RAMP_DIR_NONE,
            ramp_index: 0,
            ramp_target: -1,
            ramp_switched: false,
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
    /// Ярус, на котором лежит бонус.
    pub deck: u8,
}

/// Снаряд.
#[derive(Debug, Clone, Copy)]
pub struct Bullet {
    pub id: u32,
    /// Игрок-владелец (по нему нельзя попасть в свой снаряд).
    pub owner: u32,
    pub x: f32,
    pub y: f32,
    /// Направление полёта: 16 направлений (шаг 22.5°).
    pub dir: u8,
    /// Ярус, на котором летит снаряд.
    pub deck: u8,
    pub level: u8,
    pub damage: u8,
    pub speed: f32,
}

/// Миномётный снаряд в полёте: летит по дуге и перелетает стены.
#[derive(Debug, Clone, Copy)]
pub struct Mortar {
    pub id: u32,
    pub owner: u32,
    /// Точка запуска.
    pub x0: f32,
    pub y0: f32,
    /// Точка падения.
    pub x1: f32,
    pub y1: f32,
    /// Тик запуска.
    pub start_tick: u64,
    /// Длительность полёта в тиках.
    pub flight_ticks: u64,
    /// Ярус, с которого запущен снаряд.
    pub deck: u8,
}

/// Вызов авиаподдержки: удар приходит в точку с задержкой.
#[derive(Debug, Clone, Copy)]
pub struct AirStrike {
    pub id: u32,
    pub owner: u32,
    pub x: f32,
    pub y: f32,
    /// Тик, в который приходится удар.
    pub hit_tick: u64,
    /// Ярус, по которому бьёт удар.
    pub deck: u8,
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
    /// Направление башни на цель: 16 направлений.
    pub aim: Option<u8>,
    /// Уровень сложности.
    pub skill: BotSkill,
    /// Тик, в котором бот заметил текущую цель: реакция не мгновенная.
    pub target_seen_at: u64,
    /// Ошибка наводки в шагах башни (1/16 оборота): у «лёгких» ботов больше.
    pub aim_error: i32,
    /// Личный множитель скорости бота: 0.55..0.9 от базовой.
    ///
    /// Боты не должны наваливаться толпой: у каждого свой темп, поэтому
    /// сходятся они неспешно и не одновременно.
    pub speed_bias: f32,
    /// Личная задержка реакции на цель в тиках.
    pub react_delay: u64,
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
    /// Направление башни: 16 направлений (у человека — по курсору мыши).
    pub aim: u8,
    /// Выбранная способность (миномёт, авиаподдержка).
    pub ability: u8,
    /// Мировая точка, куда применяется способность.
    pub target_x: f32,
    pub target_y: f32,
    /// Тик, начиная с которого снова доступен миномёт.
    pub mortar_ready_at: u64,
    /// Тик, начиная с которого снова доступна авиаподдержка.
    pub airstrike_ready_at: u64,
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
    /// Миномётные снаряды в полёте.
    pub mortars: HashMap<u32, Mortar>,
    /// Запросы авиаподдержки.
    pub airstrikes: HashMap<u32, AirStrike>,
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
    /// Звуковые события текущего тика.
    ///
    /// Их слышно даже вне конуса обзора — по ним игрок понимает, с какой
    /// стороны атакуют.
    pub sounds: Vec<SoundView>,
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
        for level in 0..map.levels() {
            let rle = map.rle_encode_level(level);
            match GameMap::rle_decode(&rle, map.width(), map.height()) {
                Some(decoded) if decoded.len() == map.len() => {}
                _ => tracing::warn!("RLE-карта яруса {level} не проходит проверку round-trip"),
            }
        }

        let total = (cfg.map_w * cfg.map_h) as usize;
        Game {
            cfg,
            map,
            players: HashMap::new(),
            bullets: HashMap::new(),
            pickups: HashMap::new(),
            mortars: HashMap::new(),
            airstrikes: HashMap::new(),
            tick: 0,
            rng: Rng::new(0x51DE_2026_BEEF),
            next_entity_id: 1,
            player_grid: SpatialGrid::new(),
            bullet_grid: SpatialGrid::new(),
            pickup_grid: SpatialGrid::new(),
            leaderboard: Vec::new(),
            tile_updates: Vec::new(),
            events: Vec::new(),
            sounds: Vec::new(),
            path_parent: vec![u32::MAX; total],
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
        self.sounds.clear();

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
        self.update_effects();
        let t_effects = phase.elapsed();

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
                effects = ?t_effects,
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
        let (input, cur_move, level, vel, skill, aim, bot_bias, bot_target_pos) = {
            let Some(player) = self.players.get(&pid) else {
                return;
            };
            let Some(tank) = player.tank.as_ref() else {
                return;
            };
            let skill = player.bot.as_ref().map(|b| b.skill);
            let bias = player.bot.as_ref().map(|b| b.speed_bias).unwrap_or(1.0);
            // Позиция цели бота: к ней он сближается.
            let target_pos = player.bot.as_ref().and_then(|b| b.target).and_then(|tid| {
                self.players.get(&tid).and_then(|p| p.tank.as_ref()).map(|t| t.center())
            });
            (
                player.input,
                tank.move_dir,
                tank.level,
                tank.vel,
                skill,
                player.aim,
                bias,
                target_pos,
            )
        };

        // Направление движения выводится из осей ввода: диагональ получается
        // сама собой, когда нажаты вертикальная и горизонтальная клавиши.
        let (av, ah) = input.axes();
        let desired_move = Direction::from_axes(av, ah);
        let turned = desired_move.is_some() && desired_move != Some(cur_move);

        let Some(mut tank) = self.players.get_mut(&pid).and_then(|p| p.tank.take()) else {
            return;
        };

        let on_ice = self.is_on_ice(tank.x, tank.y, tank.deck);
        // Боты всегда медленнее игрока — иначе они ощущаются сильнее человека.
        let mut speed_mult = skill.map(|s| s.speed_mult()).unwrap_or(1.0);
        if skill.is_some() {
            // Личный темп бота: у каждого своя скорость, поэтому сходятся они
            // неспешно и не толпой.
            speed_mult *= bot_bias;
            // Медленное сближение: чем ближе цель, тем тише едем.
            if let Some((tx, ty)) = bot_target_pos {
                let dx = tx - (tank.x + TANK_SIZE * 0.5);
                let dy = ty - (tank.y + TANK_SIZE * 0.5);
                if (dx * dx + dy * dy).sqrt() < 260.0 {
                    speed_mult *= 0.65;
                }
            }
        }
        let speed = level_speed(level) * speed_mult;
        let accel = if on_ice { 220.0 } else { 1100.0 };
        let decel = if on_ice { 120.0 } else { 1800.0 };

        // Целевая скорость: вперёд по текущему направлению движения.
        let target = if desired_move.is_some() { speed } else { 0.0 };

        let base_vel = if turned { 0.0 } else { vel };
        let delta = target - base_vel;
        let rate = if delta > 0.0 { accel } else { decel };
        let max_step = rate * TICK_DT;
        let mut new_vel = base_vel + delta.clamp(-max_step, max_step);
        if new_vel < 1.0 {
            new_vel = 0.0;
        }

        if turned {
            let next = desired_move.unwrap_or(cur_move);
            // Выравнивание по сетке — только между кардинальными
            // направлениями: диагональный ход идёт свободно.
            let was_cardinal = !cur_move.is_diagonal();
            let now_cardinal = !next.is_diagonal();
            tank.move_dir = next;
            tank.vel = 0.0;
            if now_cardinal && was_cardinal {
                self.align_to_grid(&mut tank, next);
            }
        } else if let Some(next) = desired_move {
            tank.move_dir = next;
        }

        // Корпус смотрит туда, куда едет. Башня — независимо от корпуса:
        // у человека по курсору мыши, у бота на цель.
        tank.dir = desired_move.unwrap_or(tank.dir);
        tank.aim = aim % protocol::AIM_COUNT;
        tank.vel = new_vel;

        let dist = new_vel * TICK_DT;
        if dist > 0.001 {
            self.move_tank(&mut tank, dist);
            tank.track_phase += new_vel * TICK_DT / 6.0;
        }

        // Трамплин меняет ярус: заехали на переход — оказались на другом этаже.
        self.update_deck(&mut tank);

        if let Some(player) = self.players.get_mut(&pid) {
            player.tank = Some(tank);
        }
    }

    /// Приводит поперечную ось танка к сетке тайлов.
    fn align_to_grid(&self, tank: &mut Tank, dir: Direction) {
        let vertical = dir.is_vertical();
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
            if x >= 0.0 && y >= 0.0 && self.map.rect_is_free(x, y, TANK_SIZE, tank.deck) {
                tank.x = x;
                tank.y = y;
                return;
            }
        }
    }

    /// Перемещает танк пошагово, останавливаясь на препятствии.
    fn move_tank(&self, tank: &mut Tank, dist: f32) {
        if dist <= 0.0 {
            return;
        }
        let (dx, dy) = tank.move_dir.delta();
        let steps = dist.ceil().max(1.0) as i32;
        let step = dist / steps as f32;
        for _ in 0..steps {
            let nx = tank.x + dx * step;
            let ny = tank.y + dy * step;
            if !self.map.rect_is_free(nx, ny, TANK_SIZE, tank.deck) {
                tank.vel = 0.0;
                return;
            }
            tank.x = nx;
            tank.y = ny;
        }
    }

    /// Находится ли танк на скользком тайле.
    fn is_on_ice(&self, x: f32, y: f32, deck: u8) -> bool {
        let (tx, ty) = self.map.tile_at(x + TANK_SIZE * 0.5, y + TANK_SIZE * 0.5);
        self.map.kind(tx, ty, deck).is_slippery()
    }

    /// Следит за пандусом и переключает слой в конце заезда.
    ///
    /// Пандус — полоса из трёх тайлов: у полосы есть **низкая сторона**
    /// (земля для подъёма, ямка для спуска) и **высокая**. Танк на низкой
    /// стороне едет к краю высокой (номер 2) и там переходит на высокий слой;
    /// на высокой стороне — наоборот, к краю низкой (номер 0).
    ///
    /// Так заезд работает и вверх, и вниз: полоса одна и та же, а переход
    /// происходит на её краю — выглядит как спуск, а не как скачок между этажами.
    fn update_deck(&mut self, tank: &mut Tank) {
        let (tx, ty) = self.map.tile_at(tank.x + TANK_SIZE * 0.5, tank.y + TANK_SIZE * 0.5);
        if !self.map.is_ramp(tx, ty, tank.deck) {
            // Сход с пандуса: следующий заезд начнётся заново.
            tank.ramp_dir = RAMP_DIR_NONE;
            tank.ramp_index = 0;
            tank.ramp_target = -1;
            tank.ramp_switched = false;
            return;
        }
        let value = self.map.value(tx, ty, tank.deck);
        let dir = ramp_dir(value);
        let idx = ramp_index(value);
        if tank.ramp_dir == RAMP_DIR_NONE {
            // Только что въехали на пандус: запоминаем полосу и наш тайл в ней.
            tank.ramp_dir = dir;
            tank.ramp_index = idx;
            tank.ramp_switched = false;
        } else if idx != tank.ramp_index {
            // Танк поехал по полосе в другую сторону — это новый заезд,
            // счётчик переключений сбрасываем.
            tank.ramp_switched = false;
        }
        // Цель перехода задаёт сторона полосы, а не направление езда: на низкой
        // стороне цель — край высокой, на высокой — край низкой.
        let low = ramp_low_layer(value);
        let high = ramp_high_layer(value);
        tank.ramp_target = if tank.deck == low {
            RAMP_LEN as i8 - 1
        } else {
            0
        };
        // Переход — только если танк реально проехал полосу: въехал с одного
        // края и доехал до другого. Поэтому перед крайним тайлом полосы должен
        // быть средний: наехать сбоку на край пандуса — не переход, иначе
        // игрок «проваливался» на другой уровень без заезда.
        let prev_index = tank.ramp_index as i8;
        if idx as i8 == tank.ramp_target
            && prev_index == RAMP_LEN as i8 - 2
            && !tank.ramp_switched
        {
            let next = if tank.deck == low { high } else { low };
            // Переключаемся только если на новом слое тоже можно стоять.
            if self.map.rect_is_free(tank.x, tank.y, TANK_SIZE, next) {
                tank.deck = next;
                tank.ramp_switched = true;
            }
        }
        tank.ramp_index = idx;
    }

    fn update_player_fire(&mut self, pid: u32) {
        let (fire, level, aim, cx, cy, fire_mult, deck) = {
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
            // У ботов перезарядка длиннее: чем выше сложность, тем чаще они стреляют.
            let fire_mult = player.bot.as_ref().map(|b| b.skill.fire_mult()).unwrap_or(1.0);
            let c = tank.center();
            (true, tank.level, tank.aim, c.0, c.1, fire_mult, tank.deck)
        };
        if !fire {
            return;
        }

        let guns = level_guns(level);
        let alive = self.bullets.values().filter(|b| b.owner == pid).count();
        if alive + guns > level_max_bullets(level) {
            return;
        }

        // Снаряды летят по направлению башни, а не корпуса.
        let (dx, dy) = protocol::aim_delta(aim);
        // Перпендикуляр к направлению выстрела: по нему разводятся стволы.
        let (px, py) = (-dy, dx);

        for gun in 0..guns {
            // Стволы симметричны относительно продольной оси танка.
            let offset = match guns {
                1 => 0.0,
                _ if gun == 0 => -GUN_OFFSET,
                _ => GUN_OFFSET,
            };
            let muzzle_x = cx + dx * (TANK_SIZE * 0.5 + 3.0) + px * offset;
            let muzzle_y = cy + dy * (TANK_SIZE * 0.5 + 3.0) + py * offset;
            // Снаряд появляется у дула; если там стена — из центра танка.
            let (bx, by) = if self.map.rect_is_free(
                muzzle_x - BULLET_SIZE * 0.5,
                muzzle_y - BULLET_SIZE * 0.5,
                BULLET_SIZE,
                deck,
            ) {
                (muzzle_x, muzzle_y)
            } else {
                (cx + px * offset, cy + py * offset)
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
                    dir: aim,
                    deck,
                    level,
                    damage: level_bullet_damage(level),
                    speed: level_bullet_speed(level),
                },
            );
        }

        // Звук выстрела слышат и те, кто не видит стрелка.
        self.emit_sound(cx, cy, SOUND_SHOT, pid, deck);

        if let Some(player) = self.players.get_mut(&pid) {
            if let Some(tank) = player.tank.as_mut() {
                tank.cooldown = level_fire_cooldown(level) * fire_mult;
                tank.revealed_until = self.tick + REVEAL_TICKS;
            }
        }
    }

    /// Кладёт звуковое событие в очередь текущего тика.
    fn emit_sound(&mut self, x: f32, y: f32, kind: u8, owner: u32, deck: u8) {
        let id = self.next_entity_id;
        self.next_entity_id += 1;
        self.sounds.push(SoundView {
            id,
            x,
            y,
            kind,
            owner,
            deck,
        });
    }

    // -----------------------------------------------------------------------
    // Снаряды
    // -----------------------------------------------------------------------

    fn update_bullets(&mut self) {
        let bullets = std::mem::take(&mut self.bullets);
        let mut kept: HashMap<u32, Bullet> = HashMap::with_capacity(bullets.len());
        for mut b in bullets.into_values() {
            let (dx, dy) = protocol::aim_delta(b.dir);
            b.x += dx * b.speed * TICK_DT;
            b.y += dy * b.speed * TICK_DT;

            if b.x < 0.0 || b.y < 0.0 || b.x >= self.cfg.world_w() || b.y >= self.cfg.world_h() {
                continue;
            }

            let (tx, ty) = self.map.tile_at(b.x, b.y);
            if self.map.kind(tx, ty, b.deck).blocks_bullet() {
                self.hit_tile(tx, ty, b.level, b.damage, b.owner, b.deck);
                continue;
            }

            if let Some(hit) = self.find_tank_hit(&b) {
                // Звук попадания слышно и тогда, когда противника не видно.
                self.emit_sound(b.x, b.y, SOUND_HIT, b.owner, b.deck);
                self.apply_damage(hit, b.damage, b.owner);
                continue;
            }

            kept.insert(b.id, b);
        }
        self.bullets = kept;
    }

    // -----------------------------------------------------------------------
    // Способности: миномёты и авиаподдержка
    // -----------------------------------------------------------------------

    /// Обновляет миномёты в полёте и запросы авиаподдержки.
    fn update_effects(&mut self) {
        let now = self.tick;

        // Миномёты: летят по дуге, перелетают стены, падают в точку прицела.
        let mortars = std::mem::take(&mut self.mortars);
        let mut kept_mortars: HashMap<u32, Mortar> = HashMap::with_capacity(mortars.len());
        for m in mortars.into_values() {
            let elapsed = now.saturating_sub(m.start_tick);
            if elapsed >= m.flight_ticks {
                self.explode(m.x1, m.y1, MORTAR_RADIUS, MORTAR_DAMAGE, m.owner, m.deck);
                continue;
            }
            kept_mortars.insert(m.id, m);
        }
        self.mortars = kept_mortars;

        // Авиаподдержка: удар приходит с задержкой — сначала виден вызов.
        let strikes = std::mem::take(&mut self.airstrikes);
        let mut kept_strikes: HashMap<u32, AirStrike> = HashMap::with_capacity(strikes.len());
        for s in strikes.into_values() {
            if now >= s.hit_tick {
                self.explode(s.x, s.y, AIR_STRIKE_RADIUS, AIR_STRIKE_DAMAGE, s.owner, s.deck);
                continue;
            }
            kept_strikes.insert(s.id, s);
        }
        self.airstrikes = kept_strikes;
    }

    /// Взрыв в точке: урон по танкам в радиусе и разрушение кирпича.
    ///
    /// Взрыв — **навесное** оружие: он достаёт танки на любой высоте
    /// (в том числе в ямке, куда настильный огонь не дотягивается) и
    /// разрушает кирпич на всех слоях.
    fn explode(&mut self, x: f32, y: f32, radius: f32, damage: u8, owner: u32, deck: u8) {
        let r2 = radius * radius;
        let hit: Vec<u32> = self
            .players
            .iter()
            .filter(|(_, p)| {
                p.tank.as_ref().is_some_and(|t| {
                    let (cx, cy) = t.center();
                    let dx = cx - x;
                    let dy = cy - y;
                    dx * dx + dy * dy <= r2
                })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in hit {
            self.apply_damage(id, damage, owner);
        }

        // Кирпичные стены рядом с эпицентром получают повреждения — на всех
        // слоях: взрыв бьёт и сквозь настил моста, и сквозь стенку ямки.
        let (tx0, ty0) = self.map.tile_at(x - radius, y - radius);
        let (tx1, ty1) = self.map.tile_at(x + radius, y + radius);
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let cx = tx as f32 * TILE_SIZE + TILE_SIZE * 0.5;
                let cy = ty as f32 * TILE_SIZE + TILE_SIZE * 0.5;
                let dx = cx - x;
                let dy = cy - y;
                if dx * dx + dy * dy > r2 {
                    continue;
                }
                for layer in [LAYER_GROUND, LAYER_UPPER, LAYER_PIT] {
                    if self.map.kind(tx, ty, layer) == crate::map::TileKind::Brick {
                        self.hit_tile(tx, ty, 3, damage, owner, layer);
                    }
                }
            }
        }

        self.events.push(GameEvent::Explosion {
            x,
            y,
            size: (radius / 8.0).round().clamp(1.0, 24.0) as u8,
            deck,
        });
        // Взрыв слышен и за пределами конуса обзора.
        self.emit_sound(x, y, SOUND_EXPLOSION, owner, deck);
    }

    /// Обрабатывает попадание снаряда в тайл.
    fn hit_tile(&mut self, tx: u32, ty: u32, level: u8, damage: u8, owner: u32, deck: u8) {
        let kind = self.map.kind(tx, ty, deck);
        match kind {
            TileKind::Brick => {
                let dmg = self.map.damage(tx, ty, deck).saturating_add(damage);
                if dmg >= crate::map::BRICK_MAX_DAMAGE {
                    self.map.set_kind(tx, ty, deck, TileKind::Empty);
                    self.events.push(GameEvent::TileBroken {
                        x: tx as u16,
                        y: ty as u16,
                        deck,
                    });
                    if let Some(player) = self.players.get_mut(&owner) {
                        player.score += BRICK_SCORE;
                    }
                } else {
                    self.map.set_value(
                        tx,
                        ty,
                        deck,
                        TileKind::Brick.bits() | (dmg << 3),
                    );
                }
                let value = self.map.value(tx, ty, deck);
                self.tile_updates.push(TileUpdate {
                    x: tx as u16,
                    y: ty as u16,
                    value,
                    deck,
                });
            }
            TileKind::Steel => {
                // Бетон пробивается только снарядом максимального уровня.
                if level >= 3 {
                    self.map.set_kind(tx, ty, deck, TileKind::Empty);
                    self.events.push(GameEvent::TileBroken {
                        x: tx as u16,
                        y: ty as u16,
                        deck,
                    });
                    if let Some(player) = self.players.get_mut(&owner) {
                        player.score += BRICK_SCORE * 2;
                    }
                    let value = self.map.value(tx, ty, deck);
                    self.tile_updates.push(TileUpdate {
                        x: tx as u16,
                        y: ty as u16,
                        value,
                        deck,
                    });
                }
            }
            _ => {}
        }
    }

    /// Ищет танк, задетый снарядом.
    ///
    /// Правила настильного огня по высоте:
    ///  - снаряд бьёт танк на своём слое;
    ///  - снаряд с возвышенности (`+1`) достаёт землю (`0`) — преимущество
    ///    высоты, снаряды не цепляются за края платформ;
    ///  - настильный огонь **не достаёт** до ямки (`-1`): из низины и с
    ///    возвышенности попасть можно только навесным оружием;
    ///  - из ямки настильный огонь бьёт только внутри своей низины.
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
            if !flat_shot_hits(b.deck, tank.deck) {
                continue;
            }
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
            .map(|t| (t.center(), t.deck));

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

        if let Some((pos, deck)) = center {
            self.events.push(GameEvent::Explosion {
                x: pos.0,
                y: pos.1,
                size: 3,
                deck,
            });
            self.emit_sound(pos.0, pos.1, SOUND_EXPLOSION, by, deck);
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
            if let Some((pos, deck)) = center {
                self.spawn_pickup_at(pos.0, pos.1, PickupKind::Star, deck);
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
                (tank.x, tank.y, tank.deck)
            };
            let (tx, ty, deck) = rect;

            let mut collected: Option<Pickup> = None;
            self.pickups.retain(|_, p| {
                // Бонус подбирается только на своём ярусе.
                if p.deck != deck {
                    return true;
                }
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

    /// Ставит бонус в указанную точку на нужном ярусе.
    fn spawn_pickup_at(&mut self, x: f32, y: f32, kind: PickupKind, deck: u8) {
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
                deck,
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
            if self.map.blocks_tank(tx, ty, LAYER_GROUND)
                || self.map.kind(tx, ty, LAYER_GROUND) == TileKind::Forest
            {
                continue;
            }
            // Бонус не должен появляться прямо под танком.
            let cx = tx as f32 * TILE_SIZE + TILE_SIZE * 0.5;
            let cy = ty as f32 * TILE_SIZE + TILE_SIZE * 0.5;
            let occupied = self.players.values().any(|p| {
                p.tank.as_ref().is_some_and(|t| {
                    t.deck == LAYER_GROUND && (t.x - cx).abs() < 48.0 && (t.y - cy).abs() < 48.0
                })
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
        self.spawn_pickup_at(x, y, kind, LAYER_GROUND);
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
        let (bx, by, next_plan, prev_target, skill, deck) = {
            let Some(player) = self.players.get(&pid) else {
                return;
            };
            let Some(tank) = player.tank.as_ref() else {
                return;
            };
            let (next_plan, prev_target, skill) = player
                .bot
                .as_ref()
                .map(|b| (b.next_plan_tick, b.target, b.skill))
                .unwrap_or((0, None, BotSkill::Normal));
            (tank.x, tank.y, next_plan, prev_target, skill, tank.deck)
        };

        let now = self.tick;
        let replan = now >= next_plan;
        let mut target = prev_target;

        if replan {
            target = self.find_bot_target(pid, bx, by, skill, deck);
        }

        // Пересчитываем маршрут к цели.
        if replan {
            let mut path: Vec<(u32, u32)> = Vec::new();
            let mut crowded = false;
            if let Some(tid) = target {
                let goal = self
                    .players
                    .get(&tid)
                    .and_then(|p| p.tank.as_ref())
                    .filter(|t| t.deck == deck)
                    .map(|t| self.map.tile_at(t.x + TANK_SIZE * 0.5, t.y + TANK_SIZE * 0.5));
                if let Some(goal) = goal {
                    // Личное пространство: если у цели уже скопились другие
                    // боты, бот не лезет в толпу — он держит дистанцию.
                    let goal_center = (
                        goal.0 as f32 * crate::map::TILE_SIZE + crate::map::TILE_SIZE * 0.5,
                        goal.1 as f32 * crate::map::TILE_SIZE + crate::map::TILE_SIZE * 0.5,
                    );
                    crowded = self.crowd_near(goal_center.0, goal_center.1, 220.0, pid) >= 3;
                    if !crowded {
                        path = self
                            .find_path(
                                self.map.tile_at(bx + TANK_SIZE * 0.5, by + TANK_SIZE * 0.5),
                                goal,
                                deck,
                            )
                            .unwrap_or_default();
                    }
                }
            }
            // Цели нет — бот бродит по карте, чтобы мир не выглядел мёртвым.
            if path.is_empty() && (target.is_none() || crowded) {
                path = self.wander_path(bx, by, deck);
            }
            if let Some(player) = self.players.get_mut(&pid) {
                if let Some(bot) = player.bot.as_mut() {
                    bot.path = path;
                    bot.path_idx = 0;
                    // Новая цель — новое время обнаружения (реакция не мгновенная).
                    if target != bot.target {
                        bot.target_seen_at = now;
                    }
                    bot.target = target;
                    // Ошибка наводки пересчитывается вместе с маршрутом:
                    // иначе башня дёргалась бы на каждом тике.
                    let span = skill.aim_error() * 2 + 1;
                    bot.aim_error = if span > 0 {
                        self.rng.below(span as u32) as i32 - skill.aim_error()
                    } else {
                        0
                    };
                    // Перепланировки разводим по тикам: иначе все боты
                    // одновременно запускают поиск пути.
                    bot.next_plan_tick = now + 30 + (pid % 20) as u64;
                }
            }
        }

        // Принимаем решение: стрелять и куда двигаться.
        let mut input = InputFlags::default();
        let mut aim: Option<u8> = None;

        if let Some(tid) = target {
            let target_info = self
                .players
                .get(&tid)
                .and_then(|p| p.tank.as_ref())
                .map(|t| (t.center(), t.deck));
            if let Some((tc, target_deck)) = target_info {
                let bcx = bx + TANK_SIZE * 0.5;
                let bcy = by + TANK_SIZE * 0.5;
                // Видим цель — наводимся (с ошибкой по уровню сложности)
                // и стреляем, если реакция уже прошла.
                if target_deck == deck
                    && self.map.bullet_line_of_sight(bcx, bcy, tc.0, tc.1, deck)
                {
                    let error = self
                        .players
                        .get(&pid)
                        .and_then(|p| p.bot.as_ref())
                        .map(|b| b.aim_error)
                        .unwrap_or(0);
                    let base = aim_towards(bcx, bcy, tc.0, tc.1) as i32;
                    let dir = base + error;
                    aim = Some(dir.rem_euclid(protocol::AIM_COUNT as i32) as u8);
                    let seen_at = self
                        .players
                        .get(&pid)
                        .and_then(|p| p.bot.as_ref())
                        .map(|b| b.target_seen_at)
                        .unwrap_or(now);
                    // Реакция бota: сложность плюс личная задержка — боты
                    // реагируют не одновременно.
                    let react_delay = self
                        .players
                        .get(&pid)
                        .and_then(|p| p.bot.as_ref())
                        .map(|b| b.react_delay)
                        .unwrap_or(0);
                    if now >= seen_at + skill.reaction_ticks() + react_delay {
                        input.fire = true;
                    }
                }
            }
        }

        // Идём по маршруту: движение кардинальное, башня при этом
        // смотрит в другую сторону — так же, как у человека с мышью.
        {
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
            player.input = input;
            if let Some(bot) = player.bot.as_mut() {
                bot.aim = aim;
            }
            // Башня бота: держим последнее известное направление, если цели нет.
            if let Some(a) = aim {
                player.aim = a;
            }
        }
    }

    /// Сколько ботов скопилось вокруг точки.
    ///
    /// Правило личного пространства: если рядом с целью уже много ботов,
    /// новый не лезет — иначе на игрока наваливаются все разом.
    fn crowd_near(&self, x: f32, y: f32, radius: f32, except: u32) -> usize {
        self.players
            .values()
            .filter(|p| p.is_bot && p.id != except && p.removal_at.is_none())
            .filter(|p| {
                p.tank.as_ref().is_some_and(|t| {
                    let (cx, cy) = t.center();
                    let dx = cx - x;
                    let dy = cy - y;
                    dx * dx + dy * dy <= radius * radius
                })
            })
            .count()
    }

    /// Случайный маршрут для блуждания, когда поблизости нет цели.
    fn wander_path(&mut self, bx: f32, by: f32, deck: u8) -> Vec<(u32, u32)> {
        let from = self.map.tile_at(bx + TANK_SIZE * 0.5, by + TANK_SIZE * 0.5);
        for _ in 0..8 {
            let tx = self.rng.range(1, self.cfg.map_w.saturating_sub(1));
            let ty = self.rng.range(1, self.cfg.map_h.saturating_sub(1));
            if self.map.blocks_tank(tx, ty, deck) {
                continue;
            }
            if let Some(path) = self.find_path(from, (tx, ty), deck) {
                if !path.is_empty() {
                    return path;
                }
            }
        }
        Vec::new()
    }

    /// Выбирает ближайшую разумную цель для бота.
    ///
    /// Приоритет — цели, которые видно: так боты дерутся и друг с другом,
    /// а не только с игроками. Дальность обзора зависит от сложности:
    /// «лёгкий» бот в принципе не замечает врага за 800 пикселей.
    fn find_bot_target(
        &self,
        bot_id: u32,
        bx: f32,
        by: f32,
        skill: BotSkill,
        deck: u8,
    ) -> Option<u32> {
        let bcx = bx + TANK_SIZE * 0.5;
        let bcy = by + TANK_SIZE * 0.5;
        let sight = skill.sight_range();
        let mut best_visible: Option<(u32, f32)> = None;
        let mut best_any: Option<(u32, f32)> = None;
        for (pid, player) in self.players.iter() {
            if *pid == bot_id {
                continue;
            }
            let Some(tank) = player.tank.as_ref() else {
                continue;
            };
            // Противник на другом ярусе — за стеной этажа, его не видно.
            if tank.deck != deck {
                continue;
            }
            let tcx = tank.x + TANK_SIZE * 0.5;
            let tcy = tank.y + TANK_SIZE * 0.5;
            let dx = tcx - bcx;
            let dy = tcy - bcy;
            let dist = dx * dx + dy * dy;
            if dist > sight * sight {
                continue;
            }
            if best_any.is_none_or(|(_, bd)| dist < bd) {
                best_any = Some((*pid, dist));
            }
            if self.map.bullet_line_of_sight(bcx, bcy, tcx, tcy, deck)
                && best_visible.is_none_or(|(_, bd)| dist < bd)
            {
                best_visible = Some((*pid, dist));
            }
        }
        best_visible.or(best_any).map(|(id, _)| id)
    }

    /// Поиск пути по тайлам (BFS) в пределах одного яруса.
    fn find_path(
        &mut self,
        from: (u32, u32),
        to: (u32, u32),
        deck: u8,
    ) -> Option<Vec<(u32, u32)>> {
        let w = self.map.width() as usize;
        let h = self.map.height() as usize;
        if from.0 as usize >= w || from.1 as usize >= h {
            return None;
        }
        if to.0 as usize >= w || to.1 as usize >= h {
            return None;
        }
        if self.map.blocks_tank(to.0, to.1, deck) {
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
                if self.map.blocks_tank(nx as u32, ny as u32, deck) {
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
        let bots: Vec<u32> = self
            .players
            .values()
            .filter(|p| p.is_bot && p.removal_at.is_none())
            .map(|p| p.id)
            .collect();

        // Целевая численность ботов: держим карту живой, но не наваливаем
        // толпой — каждый бот идёт к игроку со своим темпом.
        let desired = self.cfg.bot_target.min(self.cfg.max_bots);

        if bots.len() < desired {
            self.spawn_bot();
        } else if bots.len() > desired {
            // Лишние боты уходят плавно: исчезают со взрывом, удаляются через 1.5 сек.
            if let Some(&id) = bots.first() {
                let pos = self
                    .players
                    .get(&id)
                    .and_then(|p| p.tank.as_ref())
                    .map(|t| (t.center(), t.deck));
                if let Some(player) = self.players.get_mut(&id) {
                    if player.removal_at.is_none() {
                        player.removal_at = Some(self.tick + 45);
                        player.tank = None;
                    }
                }
                if let Some((pos, deck)) = pos {
                    self.events.push(GameEvent::Explosion {
                        x: pos.0,
                        y: pos.1,
                        size: 2,
                        deck,
                    });
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
        tank.aim = match tank.dir {
            Direction::Up => 12,
            Direction::Down => 4,
            Direction::Left => 8,
            Direction::Right => 0,
            _ => 0,
        };
        let bot_aim = tank.aim;
        // Уровень сложности: либо задан на весь сервер, либо смешанный состав.
        let skill = match self.cfg.bot_skill {
            Some(skill) => skill,
            None => match self.rng.below(100) {
                0..=44 => BotSkill::Easy,
                45..=84 => BotSkill::Normal,
                _ => BotSkill::Hard,
            },
        };
        // Личный характер бота: свой темп и своя задержка реакции. Благодаря
        // этому двадцать ботов сходятся к игроку неспешно и не разом.
        let speed_bias = 0.55 + self.rng.unit() * 0.35;
        let react_delay: u64 = 4 + self.rng.below(22) as u64;        self.players.insert(
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
                aim: bot_aim,
                ability: protocol::ability::NONE,
                target_x: 0.0,
                target_y: 0.0,
                mortar_ready_at: 0,
                airstrike_ready_at: 0,
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
                    aim: None,
                    skill,
                    target_seen_at: 0,
                    aim_error: 0,
                    speed_bias,
                    react_delay,
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
                kills: p.kills,
                is_bot: p.is_bot,
            })
            .collect();
        // Главная метрика — число уничтоженных танков.
        entries.sort_by(|a, b| {
            b.kills
                .cmp(&a.kills)
                .then(b.score.cmp(&a.score))
                .then(a.id.cmp(&b.id))
        });
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
                        kills: player.kills,
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
                aim: 4,
                ability: protocol::ability::NONE,
                target_x: 0.0,
                target_y: 0.0,
                mortar_ready_at: 0,
                airstrike_ready_at: 0,
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
    ///
    /// Помимо клавиш приходит направление башни (по курсору мыши), выбранная
    /// способность и точка, куда её применить.
    pub fn apply_input(
        &mut self,
        conn_id: u64,
        seq: u32,
        input: InputFlags,
        aim: u8,
        ability: u8,
        target_x: f32,
        target_y: f32,
    ) {
        let ids: Vec<u32> = self
            .players
            .values()
            .filter(|p| p.conn_id == conn_id && !p.is_bot)
            .map(|p| p.id)
            .collect();
        for id in ids {
            let mut wants_ability = false;
            if let Some(player) = self.players.get_mut(&id) {
                if seq <= player.last_seq && player.last_seq != 0 {
                    // Старый или повторный пакет — игнорируем.
                    continue;
                }
                player.last_seq = seq;
                player.input = input;
                player.aim = aim % protocol::AIM_COUNT;
                player.ability = ability;
                player.target_x = target_x;
                player.target_y = target_y;
                wants_ability = input.use_ability;
            }
            if wants_ability {
                self.try_use_ability(id);
            }
        }
    }

    /// Пытается применить выбранную способность в точке курсора.
    ///
    /// Миномёт летит по дуге через стены, авиаподдержка бьёт с задержкой.
    /// У каждой способности свой кулдаун, а дальность ограничена.
    fn try_use_ability(&mut self, pid: u32) {
        let now = self.tick;
        let (ability, mortar_ready, airstrike_ready, origin, target, level, deck) = {
            let Some(player) = self.players.get(&pid) else {
                return;
            };
            let Some(tank) = player.tank.as_ref() else {
                return;
            };
            (
                player.ability,
                player.mortar_ready_at,
                player.airstrike_ready_at,
                tank.center(),
                (player.target_x, player.target_y),
                tank.level,
                tank.deck,
            )
        };

        if ability == protocol::ability::NONE {
            return;
        }
        // У каждой способности свой кулдаун.
        let ready_at = match ability {
            protocol::ability::MORTAR => mortar_ready,
            protocol::ability::AIR_STRIKE => airstrike_ready,
            _ => return,
        };
        if now < ready_at {
            return;
        }

        // Точка приведения приводится к границам мира: иначе миномёт
        // улетел бы за карту и пропал.
        let tx = target.0.clamp(0.0, self.cfg.world_w() - 1.0);
        let ty = target.1.clamp(0.0, self.cfg.world_h() - 1.0);
        let dx = tx - origin.0;
        let dy = ty - origin.1;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist > ABILITY_RANGE {
            return;
        }
        if dist < 1.0 {
            return;
        }

        match ability {
            protocol::ability::MORTAR => {
                // Скорость миномётного снаряда — вдвое меньше скорости пули,
                // поэтому время полёта считается от дистанции.
                let mortar_speed = level_bullet_speed(level) * MORTAR_SPEED_RATIO;
                let flight_ticks = ((dist / mortar_speed.max(1.0)) / TICK_DT)
                    .round()
                    .max(MORTAR_MIN_FLIGHT_TICKS as f32)
                    .min(MORTAR_MAX_FLIGHT_TICKS as f32) as u64;
                let id = self.next_entity_id;
                self.next_entity_id += 1;
                self.mortars.insert(
                    id,
                    Mortar {
                        id,
                        owner: pid,
                        x0: origin.0,
                        y0: origin.1,
                        x1: tx,
                        y1: ty,
                        start_tick: now,
                        flight_ticks,
                        deck,
                    },
                );
                if let Some(player) = self.players.get_mut(&pid) {
                    player.mortar_ready_at = now + MORTAR_COOLDOWN;
                }
            }
            protocol::ability::AIR_STRIKE => {
                let id = self.next_entity_id;
                self.next_entity_id += 1;
                self.airstrikes.insert(
                    id,
                    AirStrike {
                        id,
                        owner: pid,
                        x: tx,
                        y: ty,
                        hit_tick: now + AIR_STRIKE_DELAY,
                        deck,
                    },
                );
                if let Some(player) = self.players.get_mut(&pid) {
                    player.airstrike_ready_at = now + AIR_STRIKE_COOLDOWN;
                }
            }
            _ => return,
        }

        self.events.push(GameEvent::AbilityUsed {
            x: tx,
            y: ty,
            ability,
        });
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

        // Направление башни наблюдателя задаёт конус обзора.
        let (viewer_aim, alive, viewer_deck) = viewer
            .tank
            .as_ref()
            .map(|t| (t.aim, true, t.deck))
            .unwrap_or((0, false, LAYER_GROUND));

        // --- Танки ---
        for pid in self.player_grid.query(cx0, cy0, cx1, cy1) {
            let Some(player) = self.players.get(&pid) else {
                continue;
            };
            let Some(tank) = player.tank.as_ref() else {
                continue;
            };
            // На другом ярусе противников не видно: этаж перекрывает обзор.
            if tank.deck != viewer_deck {
                continue;
            }
            if !self.can_see(
                viewer_id,
                focus_x,
                focus_y,
                viewer_aim,
                alive,
                ccx,
                ccy,
                tank,
                viewer_deck,
            ) {
                continue;
            }
            snap.players.push(PlayerView {
                id: player.id,
                name: player.nickname.clone(),
                is_bot: player.is_bot,
                x: tank.x,
                y: tank.y,
                dir: tank.dir,
                aim: tank.aim,
                deck: tank.deck,
                level: tank.level,
                hp: tank.hp,
                max_hp: tank.max_hp,
                alive: true,
                shielded: tank.shield_until > self.tick,
                score: player.score,
                kills: player.kills,
                ability: player.ability,
                // Оба кулдауна присылаются всегда: панель показывает
                // готовность обеих способностей, а не только выбранной.
                mortar_cd: player
                    .mortar_ready_at
                    .saturating_sub(self.tick)
                    .min(u16::MAX as u64) as u16,
                airstrike_cd: player
                    .airstrike_ready_at
                    .saturating_sub(self.tick)
                    .min(u16::MAX as u64) as u16,
            });
        }

        // --- Снаряды ---
        // Свои выстрелы приходят всегда: их не должен терять ни фильтр
        // чанков, ни конус обзора — игрок видит свой залп целиком.
        let own: Vec<u32> = self
            .bullets
            .iter()
            .filter(|(_, b)| b.owner == viewer_id)
            .map(|(id, _)| *id)
            .collect();
        for bid in own {
            let Some(bullet) = self.bullets.get(&bid) else {
                continue;
            };
            snap.bullets.push(BulletView {
                id: bullet.id,
                x: bullet.x,
                y: bullet.y,
                dir: bullet.dir,
                deck: bullet.deck,
                level: bullet.level,
            });
        }
        for bid in self.bullet_grid.query(cx0, cy0, cx1, cy1) {
            let Some(bullet) = self.bullets.get(&bid) else {
                continue;
            };
            if bullet.owner == viewer_id {
                // Свой снаряд уже добавлен выше.
                continue;
            }
            if bullet.deck != viewer_deck {
                continue;
            }
            // Снаряд за пределами конуса не виден — как и противник.
            if alive && !self.in_vision(focus_x, focus_y, viewer_aim, bullet.x, bullet.y, viewer_deck)
            {
                continue;
            }
            snap.bullets.push(BulletView {
                id: bullet.id,
                x: bullet.x,
                y: bullet.y,
                dir: bullet.dir,
                deck: bullet.deck,
                level: bullet.level,
            });
        }

        // --- Бонусы ---
        for pid in self.pickup_grid.query(cx0, cy0, cx1, cy1) {
            let Some(pickup) = self.pickups.get(&pid) else {
                continue;
            };
            if pickup.deck != viewer_deck {
                continue;
            }
            snap.pickups.push(PickupView {
                id: pickup.id,
                x: pickup.x,
                y: pickup.y,
                kind: pickup.kind.as_u8(),
                ttl: (pickup.expires_at.saturating_sub(self.tick)) as u32,
                deck: pickup.deck,
            });
        }

        // --- Эффекты: миномёты в полёте и вызовы авиаподдержки ---
        for m in self.mortars.values() {
            if m.deck != viewer_deck {
                continue;
            }
            let (mx, my) = mortar_position(m, self.tick);
            if alive && !self.in_vision(focus_x, focus_y, viewer_aim, mx, my, viewer_deck) {
                continue;
            }
            // Клиенту нужны обе точки: из начала и в цель — по ним он
            // рисует дугу полёта.
            let phase = mortar_phase(m, self.tick);
            snap.effects.push(EffectView {
                id: m.id,
                kind: EFFECT_MORTAR,
                x: m.x0,
                y: m.y0,
                x2: m.x1,
                y2: m.y1,
                phase,
                deck: m.deck,
            });
        }
        for s in self.airstrikes.values() {
            if s.deck != viewer_deck {
                continue;
            }
            if alive && !self.in_vision(focus_x, focus_y, viewer_aim, s.x, s.y, viewer_deck) {
                continue;
            }
            let total = AIR_STRIKE_DELAY.max(1);
            let left = s.hit_tick.saturating_sub(self.tick);
            let phase = ((total.saturating_sub(left)) * 100 / total) as u8;
            snap.effects.push(EffectView {
                id: s.id,
                kind: EFFECT_AIR_STRIKE,
                x: s.x,
                y: s.y,
                x2: s.x,
                y2: s.y,
                phase,
                deck: s.deck,
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

        // --- Звуки ---
        //
        // Единственный канал, который намеренно не скрывается конусом
        // обзора: по нему игрок слышит, с какой стороны атакуют.
        let hear2 = SOUND_HEAR_RANGE * SOUND_HEAR_RANGE;
        for sound in self.sounds.iter() {
            let dx = sound.x - focus_x;
            let dy = sound.y - focus_y;
            if dx * dx + dy * dy > hear2 {
                continue;
            }
            snap.sounds.push(*sound);
        }

        // --- События ---
        for event in self.events.iter() {
            let (ex, ey) = match *event {
                GameEvent::Explosion { x, y, .. } => (x, y),
                GameEvent::PickupCollected { x, y, .. } => (x, y),
                GameEvent::AbilityUsed { x, y, .. } => (x, y),
                GameEvent::TileBroken { x, y, .. } => (
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

    /// Находится ли точка в конусе обзора наблюдателя.
    ///
    /// Конус шириной 120° вокруг направления башни, дальность — 900 пикселей.
    /// Кирпичные и бетонные стены перекрывают обзор, вода и лес — нет.
    pub fn in_vision(
        &self,
        vx: f32,
        vy: f32,
        aim: u8,
        x: f32,
        y: f32,
        deck: u8,
    ) -> bool {
        debug_assert!(
            (VISION_COS - VISION_HALF_ANGLE.cos()).abs() < 1e-6,
            "порог обзора разошёлся с углом конуса"
        );
        let dx = x - vx;
        let dy = y - vy;
        let dist2 = dx * dx + dy * dy;
        if dist2 > VISION_RANGE * VISION_RANGE {
            return false;
        }
        if dist2 < 1.0 {
            return true;
        }
        let (ax, ay) = protocol::aim_delta(aim);
        let dist = dist2.sqrt();
        let cos = (dx * ax + dy * ay) / dist;
        if cos < VISION_COS {
            return false;
        }
        self.map.bullet_line_of_sight(vx, vy, x, y, deck)
    }

    /// Виден ли танк наблюдателю (конус обзора, чаща и радиус чанков).
    fn can_see(
        &self,
        viewer_id: u32,
        vx: f32,
        vy: f32,
        aim: u8,
        alive: bool,
        ccx: u32,
        ccy: u32,
        tank: &Tank,
        deck: u8,
    ) -> bool {
        let (ocx, ocy) = self.map.chunk_at(tank.x + TANK_SIZE * 0.5, tank.y + TANK_SIZE * 0.5);
        let radius = crate::map::VIEW_CHUNK_RADIUS as i64;
        if (ocx as i64 - ccx as i64).abs() > radius || (ocy as i64 - ccy as i64).abs() > radius {
            return false;
        }

        // Главное правило тумана войны: противник виден только там,
        // куда «смотрит» башня наблюдателя.
        if alive
            && !self.in_vision(vx, vy, aim, tank.x + TANK_SIZE * 0.5, tank.y + TANK_SIZE * 0.5, deck)
        {
            return false;
        }

        // Танк в чаще не виден врагам снаружи лесного массива, пока не выстрелит.
        let (otx, oty) = self.map.tile_at(tank.x + TANK_SIZE * 0.5, tank.y + TANK_SIZE * 0.5);
        if let Some(region) = self.map.forest_region(otx, oty, deck) {
            if tank.revealed_until <= self.tick {
                let viewer_region = self
                    .players
                    .get(&viewer_id)
                    .and_then(|p| p.tank.as_ref())
                    .and_then(|t| {
                        let (tx, ty) = self.map.tile_at(t.x + TANK_SIZE * 0.5, t.y + TANK_SIZE * 0.5);
                        self.map.forest_region(tx, ty, deck)
                    });
                if viewer_region != Some(region) {
                    return false;
                }
            }
        }
        true
    }
}

/// Тип эффекта: миномёт в полёте.
pub const EFFECT_MORTAR: u8 = 1;
/// Тип эффекта: вызов авиаподдержки.
pub const EFFECT_AIR_STRIKE: u8 = 2;

/// Текущая позиция миномёта: он летит по прямой, высота дуги рисуется клиентом.
fn mortar_position(m: &Mortar, now: u64) -> (f32, f32) {
    let t = (now.saturating_sub(m.start_tick) as f32 / m.flight_ticks.max(1) as f32).clamp(0.0, 1.0);
    (m.x0 + (m.x1 - m.x0) * t, m.y0 + (m.y1 - m.y0) * t)
}

/// Ход полёта миномёта в процентах.
fn mortar_phase(m: &Mortar, now: u64) -> u8 {
    let t = (now.saturating_sub(m.start_tick) as f32 / m.flight_ticks.max(1) as f32).clamp(0.0, 1.0);
    (t * 100.0) as u8
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

/// Направление башни от точки к точке: 16 направлений (шаг 22.5°).
///
/// Это «точная» наводка, в отличие от `Direction`, который описывает
/// только перемещение корпуса.
pub fn aim_towards(x0: f32, y0: f32, x1: f32, y1: f32) -> u8 {
    protocol::aim_towards(x0, y0, x1, y1)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{
        ramp_dir_delta, ramp_value, RAMP_DIR_DOWN, RAMP_DIR_LEFT, RAMP_DIR_NONE, RAMP_DIR_RIGHT,
        RAMP_DIR_UP,
    };

    fn test_game() -> Game {
        Game::new(GameConfig {
            tick_rate: 30,
            map_w: 64,
            map_h: 64,
            tile_size: TILE_SIZE,
            min_population: 0,
            max_bots: 0,
            bot_target: 0,
            seed: 1234,
            profile: false,
            bot_skill: None,
        })
    }

    /// Отправляет ввод без наводки и способностей — для тестов движения.
    fn send_input(game: &mut Game, conn_id: u64, seq: u32, input: InputFlags) {
        game.apply_input(conn_id, seq, input, 4, protocol::ability::NONE, 0.0, 0.0);
    }

    #[test]
    fn input_axes_make_diagonals() {
        let up_right = InputFlags {
            up: true,
            right: true,
            ..Default::default()
        };
        assert_eq!(up_right.axes(), (-1, 1));
        assert_eq!(
            Direction::from_axes(up_right.axes().0, up_right.axes().1),
            Some(Direction::UpRight)
        );

        let up = InputFlags {
            up: true,
            ..Default::default()
        };
        assert_eq!(
            Direction::from_axes(up.axes().0, up.axes().1),
            Some(Direction::Up)
        );

        // Обе клавиши одной оси — ввода по этой оси нет.
        let both = InputFlags {
            up: true,
            down: true,
            ..Default::default()
        };
        assert_eq!(both.axes(), (0, 0));
        assert_eq!(Direction::from_axes(0, 0), None);
    }

    #[test]
    fn aim_towards_picks_sixty_four_directions() {
        // Угол 0 — вправо, дальше по часовой стрелке (ось Y вниз).
        assert_eq!(aim_towards(0.0, 0.0, 100.0, 0.0), 0);
        assert_eq!(aim_towards(0.0, 0.0, 0.0, 100.0), 16);
        assert_eq!(aim_towards(0.0, 0.0, -100.0, 0.0), 32);
        assert_eq!(aim_towards(0.0, 0.0, 0.0, -100.0), 48);
        // Диагональ 45° попадает между кардинальными: шаг башни 5.625°.
        assert_eq!(aim_towards(0.0, 0.0, 100.0, 100.0), 8);
        assert_eq!(aim_towards(0.0, 0.0, -100.0, 100.0), 24);
    }

    #[test]
    fn stars_add_second_gun_and_slow_reload_down() {
        assert_eq!(level_guns(1), 1);
        assert_eq!(level_guns(2), 2);
        assert_eq!(level_guns(3), 2);
        // Базовый танк стреляет втрое медленнее прежнего (0.55 с -> 1.65 с),
        // и с каждой звездой перезарядка сокращается.
        assert!((level_fire_cooldown(1) - 1.65).abs() < 0.001);
        assert!(level_fire_cooldown(2) < level_fire_cooldown(1));
        assert!(level_fire_cooldown(3) < level_fire_cooldown(2));
        // Предел на снаряды в полёте не должен быть жёстче перезарядки,
        // иначе темп стрельбы задаёт время жизни снаряда, а не перезарядка.
        assert_eq!(level_max_bullets(1), 3);
        assert_eq!(level_max_bullets(2), 6);
        assert_eq!(level_max_bullets(3), 6);
    }

    #[test]
    fn fire_cadence_follows_cooldown() {
        // Танк держит огонь: темп выстрелов должен совпадать с перезарядкой,
        // а на втором уровне — стать чаще и бить из двух стволов.
        fn measure(level: u8) -> (Vec<u64>, usize) {
            let mut game = test_game();
            let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
            game.join_player(pid, "A".into());
            {
                let tank = game.players.get_mut(&pid).and_then(|p| p.tank.as_mut()).expect("tank");
                tank.level = level;
                tank.max_hp = level_max_hp(level);
                tank.hp = tank.max_hp;
            }
            // Заглушки-бонусы далеко от танка: случайная звезда не изменит уровень.
            let (tx, ty) = game
                .players
                .get(&pid)
                .and_then(|p| p.tank.as_ref())
                .map(|t| (t.x, t.y))
                .expect("center");
            for i in 0..MAX_PICKUPS as u32 {
                game.pickups.insert(
                    i,
                    Pickup {
                        id: i,
                        x: tx + 4000.0 + i as f32,
                        y: ty,
                        kind: PickupKind::Star,
                        expires_at: u64::MAX,
                        deck: LAYER_GROUND,
                    },
                );
            }
            send_input(
                &mut game,
                1,
                1,
                InputFlags {
                    fire: true,
                    ..Default::default()
                },
            );

            let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
            let mut shots: Vec<u64> = Vec::new();
            let mut bullets_per_shot = 0usize;
            for _ in 0..300 {
                game.tick();
                let fresh: Vec<u32> = game
                    .bullets
                    .values()
                    .filter(|b| b.owner == pid)
                    .filter(|b| seen.insert(b.id))
                    .map(|b| b.id)
                    .collect();
                if !fresh.is_empty() {
                    shots.push(game.tick);
                    bullets_per_shot = fresh.len();
                }
            }
            (shots, bullets_per_shot)
        }

        // Первый уровень: выстрел раз в ~1.65 с = 50 тиков, один ствол.
        let (shots, per_shot) = measure(1);
        assert!(shots.len() >= 3, "выстрелов слишком мало: {:?}", shots);
        let gaps: Vec<u64> = shots.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            (49..=51).contains(&gaps[0]),
            "первый интервал должен быть ~50 тиков (1.65 с): {:?}",
            gaps
        );
        for g in &gaps {
            assert!(
                *g >= 49,
                "стрелять быстрее перезарядки нельзя (мин. 50 тиков): {:?}",
                gaps
            );
        }
        assert_eq!(per_shot, 1);

        // Второй уровень: перезарядка короче (~1.35 с = 40 тиков) и два ствола.
        let (shots, per_shot) = measure(2);
        let gaps: Vec<u64> = shots.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            (39..=42).contains(&gaps[0]),
            "на втором уровне первый интервал должен быть ~40 тиков (1.35 с): {:?}",
            gaps
        );
        for g in &gaps {
            assert!(
                *g >= 39,
                "стрелять быстрее перезарядки нельзя (мин. 40 тиков): {:?}",
                gaps
            );
        }
        assert_eq!(per_shot, 2);
    }

    #[test]
    fn diagonal_input_moves_tank_diagonally() {
        let mut game = test_game();
        let pid = game.add_player(1, "Тест".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "Тест".into());
        send_input(
            &mut game,
            1,
            1,
            InputFlags {
                right: true,
                down: true,
                ..Default::default()
            },
        );
        game.tick();
        let tank = game
            .players
            .get(&pid)
            .and_then(|p| p.tank.as_ref())
            .expect("tank");
        assert_eq!(tank.move_dir, Direction::DownRight);
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
        assert!(game.map.rect_is_free(start.0, start.1, TANK_SIZE, LAYER_GROUND));

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
        game.map.set_kind(10, 10, LAYER_GROUND, TileKind::Brick);
        let owner = 1u32;
        game.hit_tile(10, 10, 1, 1, owner, LAYER_GROUND);
        assert_eq!(game.map.damage(10, 10, LAYER_GROUND), 1);
        game.hit_tile(10, 10, 1, 1, owner, LAYER_GROUND);
        assert_eq!(game.map.damage(10, 10, LAYER_GROUND), 2);
        // Четыре попадания — кирпич разрушен.
        game.hit_tile(10, 10, 1, 1, owner, LAYER_GROUND);
        game.hit_tile(10, 10, 1, 1, owner, LAYER_GROUND);
        assert_eq!(game.map.kind(10, 10, LAYER_GROUND), TileKind::Empty);
    }

    #[test]
    fn steel_survives_normal_bullets_and_dies_from_level_three() {
        let mut game = test_game();
        game.map.set_kind(12, 12, LAYER_GROUND, TileKind::Steel);
        game.hit_tile(12, 12, 1, 1, 1, LAYER_GROUND);
        assert_eq!(game.map.kind(12, 12, LAYER_GROUND), TileKind::Steel);
        game.hit_tile(12, 12, 3, 3, 1, LAYER_GROUND);
        assert_eq!(game.map.kind(12, 12, LAYER_GROUND), TileKind::Empty);
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
        cfg.bot_target = 5;
        cfg.max_bots = 5;
        let mut game = Game::new(cfg);
        for _ in 0..10 {
            game.tick();
        }
        let bots = game.players.values().filter(|p| p.is_bot).count();
        assert_eq!(bots, 5);
    }

    #[test]
    fn twenty_bots_cover_the_map() {
        let mut cfg = test_game().cfg;
        cfg.bot_target = 20;
        cfg.max_bots = 24;
        let mut game = Game::new(cfg);
        for _ in 0..40 {
            game.tick();
        }
        let bots = game.players.values().filter(|p| p.is_bot).count();
        assert_eq!(bots, 20, "на карте должно быть 20 ботов");
    }

    #[test]
    fn bots_stay_when_humans_join() {
        let mut cfg = test_game().cfg;
        cfg.bot_target = 4;
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
        // Боты держат численность: карта остаётся живой, и человек — не единственный.
        assert_eq!(game.players.values().filter(|p| p.is_bot).count(), 4);
        assert!(game.players.values().any(|p| !p.is_bot));
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
        game.spawn_pickup_at(center.0, center.1, PickupKind::Repair, LAYER_GROUND);
        game.update_pickups();
        let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("tank");
        assert_eq!(tank.hp, tank.max_hp);

        game.spawn_pickup_at(center.0, center.1, PickupKind::Star, LAYER_GROUND);
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

    // -----------------------------------------------------------------------
    // Механики v3: башня, сложность, способности, туман войны
    // -----------------------------------------------------------------------

    /// Расчищает прямоугольник тайлов — чтобы тест не зависел от карты.
    ///
    /// На верхнем ярусе пустота — пропасть, поэтому «расчищенные» тайлы
    /// помечаются настилом: по ним можно ездить.
    fn clear_tiles(game: &mut Game, x0: u32, y0: u32, x1: u32, y1: u32, deck: u8) {
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                game.map.set_kind(tx, ty, deck, TileKind::Empty);
                if deck == LAYER_UPPER {
                    game.map.set_deck(tx, ty, false);
                }
            }
        }
    }

    /// Ставит танк игрока в заданную точку: без стен, без защиты при спавне.
    fn place_tank(game: &mut Game, pid: u32, x: f32, y: f32, aim: u8) {
        place_tank_on(game, pid, x, y, aim, LAYER_GROUND);
    }

    /// Ставит танк игрока на нужный ярус.
    fn place_tank_on(game: &mut Game, pid: u32, x: f32, y: f32, aim: u8, deck: u8) {
        if let Some(player) = game.players.get_mut(&pid) {
            player.aim = aim;
            if let Some(tank) = player.tank.as_mut() {
                tank.x = x;
                tank.y = y;
                tank.aim = aim;
                tank.dir = Direction::Down;
                tank.deck = deck;
                tank.ramp_dir = RAMP_DIR_NONE;
                tank.ramp_index = 0;
                tank.ramp_target = -1;
                tank.ramp_switched = false;
                tank.cooldown = 0.0;
                tank.spawn_protect_until = 0;
                tank.shield_until = 0;
            }
        }
    }

    /// Ставит полосу пандуса из трёх тайлов: низ, середина, верх.
    ///
    /// Полоса кладётся одинаково на обоих ярусах — так переход работает и
    /// вверх, и вниз. `dir` — направление подъёма: от нижнего края полосы
    /// к верхнему.
    fn place_ramp_strip(game: &mut Game, x0: u32, y0: u32, dir: u8) {
        let (dx, dy) = ramp_dir_delta(dir);
        for level in [LAYER_GROUND, LAYER_UPPER] {
            for k in 0..RAMP_LEN {
                let tx = (x0 as i64 + dx as i64 * k as i64) as u32;
                let ty = (y0 as i64 + dy as i64 * k as i64) as u32;
                game.map.set_value(tx, ty, level, ramp_value(dir, k, false));
            }
        }
    }

    /// Расчищает площадку под полосу пандуса и кладёт саму полосу.
    ///
    /// Нижний край полосы — тайл (10, 10), верхний — на два тайла дальше в
    /// направлении подъёма.
    fn prepare_ramp(game: &mut Game, dir: u8) {
        clear_tiles(game, 8, 8, 16, 12, LAYER_GROUND);
        clear_tiles(game, 8, 8, 16, 12, LAYER_UPPER);
        place_ramp_strip(game, 10, 10, dir);
    }

    /// Ввод для езды в заданную сторону.
    fn input_towards(dir: u8) -> InputFlags {
        let (dx, dy) = ramp_dir_delta(dir);
        InputFlags {
            right: dx > 0,
            down: dy > 0,
            left: dx < 0,
            up: dy < 0,
            ..Default::default()
        }
    }

    /// Противоположное направление полосы.
    fn opposite_dir(dir: u8) -> u8 {
        (dir + 2) & 0x03
    }

    /// Номер тайла полосы под центром танка, если он стоит на пандусе.
    fn ramp_index_under(game: &Game, tank: &Tank) -> Option<u8> {
        let (tx, ty) = game.map.tile_at(tank.center().0, tank.center().1);
        if game.map.kind(tx, ty, tank.deck) != TileKind::Ramp {
            return None;
        }
        Some(ramp_index(game.map.value(tx, ty, tank.deck)))
    }

    #[test]
    fn bots_are_always_slower_than_the_player() {
        // Ключевое требование: ни один бот не должен бежать быстрее человека.
        for skill in [BotSkill::Easy, BotSkill::Normal, BotSkill::Hard] {
            assert!(
                skill.speed_mult() < 1.0,
                "бот уровня «{}» быстрее игрока",
                skill.label()
            );
        }
        // И чем сложнее бот, тем он быстрее — но всё равно не быстрее игрока.
        assert!(BotSkill::Easy.speed_mult() < BotSkill::Normal.speed_mult());
        assert!(BotSkill::Normal.speed_mult() < BotSkill::Hard.speed_mult());
        // Сложный бот точнее и реактивнее лёгкого.
        assert!(BotSkill::Hard.aim_error() < BotSkill::Easy.aim_error());
        assert!(BotSkill::Hard.reaction_ticks() < BotSkill::Easy.reaction_ticks());
        assert!(BotSkill::Hard.sight_range() > BotSkill::Easy.sight_range());
    }

    #[test]
    fn tower_aims_by_mouse_while_body_follows_keys() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        clear_tiles(&mut game, 2, 2, 20, 20, LAYER_GROUND);
        // Корпус смотрит вниз, башня — вправо.
        place_tank(&mut game, pid, 10.0 * TILE_SIZE, 10.0 * TILE_SIZE, 0);

        game.apply_input(
            1,
            1,
            InputFlags {
                fire: true,
                ..Default::default()
            },
            0,
            protocol::ability::NONE,
            0.0,
            0.0,
        );
        game.tick();

        let bullet = game.bullets.values().next().expect("снаряд");
        assert_eq!(bullet.dir, 0, "снаряд должен лететь по башне, а не по корпусу");
        let (dx, dy) = protocol::aim_delta(bullet.dir);
        assert!(dx > 0.9, "снаряд должен лететь вправо");
        assert!(dy.abs() < 0.01, "снаряд не должен отклоняться вниз");
        let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("tank");
        assert_eq!(tank.dir, Direction::Down, "корпус остаётся направленным вниз");
    }

    #[test]
    fn enemies_are_hidden_outside_the_view_cone() {
        let mut game = test_game();
        let viewer = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        let behind = game.add_player(2, "B".into(), tokio::sync::mpsc::channel(4).0);
        let ahead = game.add_player(3, "C".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(viewer, "A".into());
        game.join_player(behind, "B".into());
        game.join_player(ahead, "C".into());
        clear_tiles(&mut game, 0, 6, 30, 14, LAYER_GROUND);

        let x = 10.0 * TILE_SIZE;
        let y = 10.0 * TILE_SIZE;
        // Башня смотрит вправо: цель спереди (C) и цель за спиной (B).
        place_tank(&mut game, viewer, x, y, 0);
        place_tank(&mut game, ahead, x + 200.0, y, 0);
        place_tank(&mut game, behind, x - 200.0, y, 0);

        game.tick();
        let snap = game.snapshot_for(viewer).expect("снимок");
        assert!(
            snap.players.iter().any(|p| p.id == ahead),
            "противник впереди должен быть виден"
        );
        assert!(
            !snap.players.iter().any(|p| p.id == behind),
            "противник за спиной не должен попадать в снимок"
        );
    }

    #[test]
    fn mortar_flies_over_walls_and_damages_the_area() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        let victim = game.add_player(2, "B".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        game.join_player(victim, "B".into());

        let x = 10.0 * TILE_SIZE;
        let y = 10.0 * TILE_SIZE;
        place_tank(&mut game, pid, x, y, 0);
        place_tank(&mut game, victim, x + 150.0, y, 0);
        // Между стрелком и целью — стена: миномёт её перелетает.
        clear_tiles(&mut game, 8, 9, 12, 11, LAYER_GROUND);
        game.map.set_kind(13, 10, LAYER_GROUND, TileKind::Steel);

        let hp_before = game
            .players
            .get(&victim)
            .and_then(|p| p.tank.as_ref())
            .map(|t| t.hp)
            .expect("танк");

        game.apply_input(
            1,
            1,
            InputFlags {
                use_ability: true,
                ..Default::default()
            },
            0,
            protocol::ability::MORTAR,
            x + 150.0,
            y,
        );
        assert_eq!(game.mortars.len(), 1, "миномёт должен вылететь");
        let flight_ticks = game.mortars.values().next().expect("миномёт").flight_ticks;

        for _ in 0..flight_ticks + 2 {
            game.tick();
        }
        assert!(game.mortars.is_empty(), "миномёт должен упасть");
        let hp_after = game
            .players
            .get(&victim)
            .and_then(|p| p.tank.as_ref())
            .map(|t| t.hp);
        assert!(
            hp_after.map(|hp| hp < hp_before).unwrap_or(true),
            "миномёт должен нанести урон"
        );
    }

    #[test]
    fn air_strike_hits_after_a_delay() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        let victim = game.add_player(2, "B".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        game.join_player(victim, "B".into());
        clear_tiles(&mut game, 8, 9, 12, 11, LAYER_GROUND);

        let x = 10.0 * TILE_SIZE;
        let y = 10.0 * TILE_SIZE;
        place_tank(&mut game, pid, x, y, 0);
        place_tank(&mut game, victim, x + 150.0, y, 0);
        let hp_before = game
            .players
            .get(&victim)
            .and_then(|p| p.tank.as_ref())
            .map(|t| t.hp)
            .expect("танк");

        game.apply_input(
            1,
            1,
            InputFlags {
                use_ability: true,
                ..Default::default()
            },
            0,
            protocol::ability::AIR_STRIKE,
            x + 150.0,
            y,
        );
        assert_eq!(game.airstrikes.len(), 1, "вызов авиаподдержки должен принять");

        // До задержки удара цела.
        for _ in 0..AIR_STRIKE_DELAY - 2 {
            game.tick();
        }
        let hp_mid = game
            .players
            .get(&victim)
            .and_then(|p| p.tank.as_ref())
            .map(|t| t.hp)
            .expect("танк");
        assert_eq!(hp_mid, hp_before, "удар не должен приходить мгновенно");

        for _ in 0..4 {
            game.tick();
        }
        // Удар может и убить — тогда танка просто не остаётся.
        let hp_after = game
            .players
            .get(&victim)
            .and_then(|p| p.tank.as_ref())
            .map(|t| t.hp);
        assert!(
            hp_after.map(|hp| hp < hp_before).unwrap_or(true),
            "авиаудар должен нанести урон"
        );
        assert!(game.airstrikes.is_empty(), "вызов должен завершиться");
    }

    #[test]
    fn abilities_respect_cooldown_and_range() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        clear_tiles(&mut game, 8, 9, 12, 11, LAYER_GROUND);
        let x = 10.0 * TILE_SIZE;
        let y = 10.0 * TILE_SIZE;
        place_tank(&mut game, pid, x, y, 0);

        // Миномёт.
        game.apply_input(
            1,
            1,
            InputFlags {
                use_ability: true,
                ..Default::default()
            },
            0,
            protocol::ability::MORTAR,
            x + 200.0,
            y,
        );
        assert_eq!(game.mortars.len(), 1);
        // Повторный вызов до кулдауна игнорируется.
        game.apply_input(
            1,
            2,
            InputFlags {
                use_ability: true,
                ..Default::default()
            },
            0,
            protocol::ability::MORTAR,
            x + 200.0,
            y,
        );
        assert_eq!(game.mortars.len(), 1, "кулдаун должен блокировать повтор");

        // Цель за пределами дальности — вызов не проходит.
        game.apply_input(
            1,
            3,
            InputFlags {
                use_ability: true,
                ..Default::default()
            },
            0,
            protocol::ability::AIR_STRIKE,
            x + ABILITY_RANGE + 500.0,
            y,
        );
        assert!(game.airstrikes.is_empty(), "дальность способности ограничена");
    }

    // -----------------------------------------------------------------------
    // Механики v4: ярусы, звук, кулдауны
    // -----------------------------------------------------------------------

    #[test]
    fn mortar_flies_twice_slower_than_a_bullet() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        clear_tiles(&mut game, 8, 9, 16, 11, LAYER_GROUND);
        let x = 10.0 * TILE_SIZE;
        let y = 10.0 * TILE_SIZE;
        place_tank(&mut game, pid, x, y, 0);

        game.apply_input(
            1,
            1,
            InputFlags {
                use_ability: true,
                ..Default::default()
            },
            0,
            protocol::ability::MORTAR,
            x + 300.0,
            y,
        );
        let mortar = game.mortars.values().next().expect("миномёт");
        let dist = (mortar.x1 - mortar.x0).abs();

        // Скорость миномёта — ровно половина скорости пули.
        let bullet_speed = level_bullet_speed(1);
        let expected =
            ((dist / (bullet_speed * MORTAR_SPEED_RATIO)) / TICK_DT).round() as u64;
        assert_eq!(mortar.flight_ticks, expected, "время полёта миномёта");

        // А значит — вдвое дольше, чем летел бы прямой снаряд.
        let bullet_time = ((dist / bullet_speed) / TICK_DT).round() as u64;
        assert!(
            (mortar.flight_ticks as f64 - bullet_time as f64 * 2.0).abs() < 2.0,
            "миномёт должен лететь вдвое медленнее пули: {} против {} тиков",
            mortar.flight_ticks,
            bullet_time
        );
    }

    #[test]
    fn shots_are_heard_outside_the_view_cone() {
        let mut game = test_game();
        let viewer = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        let shooter = game.add_player(2, "B".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(viewer, "A".into());
        game.join_player(shooter, "B".into());
        clear_tiles(&mut game, 0, 6, 30, 14, LAYER_GROUND);

        let x = 10.0 * TILE_SIZE;
        let y = 10.0 * TILE_SIZE;
        // Башня наблюдателя смотрит вправо, стрелок — за спиной.
        place_tank(&mut game, viewer, x, y, 0);
        place_tank(&mut game, shooter, x - 200.0, y, 0);

        game.apply_input(
            2,
            1,
            InputFlags {
                fire: true,
                ..Default::default()
            },
            0,
            protocol::ability::NONE,
            0.0,
            0.0,
        );
        game.tick();

        let snap = game.snapshot_for(viewer).expect("снимок");
        assert!(
            !snap.players.iter().any(|p| p.id == shooter),
            "стрелок за спиной не должен быть виден"
        );
        assert!(
            snap.sounds
                .iter()
                .any(|s| s.kind == SOUND_SHOT && s.owner == shooter),
            "выстрел за спиной должен быть слышен"
        );
    }

    #[test]
    fn ramp_moves_the_tank_to_another_deck() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        // Пандус из трёх тайлов: подъём вправо, от нижнего края к верхнему.
        prepare_ramp(&mut game, RAMP_DIR_RIGHT);

        place_tank(&mut game, pid, 10.0 * TILE_SIZE, 10.0 * TILE_SIZE, 0);
        send_input(&mut game, 1, 1, input_towards(RAMP_DIR_RIGHT));

        for _ in 0..300 {
            game.tick();
            let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
            if tank.center().0 >= 12.0 * TILE_SIZE {
                assert_eq!(tank.deck, LAYER_UPPER, "проезд по пандусу меняет ярус");
                return;
            }
        }
        panic!("танк должен доехать до конца пандуса");
    }

    #[test]
    fn ramp_switches_deck_only_at_the_top_of_the_strip() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        prepare_ramp(&mut game, RAMP_DIR_RIGHT);

        // Танк въезжает на нижний тайл полосы и едет вверх по пандусу.
        place_tank(&mut game, pid, 10.0 * TILE_SIZE, 10.0 * TILE_SIZE, 0);
        send_input(&mut game, 1, 1, input_towards(RAMP_DIR_RIGHT));

        let mut seen = [false; RAMP_LEN as usize];
        for _ in 0..300 {
            game.tick();
            let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
            let Some(idx) = ramp_index_under(&game, tank) else {
                continue;
            };
            seen[idx as usize] = true;
            match idx {
                // На нижнем и среднем тайле ярус ещё не меняется.
                0 | 1 => assert_eq!(
                    tank.deck, LAYER_GROUND,
                    "внутри пандуса ярус меняться не должен"
                ),
                // На верхнем тайле — переход на верхний этаж.
                _ => assert_eq!(
                    tank.deck, LAYER_UPPER,
                    "на верхнем тайле пандуса ярус меняется"
                ),
            }
            if tank.deck == LAYER_UPPER {
                break;
            }
        }
        assert!(
            seen[0] && seen[1] && seen[2],
            "танк должен проехать всю полосу: {:?}",
            seen
        );
    }

    #[test]
    fn ramp_switches_deck_down_at_the_bottom_of_the_strip() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        prepare_ramp(&mut game, RAMP_DIR_RIGHT);

        // Танк стоит на верхнем тайле полосы и спускается по пандусу вниз.
        place_tank_on(
            &mut game,
            pid,
            12.0 * TILE_SIZE,
            10.0 * TILE_SIZE,
            0,
            LAYER_UPPER,
        );
        send_input(&mut game, 1, 1, input_towards(RAMP_DIR_LEFT));

        let mut seen = [false; RAMP_LEN as usize];
        for _ in 0..300 {
            game.tick();
            let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
            let Some(idx) = ramp_index_under(&game, tank) else {
                continue;
            };
            seen[idx as usize] = true;
            match idx {
                // На верхнем и среднем тайле ярус ещё не меняется.
                1 | 2 => assert_eq!(
                    tank.deck, LAYER_UPPER,
                    "внутри пандуса ярус меняться не должен"
                ),
                // На нижнем тайле — переход на землю.
                _ => assert_eq!(
                    tank.deck, LAYER_GROUND,
                    "на нижнем тайле пандуса ярус меняется вниз"
                ),
            }
            if tank.deck == LAYER_GROUND {
                break;
            }
        }
        assert!(
            seen[0] && seen[1] && seen[2],
            "танк должен проехать всю полосу: {:?}",
            seen
        );
    }

    #[test]
    fn ramp_on_the_right_strip_keeps_the_ride_state() {
        // Направление полосы «вправо» — ноль, поэтому состояние заезда
        // нельзя держать в нуле: иначе оно сбрасывалось бы каждый тик и
        // переход не работал бы вовсе.
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        prepare_ramp(&mut game, RAMP_DIR_RIGHT);

        place_tank(&mut game, pid, 10.0 * TILE_SIZE, 10.0 * TILE_SIZE, 0);
        send_input(&mut game, 1, 1, input_towards(RAMP_DIR_RIGHT));

        let mut switched = false;
        for _ in 0..300 {
            game.tick();
            let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
            if tank.deck == LAYER_UPPER {
                switched = true;
                break;
            }
        }
        assert!(switched, "подъём по полосе вправо должен сменить ярус");

        // Танк остановился на верхнем тайле полосы: состояние заезда должно
        // сохраниться, иначе ярус может щёлкнуть обратно.
        {
            let tank = game
                .players
                .get_mut(&pid)
                .and_then(|p| p.tank.as_mut())
                .expect("танк");
            tank.x = 12.0 * TILE_SIZE;
            tank.y = 10.0 * TILE_SIZE;
            tank.vel = 0.0;
        }
        send_input(&mut game, 1, 2, InputFlags::default());
        for _ in 0..5 {
            game.tick();
        }
        let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
        assert_eq!(tank.deck, LAYER_UPPER, "ярус не должен вернуться назад");
        assert_eq!(tank.ramp_dir, RAMP_DIR_RIGHT, "направление полосы сохранено");
        assert!(tank.ramp_switched, "состояние заезда не должно сбрасываться");
    }

    #[test]
    fn ramp_strips_work_in_every_direction() {
        // Полоса «вправо» (RAMP_DIR_RIGHT == 0) — самая частая, и раньше
        // именно на ней ломался переход: её направление совпадало с
        // признаком «танк не на пандусе».
        for dir in [RAMP_DIR_RIGHT, RAMP_DIR_DOWN, RAMP_DIR_LEFT, RAMP_DIR_UP] {
            let mut game = test_game();
            let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
            game.join_player(pid, "A".into());
            prepare_ramp(&mut game, dir);

            // Подъём: с нижнего яруса до верхнего тайла полосы.
            place_tank(&mut game, pid, 10.0 * TILE_SIZE, 10.0 * TILE_SIZE, 0);
            send_input(&mut game, 1, 1, input_towards(dir));
            let mut up = false;
            for _ in 0..300 {
                game.tick();
                let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
                if ramp_index_under(&game, tank) == Some(RAMP_LEN - 1) {
                    assert_eq!(
                        tank.deck, LAYER_UPPER,
                        "подъём по пандусу направления {dir} должен сменить ярус"
                    );
                    up = true;
                    break;
                }
            }
            assert!(up, "танк должен доехать до верха полосы направления {dir}");

            // Спуск: с верхнего яруса до нижнего тайла полосы.
            let (dx, dy) = ramp_dir_delta(dir);
            let top_x = 10.0 + 2.0 * dx as f32;
            let top_y = 10.0 + 2.0 * dy as f32;
            let mut game = test_game();
            let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
            game.join_player(pid, "A".into());
            prepare_ramp(&mut game, dir);
            place_tank_on(
                &mut game,
                pid,
                top_x * TILE_SIZE,
                top_y * TILE_SIZE,
                0,
                LAYER_UPPER,
            );
            send_input(&mut game, 1, 1, input_towards(opposite_dir(dir)));
            let mut down = false;
            for _ in 0..300 {
                game.tick();
                let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
                if ramp_index_under(&game, tank) == Some(0) {
                    assert_eq!(
                        tank.deck, LAYER_GROUND,
                        "спуск по пандусу направления {dir} должен сменить ярус"
                    );
                    down = true;
                    break;
                }
            }
            assert!(down, "танк должен доехать до низа полосы направления {dir}");
        }
    }

    #[test]
    fn ramp_switches_the_deck_only_after_the_strip_was_driven_through() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        prepare_ramp(&mut game, RAMP_DIR_RIGHT);

        // Танк на верхнем ярусе стоит на нижнем краю полосы — но он не проехал
        // полосу, поэтому ярус не меняется: наезд сбоку переходом не считается.
        place_tank_on(
            &mut game,
            pid,
            10.0 * TILE_SIZE,
            10.0 * TILE_SIZE,
            0,
            LAYER_UPPER,
        );
        game.tick();
        {
            let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
            assert_eq!(
                tank.deck, LAYER_UPPER,
                "наезд сбоку на край полосы ярус не меняет"
            );
        }

        // А вот проезд по полосе — переход: на нижнем краю ярус меняется.
        place_tank_on(
            &mut game,
            pid,
            12.0 * TILE_SIZE,
            10.0 * TILE_SIZE,
            0,
            LAYER_UPPER,
        );
        send_input(&mut game, 1, 1, input_towards(RAMP_DIR_LEFT));
        let mut down = false;
        for _ in 0..300 {
            game.tick();
            let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
            if ramp_index_under(&game, tank) == Some(0) {
                assert_eq!(
                    tank.deck, LAYER_GROUND,
                    "спуск по пандусу должен сменить ярус"
                );
                down = true;
                break;
            }
        }
        assert!(down, "танк должен доехать до низа полосы");
    }

    #[test]
    fn ramp_in_the_middle_switches_only_after_the_top_edge() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        prepare_ramp(&mut game, RAMP_DIR_RIGHT);

        // Танк въехал сбоку на средний тайл полосы.
        place_tank(&mut game, pid, 11.0 * TILE_SIZE, 10.0 * TILE_SIZE, 0);
        game.tick();
        {
            let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
            assert_eq!(
                tank.deck, LAYER_GROUND,
                "на среднем тайле ярус ещё не меняется"
            );
        }

        // Теперь едет вверх по пандусу — переход на верхнем тайле.
        send_input(&mut game, 1, 1, input_towards(RAMP_DIR_RIGHT));
        let mut switched = false;
        for _ in 0..300 {
            game.tick();
            let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
            match ramp_index_under(&game, tank) {
                Some(2) => {
                    assert_eq!(
                        tank.deck, LAYER_UPPER,
                        "на верхнем тайле пандуса ярус меняется"
                    );
                    switched = true;
                    break;
                }
                _ => assert_eq!(
                    tank.deck, LAYER_GROUND,
                    "внутри пандуса ярус меняться не должен"
                ),
            }
        }
        assert!(switched, "танк должен доехать до верхнего тайла пандуса");
    }

    #[test]
    fn leaving_the_strip_resets_the_ramp_state() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        prepare_ramp(&mut game, RAMP_DIR_RIGHT);

        // Подъём по пандусу и выезд с него на площадку.
        place_tank(&mut game, pid, 10.0 * TILE_SIZE, 10.0 * TILE_SIZE, 0);
        send_input(&mut game, 1, 1, input_towards(RAMP_DIR_RIGHT));
        let mut left = false;
        for _ in 0..300 {
            game.tick();
            let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
            if tank.center().0 >= 13.0 * TILE_SIZE {
                left = true;
                break;
            }
        }
        assert!(left, "танк должен выехать с пандуса");
        {
            let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
            assert_eq!(tank.deck, LAYER_UPPER, "подъём по пандусу сменил ярус");
            assert_eq!(
                tank.ramp_dir, RAMP_DIR_NONE,
                "после схода с пандуса заезд сброшен"
            );
            assert_eq!(tank.ramp_target, -1, "цель перехода сброшена");
            assert!(!tank.ramp_switched, "счётчик переключений сброшен");
        }

        // Снова заезжаем на полосу — заезд начинается заново и вниз.
        send_input(&mut game, 1, 2, input_towards(RAMP_DIR_LEFT));
        let mut down = false;
        for _ in 0..300 {
            game.tick();
            let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
            if ramp_index_under(&game, tank) == Some(0) {
                assert_eq!(
                    tank.deck, LAYER_GROUND,
                    "спуск по пандусу снова меняет ярус"
                );
                down = true;
                break;
            }
        }
        assert!(down, "танк должен доехать до нижнего тайла пандуса");
    }

    #[test]
    fn enemies_on_another_deck_are_hidden() {
        let mut game = test_game();
        let viewer = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        let foe = game.add_player(2, "B".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(viewer, "A".into());
        game.join_player(foe, "B".into());
        clear_tiles(&mut game, 0, 6, 30, 14, LAYER_GROUND);
        clear_tiles(&mut game, 0, 6, 30, 14, LAYER_UPPER);

        let x = 10.0 * TILE_SIZE;
        let y = 10.0 * TILE_SIZE;
        place_tank_on(&mut game, viewer, x, y, 0, LAYER_GROUND);
        // Противник прямо перед наблюдателем, но этажом выше.
        place_tank_on(&mut game, foe, x + 200.0, y, 0, LAYER_UPPER);

        game.tick();
        let snap = game.snapshot_for(viewer).expect("снимок");
        assert!(
            !snap.players.iter().any(|p| p.id == foe),
            "противник на другом ярусе не виден"
        );
        assert!(
            snap.players.iter().any(|p| p.id == viewer),
            "свой танк виден всегда"
        );
    }

    #[test]
    fn snapshot_always_reports_both_cooldowns() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        clear_tiles(&mut game, 8, 9, 16, 11, LAYER_GROUND);
        let x = 10.0 * TILE_SIZE;
        let y = 10.0 * TILE_SIZE;
        place_tank(&mut game, pid, x, y, 0);

        game.apply_input(
            1,
            1,
            InputFlags {
                use_ability: true,
                ..Default::default()
            },
            0,
            protocol::ability::MORTAR,
            x + 200.0,
            y,
        );
        game.tick();

        let snap = game.snapshot_for(pid).expect("снимок");
        let me = snap.players.iter().find(|p| p.id == pid).expect("игрок");
        assert!(me.mortar_cd > 0, "кулдаун миномёта виден всегда");
        assert_eq!(
            me.airstrike_cd, 0,
            "кулдаун авиаподдержки тоже присылается всегда"
        );
    }

    // -----------------------------------------------------------------------
    // Три уровня высот: ямки, земля, возвышенности
    // -----------------------------------------------------------------------

    /// Ставит спускной пандус в ямку: край земли, откос, дно.
    ///
    /// Полоса идёт сверху вниз: верхний тайл — ещё на уровне земли, нижний —
    /// уже на дне ямки.
    fn place_pit_ramp(game: &mut Game, x0: u32, y0: u32) {
        for k in 0..RAMP_LEN {
            let value = ramp_value(RAMP_DIR_UP, RAMP_LEN - 1 - k, true);
            game.map.set_value(x0, y0 + k as u32, LAYER_GROUND, value);
            game.map.set_value(x0, y0 + k as u32, LAYER_PIT, value);
        }
    }

    #[test]
    fn pit_ramp_takes_the_tank_down_and_back() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        clear_tiles(&mut game, 8, 8, 16, 18, LAYER_GROUND);
        clear_tiles(&mut game, 8, 8, 16, 18, LAYER_PIT);
        clear_tiles(&mut game, 8, 8, 16, 18, LAYER_UPPER);
        // Дно ямки под спуском.
        for ty in 12..=15 {
            game.map.set_kind(10, ty, LAYER_PIT, TileKind::Pit);
        }
        place_pit_ramp(&mut game, 10, 10);

        // Танк на земле, на верхнем тайле полосы, и едет вниз.
        place_tank_on(&mut game, pid, 10.0 * TILE_SIZE, 10.0 * TILE_SIZE, 0, LAYER_GROUND);
        send_input(&mut game, 1, 1, input_towards(RAMP_DIR_DOWN));

        let mut reached_pit = false;
        for _ in 0..300 {
            game.tick();
            let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
            if tank.deck == LAYER_PIT {
                reached_pit = true;
                break;
            }
        }
        assert!(reached_pit, "спускной пандус должен увести танка в ямку");

        // И обратно — на землю.
        send_input(&mut game, 1, 2, input_towards(RAMP_DIR_UP));
        let mut reached_ground = false;
        for _ in 0..300 {
            game.tick();
            let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
            if tank.deck == LAYER_GROUND {
                reached_ground = true;
                break;
            }
        }
        assert!(reached_ground, "по спускному пандусу можно вернуться на землю");
    }

    #[test]
    fn tank_cannot_drive_into_a_pit() {
        let mut game = test_game();
        let pid = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(pid, "A".into());
        clear_tiles(&mut game, 8, 8, 18, 14, LAYER_GROUND);
        clear_tiles(&mut game, 8, 8, 18, 14, LAYER_PIT);
        clear_tiles(&mut game, 8, 8, 18, 14, LAYER_UPPER);
        // Ямка: дыра в земле на тайлах (12, 10)..(13, 11).
        for ty in 10..=11 {
            for tx in 12..=13 {
                game.map.set_kind(tx, ty, LAYER_GROUND, TileKind::Empty);
                game.map.set_kind(tx, ty, LAYER_PIT, TileKind::Pit);
            }
        }

        place_tank(&mut game, pid, 10.0 * TILE_SIZE, 10.0 * TILE_SIZE, 0);
        send_input(&mut game, 1, 1, input_towards(RAMP_DIR_RIGHT));
        for _ in 0..120 {
            game.tick();
        }
        let tank = game.players.get(&pid).and_then(|p| p.tank.as_ref()).expect("танк");
        assert_eq!(tank.deck, LAYER_GROUND, "танк остаётся на земле");
        assert!(
            tank.x + TANK_SIZE <= 12.0 * TILE_SIZE + 0.5,
            "в ямку с земли уехать нельзя, x = {}",
            tank.x
        );
    }

    #[test]
    fn flat_fire_rules_follow_the_height() {
        // Свой слой — всегда.
        assert!(flat_shot_hits(LAYER_GROUND, LAYER_GROUND));
        assert!(flat_shot_hits(LAYER_UPPER, LAYER_UPPER));
        assert!(flat_shot_hits(LAYER_PIT, LAYER_PIT));
        // С возвышенности достаёт землю — преимущество высоты.
        assert!(flat_shot_hits(LAYER_UPPER, LAYER_GROUND));
        // Снизу вверх — нет: края платформ и бортики мостов блокируют.
        assert!(!flat_shot_hits(LAYER_GROUND, LAYER_UPPER));
        // В ямку настильным огнём не достать.
        assert!(!flat_shot_hits(LAYER_GROUND, LAYER_PIT));
        assert!(!flat_shot_hits(LAYER_UPPER, LAYER_PIT));
        // Из ямки — только внутри своей низины.
        assert!(!flat_shot_hits(LAYER_PIT, LAYER_GROUND));
        assert!(!flat_shot_hits(LAYER_PIT, LAYER_UPPER));
    }

    #[test]
    fn bullets_follow_the_height_rules() {
        let mut game = test_game();
        let shooter = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(shooter, "A".into());
        let foe = game.add_player(2, "B".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(foe, "B".into());
        clear_tiles(&mut game, 4, 4, 30, 20, LAYER_GROUND);
        clear_tiles(&mut game, 4, 4, 30, 20, LAYER_PIT);
        clear_tiles(&mut game, 4, 4, 30, 20, LAYER_UPPER);

        let bx = 20.0 * TILE_SIZE + 8.0;
        let by = 10.0 * TILE_SIZE + 8.0;
        let shot = |deck: u8| Bullet {
            id: 1,
            owner: shooter,
            x: bx,
            y: by,
            dir: 0,
            deck,
            level: 1,
            damage: 1,
            speed: 100.0,
        };

        // Противник в ямке: с земли и с возвышенности настильным огнём
        // не достать.
        place_tank_on(&mut game, foe, 20.0 * TILE_SIZE, 10.0 * TILE_SIZE, 0, LAYER_PIT);
        assert_eq!(game.find_tank_hit(&shot(LAYER_GROUND)), None);
        assert_eq!(game.find_tank_hit(&shot(LAYER_UPPER)), None);
        // В ямку можно попасть только из своей же низины.
        assert_eq!(game.find_tank_hit(&shot(LAYER_PIT)), Some(foe));

        // Противник на земле: с возвышенности — попадание.
        place_tank_on(&mut game, foe, 20.0 * TILE_SIZE, 10.0 * TILE_SIZE, 0, LAYER_GROUND);
        assert_eq!(game.find_tank_hit(&shot(LAYER_UPPER)), Some(foe));
        // Из ямки по земле — мимо.
        assert_eq!(game.find_tank_hit(&shot(LAYER_PIT)), None);

        // Противник на возвышенности: с земли вверх — нет.
        place_tank_on(&mut game, foe, 20.0 * TILE_SIZE, 10.0 * TILE_SIZE, 0, LAYER_UPPER);
        assert_eq!(game.find_tank_hit(&shot(LAYER_GROUND)), None);
        assert_eq!(game.find_tank_hit(&shot(LAYER_UPPER)), Some(foe));
    }

    #[test]
    fn mortar_reaches_a_tank_in_a_pit() {
        let mut game = test_game();
        let shooter = game.add_player(1, "A".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(shooter, "A".into());
        let foe = game.add_player(2, "B".into(), tokio::sync::mpsc::channel(4).0);
        game.join_player(foe, "B".into());
        clear_tiles(&mut game, 4, 4, 30, 20, LAYER_GROUND);
        clear_tiles(&mut game, 4, 4, 30, 20, LAYER_PIT);
        clear_tiles(&mut game, 4, 4, 30, 20, LAYER_UPPER);
        place_tank_on(&mut game, foe, 20.0 * TILE_SIZE, 10.0 * TILE_SIZE, 0, LAYER_PIT);

        // Навесной удар в точку рядом с танком в ямке.
        game.explode(
            20.0 * TILE_SIZE + 10.0,
            10.0 * TILE_SIZE + 10.0,
            48.0,
            3,
            shooter,
            LAYER_GROUND,
        );
        let hp = game
            .players
            .get(&foe)
            .and_then(|p| p.tank.as_ref())
            .map(|t| t.hp)
            .unwrap_or(0);
        assert!(hp < 3, "миномёт достаёт танк в ямке, hp = {hp}");
    }
}
