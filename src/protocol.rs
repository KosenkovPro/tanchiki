//! Компактный бинарный протокол клиент <-> сервер.
//!
//! Формат полностью описан в `README.md`. Ключевые свойства:
//!
//! * все целые числа — little-endian;
//! * каждое сообщение начинается с одного байта-типа (`MSG_*`);
//! * строки кодируются как `u16 len + utf8 bytes`;
//! * клиент шлёт только ввод и пинги, сервер — только снапшоты и служебные сообщения;
//! * декодирование никогда не паникует: любая ошибка возвращается как [`ProtocolError`].

use std::fmt;

/// Версия протокола. Клиент и сервер обязаны иметь одинаковую.
///
/// * `1` — четыре направления, лидерборд без убийств;
/// * `2` — восемь направлений (включая диагонали), в лидерборде есть убийства;
/// * `3` — башня смотрит в 16 направлений (наведение мышью), добавлены
///   способности (миномёт, авиаподдержка) и туман войны по конусу взгляда;
/// * `4` — башня смотрит в 64 направления (шаг 5.625°), карта разделена на
///   ярусы (этажи), у сущностей есть `deck`, звуки приходят отдельным
///   каналом и слышны вне конуса обзора.
pub const PROTOCOL_VERSION: u16 = 4;

/// Максимальная длина никнейма в байтах UTF-8.
pub const MAX_NICK_LEN: usize = 16;

// ---------------------------------------------------------------------------
// Типы сообщений
// ---------------------------------------------------------------------------

/// Клиент -> сервер.
pub mod client_msg {
    /// Запрос входа в бой: никнейм.
    pub const JOIN: u8 = 0x01;
    /// Ввод игрока.
    pub const INPUT: u8 = 0x02;
    /// Запрос респауна после смерти.
    pub const RESPAWN: u8 = 0x03;
    /// Пинг для измерения RTT.
    pub const PING: u8 = 0x04;
}

/// Сервер -> клиент.
pub mod server_msg {
    /// Приветствие: идентификатор игрока + полная карта.
    pub const WELCOME: u8 = 0x11;
    /// Снапшот мира для конкретного игрока.
    pub const SNAPSHOT: u8 = 0x12;
    /// Таблица лидеров.
    pub const LEADERBOARD: u8 = 0x13;
    /// Игрок погиб.
    pub const DEATH: u8 = 0x14;
    /// Игрок возродился.
    pub const RESPAWNED: u8 = 0x15;
    /// Ответ на пинг.
    pub const PONG: u8 = 0x16;
    /// Ошибка протокола.
    pub const ERROR: u8 = 0x17;
}

/// Флаги ввода, передаются одним байтом.
pub mod input_bit {
    pub const UP: u8 = 1 << 0;
    pub const DOWN: u8 = 1 << 1;
    pub const LEFT: u8 = 1 << 2;
    pub const RIGHT: u8 = 1 << 3;
    pub const FIRE: u8 = 1 << 4;
    /// Применить выбранную способность (миномёт / авиаподдержка).
    pub const USE: u8 = 1 << 5;
}

/// Специальные способности, доступные игроку.
pub mod ability {
    /// Обычная пушка.
    pub const NONE: u8 = 0;
    /// Миномёт: снаряд летит по дуге и перелетает стены.
    pub const MORTAR: u8 = 1;
    /// Авиаподдержка: удар с воздуха по точке.
    pub const AIR_STRIKE: u8 = 2;
}

// ---------------------------------------------------------------------------
// Ошибки
// ---------------------------------------------------------------------------

/// Ошибка декодирования сообщения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// Сообщение пустое.
    Empty,
    /// Данных не хватило.
    Truncated,
    /// Неизвестный тип сообщения.
    UnknownMessage(u8),
    /// Строка не является валидным UTF-8.
    InvalidUtf8,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::Empty => write!(f, "пустое сообщение"),
            ProtocolError::Truncated => write!(f, "сообщение обрезано"),
            ProtocolError::UnknownMessage(t) => write!(f, "неизвестный тип сообщения: {t:#04x}"),
            ProtocolError::InvalidUtf8 => write!(f, "некорректный UTF-8"),
        }
    }
}

impl std::error::Error for ProtocolError {}

pub type ProtocolResult<T> = Result<T, ProtocolError>;

// ---------------------------------------------------------------------------
// Направления
// ---------------------------------------------------------------------------

/// Направление движения и стрельбы. Значения совпадают с байтом в протоколе.
///
/// Помимо четырёх кардинальных направлений есть четыре диагональных:
/// движение по диагонали разрешено и выглядит естественно, потому что
/// спрайт танка симметричен относительно продольной оси.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Direction {
    Up = 0,
    Down = 1,
    Left = 2,
    Right = 3,
    UpLeft = 4,
    UpRight = 5,
    DownLeft = 6,
    DownRight = 7,
}

impl Direction {
    /// Единичный вектор движения.
    ///
    /// У диагоналей длина нормализована (`√2/2` по каждой оси), поэтому
    /// диагональный ход не быстрее кардинального по общей скорости.
    pub fn delta(self) -> (f32, f32) {
        const S: f32 = 0.707_106_78;
        match self {
            Direction::Up => (0.0, -1.0),
            Direction::Down => (0.0, 1.0),
            Direction::Left => (-1.0, 0.0),
            Direction::Right => (1.0, 0.0),
            Direction::UpLeft => (-S, -S),
            Direction::UpRight => (S, -S),
            Direction::DownLeft => (-S, S),
            Direction::DownRight => (S, S),
        }
    }

    /// `true` для строго вертикального направления (вверх/вниз).
    pub fn is_vertical(self) -> bool {
        matches!(self, Direction::Up | Direction::Down)
    }

    /// `true` для диагональных направлений.
    pub fn is_diagonal(self) -> bool {
        matches!(
            self,
            Direction::UpLeft | Direction::UpRight | Direction::DownLeft | Direction::DownRight
        )
    }

    /// Направление по знакам осей: вертикаль (`-1` вверх, `1` вниз) и
    /// горизонталь (`-1` влево, `1` вправо).
    ///
    /// Обе оси нулевые — направления нет (ввод не задан).
    pub fn from_axes(v: i8, h: i8) -> Option<Direction> {
        match (v, h) {
            (-1, 0) => Some(Direction::Up),
            (1, 0) => Some(Direction::Down),
            (0, -1) => Some(Direction::Left),
            (0, 1) => Some(Direction::Right),
            (-1, -1) => Some(Direction::UpLeft),
            (-1, 1) => Some(Direction::UpRight),
            (1, -1) => Some(Direction::DownLeft),
            (1, 1) => Some(Direction::DownRight),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Наведение (башня)
// ---------------------------------------------------------------------------

/// Количество направлений наведения башни (шаг 5.625°).
///
/// Много направлений нужно для плавной границы конуса обзора: при 16
/// направлениях край конуса прыгал на 22.5° и тайлы «моргали».
pub const AIM_COUNT: u8 = 64;

/// Единичный вектор направления наведения.
///
/// Угол `0` — вправо, дальше по часовой стрелке: в экранных координатах
/// ось Y направлена вниз.
pub fn aim_delta(aim: u8) -> (f32, f32) {
    let a = aim as f32 * (std::f32::consts::PI * 2.0 / AIM_COUNT as f32);
    (a.cos(), a.sin())
}

/// Направление наведения по вектору: округление до ближайшего из 64.
pub fn aim_from_vector(dx: f32, dy: f32) -> u8 {
    if dx.abs() < f32::EPSILON && dy.abs() < f32::EPSILON {
        return 0;
    }
    let step = std::f32::consts::PI * 2.0 / AIM_COUNT as f32;
    let idx = (dy.atan2(dx) / step).round() as i32;
    idx.rem_euclid(AIM_COUNT as i32) as u8
}

/// Направление наведения от одной точки к другой.
pub fn aim_towards(x0: f32, y0: f32, x1: f32, y1: f32) -> u8 {
    aim_from_vector(x1 - x0, y1 - y0)
}

// ---------------------------------------------------------------------------
// Ввод
// ---------------------------------------------------------------------------

/// Состояние управления игрока.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputFlags {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub fire: bool,
    /// Применить выбранную способность (миномёт / авиаподдержка).
    ///
    /// Флаг одноразовый: клиент выставляет его ровно в одном пакете,
    /// иначе способность срабатывала бы каждый кадр.
    pub use_ability: bool,
}

impl InputFlags {
    /// Распаковывает байт ввода.
    pub fn from_byte(b: u8) -> InputFlags {
        InputFlags {
            up: b & input_bit::UP != 0,
            down: b & input_bit::DOWN != 0,
            left: b & input_bit::LEFT != 0,
            right: b & input_bit::RIGHT != 0,
            fire: b & input_bit::FIRE != 0,
            use_ability: b & input_bit::USE != 0,
        }
    }

    /// Знаки осей ввода: вертикаль (`-1` вверх, `1` вниз) и горизонталь
    /// (`-1` влево, `1` вправо).
    ///
    /// Обе клавиши одной оси одновременно считаются отсутствующим вводом.
    pub fn axes(self) -> (i8, i8) {
        let v = match (self.up, self.down) {
            (true, false) => -1,
            (false, true) => 1,
            _ => 0,
        };
        let h = match (self.left, self.right) {
            (true, false) => -1,
            (false, true) => 1,
            _ => 0,
        };
        (v, h)
    }
}

// ---------------------------------------------------------------------------
// Структуры данных протокола
// ---------------------------------------------------------------------------

/// Состояние игрока в снимке мира.
#[derive(Debug, Clone)]
pub struct PlayerView {
    pub id: u32,
    pub name: String,
    pub is_bot: bool,
    pub x: f32,
    pub y: f32,
    /// Направление корпуса (восемь направлений движения).
    pub dir: Direction,
    /// Направление башни (64 направления, шаг 5.625°) — по нему летят снаряды.
    pub aim: u8,
    /// Ярус (этаж по высоте), на котором находится танк.
    pub deck: u8,
    pub level: u8,
    pub hp: u8,
    pub max_hp: u8,
    pub alive: bool,
    pub shielded: bool,
    pub score: u32,
    pub kills: u32,
    /// Выбранная способность игрока.
    pub ability: u8,
    /// Через сколько тиков миномёт снова будет доступен.
    pub mortar_cd: u16,
    /// Через сколько тиков авиаподдержка снова будет доступна.
    pub airstrike_cd: u16,
}

/// Состояние снаряда.
#[derive(Debug, Clone, Copy)]
pub struct BulletView {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    /// Направление полёта (64 направления).
    pub dir: u8,
    /// Ярус, на котором летит снаряд.
    pub deck: u8,
    pub level: u8,
}

/// Звуковое событие: источник звука в мировых координатах.
///
/// Отправляется всем, кто рядом, даже если источника не видно: по нему
/// клиент играет объёмный звук и игрок слышит, с какой стороны атакуют.
#[derive(Debug, Clone, Copy)]
pub struct SoundView {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    /// `0` — выстрел, `1` — взрыв, `2` — попадание.
    pub kind: u8,
    /// Чей это звук (`0` — бота, иначе id игрока).
    pub owner: u32,
    /// Ярус, на котором произошёл звук.
    pub deck: u8,
}

/// Визуальный эффект: снаряд миномёта, вызов авиаподдержки и т. п.
#[derive(Debug, Clone, Copy)]
pub struct EffectView {
    pub id: u32,
    /// `1` — миномёт в полёте, `2` — авиаподдержка.
    pub kind: u8,
    pub x: f32,
    pub y: f32,
    /// Точка падения / удара.
    pub x2: f32,
    pub y2: f32,
    /// Ход процесса: `0..=100`.
    pub phase: u8,
    /// Ярус эффекта.
    pub deck: u8,
}

/// Состояние бонуса.
#[derive(Debug, Clone, Copy)]
pub struct PickupView {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub kind: u8,
    /// Остаток времени жизни в тиках.
    pub ttl: u32,
    /// Ярус, на котором лежит бонус.
    pub deck: u8,
}

/// Изменение одного тайла карты.
#[derive(Debug, Clone, Copy)]
pub struct TileUpdate {
    pub x: u16,
    pub y: u16,
    pub value: u8,
    /// Ярус, на котором изменился тайл.
    pub deck: u8,
}

/// Событие мира для клиента.
#[derive(Debug, Clone, Copy)]
pub enum GameEvent {
    /// Взрыв.
    Explosion {
        x: f32,
        y: f32,
        size: u8,
        /// Ярус, на котором произошёл взрыв.
        deck: u8,
    },
    /// Разрушенный тайл.
    TileBroken {
        x: u16,
        y: u16,
        /// Ярус, на котором разрушен тайл.
        deck: u8,
    },
    /// Подобранный бонус.
    PickupCollected { x: f32, y: f32, kind: u8 },
    /// Уничтожение танка.
    Kill { killer: u32, victim: u32 },
    /// Повышение уровня.
    LevelUp { id: u32, level: u8 },
    /// Применена способность (миномёт, авиаподдержка).
    AbilityUsed { x: f32, y: f32, ability: u8 },
}

impl GameEvent {
    fn kind_byte(self) -> u8 {
        match self {
            GameEvent::Explosion { .. } => 0,
            GameEvent::TileBroken { .. } => 1,
            GameEvent::PickupCollected { .. } => 2,
            GameEvent::Kill { .. } => 3,
            GameEvent::LevelUp { .. } => 4,
            GameEvent::AbilityUsed { .. } => 5,
        }
    }
}

/// Строка таблицы лидеров.
#[derive(Debug, Clone)]
pub struct LeaderboardEntry {
    pub id: u32,
    pub name: String,
    /// Очки (за убийства, бонусы и разрушенные стены).
    pub score: u32,
    /// Число уничтоженных танков — именно это показывается в таблице.
    pub kills: u32,
    pub is_bot: bool,
}

/// Снимок мира, видимый одному игроку.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub tick: u64,
    /// Точка, вокруг которой клиент должен центрировать камеру.
    pub focus_x: f32,
    pub focus_y: f32,
    pub players: Vec<PlayerView>,
    pub bullets: Vec<BulletView>,
    pub pickups: Vec<PickupView>,
    pub tiles: Vec<TileUpdate>,
    pub events: Vec<GameEvent>,
    /// Визуальные эффекты (миномёты, авиаподдержка).
    pub effects: Vec<EffectView>,
    /// Звуковые события — их слышно даже вне конуса обзора.
    pub sounds: Vec<SoundView>,
}

/// Данные карты для начальной синхронизации.
#[derive(Debug, Clone)]
pub struct MapData {
    pub width: u32,
    pub height: u32,
    pub tile_size: u32,
    /// RLE-потоки тайлов: по одному на ярус.
    pub rle: Vec<Vec<u8>>,
}

/// Сообщение, разобранное из клиентского пакета.
#[derive(Debug, Clone)]
pub enum ClientMessage {
    Join {
        nickname: String,
    },
    Input {
        seq: u32,
        input: InputFlags,
        /// Направление башни (0..64).
        aim: u8,
        /// Запрошенная способность.
        ability: u8,
        /// Мировая точка применения способности.
        target_x: f32,
        target_y: f32,
    },
    Respawn,
    Ping {
        client_time: u64,
    },
}

// ---------------------------------------------------------------------------
// Писатель
// ---------------------------------------------------------------------------

/// Компактный писатель в буфер.
#[derive(Debug, Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// Создаёт пустой писатель.
    pub fn new() -> Writer {
        Writer { buf: Vec::with_capacity(256) }
    }

    /// Записывает `u8`.
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    /// Записывает `u16`.
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Записывает `u32`.
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Записывает `u64`.
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Записывает `f32`.
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Записывает сырые байты.
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(v);
        self
    }

    /// Записывает строку с префиксом-длиной `u16`.
    ///
    /// Обрезка идёт **по границе символа**: кириллица занимает два байта,
    /// и простое обрезание по байтам разорвало бы многобайтовый символ —
    /// клиент получил бы невалидный UTF-8 и отбросил бы сообщение.
    pub fn string(&mut self, v: &str) -> &mut Self {
        let mut end = v.len().min(MAX_NICK_LEN);
        while end > 0 && !v.is_char_boundary(end) {
            end -= 1;
        }
        self.u16(end as u16);
        self.buf.extend_from_slice(&v.as_bytes()[..end]);
        self
    }

    /// Завершает запись и отдаёт буфер.
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

// ---------------------------------------------------------------------------
// Читатель
// ---------------------------------------------------------------------------

/// Читатель бинарного сообщения с проверкой границ.
#[derive(Debug)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Создаёт читатель поверх буфера.
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }

    /// Осталось ли что-то читать.
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> ProtocolResult<&'a [u8]> {
        if self.remaining() < n {
            return Err(ProtocolError::Truncated);
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    /// Читает `u8`.
    pub fn u8(&mut self) -> ProtocolResult<u8> {
        Ok(self.take(1)?[0])
    }

    /// Читает `u16`.
    pub fn u16(&mut self) -> ProtocolResult<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    /// Читает `u32`.
    pub fn u32(&mut self) -> ProtocolResult<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Читает `u64`.
    pub fn u64(&mut self) -> ProtocolResult<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// Читает `f32`.
    pub fn f32(&mut self) -> ProtocolResult<f32> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Читает строку с префиксом-длиной `u16`.
    pub fn string(&mut self) -> ProtocolResult<String> {
        let len = self.u16()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ProtocolError::InvalidUtf8)
    }
}

// ---------------------------------------------------------------------------
// Декодирование клиентских сообщений
// ---------------------------------------------------------------------------

/// Разбирает пакет клиента. Любая ошибка возвращается как `Err`, паник нет.
pub fn decode_client(payload: &[u8]) -> ProtocolResult<ClientMessage> {
    if payload.is_empty() {
        return Err(ProtocolError::Empty);
    }
    let mut r = Reader::new(payload);
    let msg_type = r.u8()?;
    match msg_type {
        client_msg::JOIN => Ok(ClientMessage::Join {
            nickname: r.string()?,
        }),
        client_msg::INPUT => {
            let seq = r.u32()?;
            let flags = r.u8()?;
            let aim = r.u8()?;
            let ability = r.u8()?;
            let target_x = r.f32()?;
            let target_y = r.f32()?;
            Ok(ClientMessage::Input {
                seq,
                input: InputFlags::from_byte(flags),
                aim: aim % AIM_COUNT,
                ability,
                target_x,
                target_y,
            })
        }
        client_msg::RESPAWN => Ok(ClientMessage::Respawn),
        client_msg::PING => Ok(ClientMessage::Ping {
            client_time: r.u64()?,
        }),
        other => Err(ProtocolError::UnknownMessage(other)),
    }
}

// ---------------------------------------------------------------------------
// Кодирование серверных сообщений
// ---------------------------------------------------------------------------

/// Кодирует приветствие: идентификатор игрока, тик и полная карта.
pub fn encode_welcome(player_id: u32, tick: u64, map: &MapData) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(server_msg::WELCOME);
    w.u16(PROTOCOL_VERSION);
    w.u32(player_id);
    w.u64(tick);
    write_map(&mut w, map);
    w.finish()
}

/// Кодирует только карту (для фонового отображения до входа в бой).
fn write_map(w: &mut Writer, map: &MapData) {
    w.u32(map.width);
    w.u32(map.height);
    w.u32(map.tile_size);
    w.u8(map.rle.len() as u8);
    for layer in &map.rle {
        w.u32(layer.len() as u32);
        w.bytes(layer);
    }
}

/// Кодирует снимок мира.
pub fn encode_snapshot(snap: &Snapshot) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(server_msg::SNAPSHOT);
    w.u64(snap.tick);
    w.f32(snap.focus_x);
    w.f32(snap.focus_y);

    w.u16(snap.players.len() as u16);
    for p in &snap.players {
        w.u32(p.id);
        w.string(&p.name);
        let mut flags = 0u8;
        if p.is_bot {
            flags |= 1 << 0;
        }
        if p.alive {
            flags |= 1 << 1;
        }
        if p.shielded {
            flags |= 1 << 2;
        }
        w.u8(flags);
        w.f32(p.x);
        w.f32(p.y);
        w.u8(p.dir as u8);
        w.u8(p.aim);
        w.u8(p.deck);
        w.u8(p.level);
        w.u8(p.hp);
        w.u8(p.max_hp);
        w.u32(p.score);
        w.u32(p.kills);
        w.u8(p.ability);
        w.u16(p.mortar_cd);
        w.u16(p.airstrike_cd);
    }

    w.u16(snap.bullets.len() as u16);
    for b in &snap.bullets {
        w.u32(b.id);
        w.f32(b.x);
        w.f32(b.y);
        w.u8(b.dir);
        w.u8(b.deck);
        w.u8(b.level);
    }

    w.u16(snap.pickups.len() as u16);
    for p in &snap.pickups {
        w.u32(p.id);
        w.f32(p.x);
        w.f32(p.y);
        w.u8(p.kind);
        w.u32(p.ttl);
        w.u8(p.deck);
    }

    w.u16(snap.tiles.len() as u16);
    for t in &snap.tiles {
        w.u16(t.x);
        w.u16(t.y);
        w.u8(t.value);
        w.u8(t.deck);
    }

    w.u16(snap.effects.len() as u16);
    for e in &snap.effects {
        w.u32(e.id);
        w.u8(e.kind);
        w.f32(e.x);
        w.f32(e.y);
        w.f32(e.x2);
        w.f32(e.y2);
        w.u8(e.phase);
        w.u8(e.deck);
    }

    w.u16(snap.sounds.len() as u16);
    for s in &snap.sounds {
        w.u32(s.id);
        w.f32(s.x);
        w.f32(s.y);
        w.u8(s.kind);
        w.u32(s.owner);
        w.u8(s.deck);
    }

    w.u16(snap.events.len() as u16);
    for e in &snap.events {
        w.u8(e.kind_byte());
        match *e {
            GameEvent::Explosion { x, y, size, deck } => {
                w.f32(x);
                w.f32(y);
                w.u8(size);
                w.u8(deck);
            }
            GameEvent::TileBroken { x, y, deck } => {
                w.u16(x);
                w.u16(y);
                w.u8(deck);
            }
            GameEvent::PickupCollected { x, y, kind } => {
                w.f32(x);
                w.f32(y);
                w.u8(kind);
            }
            GameEvent::Kill { killer, victim } => {
                w.u32(killer);
                w.u32(victim);
            }
            GameEvent::LevelUp { id, level } => {
                w.u32(id);
                w.u8(level);
            }
            GameEvent::AbilityUsed { x, y, ability } => {
                w.f32(x);
                w.f32(y);
                w.u8(ability);
            }
        }
    }
    w.finish()
}

/// Кодирует таблицу лидеров.
pub fn encode_leaderboard(entries: &[LeaderboardEntry]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(server_msg::LEADERBOARD);
    w.u16(entries.len() as u16);
    for e in entries {
        w.u32(e.id);
        w.string(&e.name);
        w.u32(e.score);
        w.u32(e.kills);
        w.u8(if e.is_bot { 1 } else { 0 });
    }
    w.finish()
}

/// Кодирует сообщение о смерти игрока.
pub fn encode_death(score: u32, kills: u32, deaths: u32, survived_ms: u64) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(server_msg::DEATH);
    w.u32(score);
    w.u32(kills);
    w.u32(deaths);
    w.u64(survived_ms);
    w.finish()
}

/// Кодирует сообщение о возрождении.
pub fn encode_respawned(tick: u64, x: f32, y: f32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(server_msg::RESPAWNED);
    w.u64(tick);
    w.f32(x);
    w.f32(y);
    w.finish()
}

/// Кодирует ответ на пинг.
pub fn encode_pong(client_time: u64, server_time: u64) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(server_msg::PONG);
    w.u64(client_time);
    w.u64(server_time);
    w.finish()
}

/// Кодирует ошибку протокола.
pub fn encode_error(code: u16, message: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(server_msg::ERROR);
    w.u16(code);
    w.string(message);
    w.finish()
}

/// Нормализует никнейм: убирает управляющие символы, обрезает по длине.
pub fn sanitize_nickname(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string();
    let mut out = String::new();
    for c in cleaned.chars() {
        if out.len() + c.len_utf8() > MAX_NICK_LEN {
            break;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_flags_roundtrip() {
        let flags = InputFlags {
            up: true,
            left: true,
            fire: true,
            ..Default::default()
        };
        let byte = input_bit::UP | input_bit::LEFT | input_bit::FIRE;
        assert_eq!(InputFlags::from_byte(byte), flags);
    }

    #[test]
    fn client_message_roundtrip() {
        let mut w = Writer::new();
        w.u8(client_msg::JOIN);
        w.string("Ильза");
        let bytes = w.finish();
        match decode_client(&bytes).expect("decode") {
            ClientMessage::Join { nickname } => assert_eq!(nickname, "Ильза"),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn input_carries_aim_and_ability() {
        let mut w = Writer::new();
        w.u8(client_msg::INPUT);
        w.u32(7);
        w.u8(input_bit::UP | input_bit::RIGHT | input_bit::FIRE | input_bit::USE);
        w.u8(9);
        w.u8(ability::MORTAR);
        w.f32(123.5);
        w.f32(-45.25);
        let bytes = w.finish();
        match decode_client(&bytes).expect("decode") {
            ClientMessage::Input {
                seq,
                input,
                aim,
                ability: ab,
                target_x,
                target_y,
            } => {
                assert_eq!(seq, 7);
                assert!(input.up && input.right && input.fire);
                assert_eq!(aim, 9);
                assert_eq!(ab, ability::MORTAR);
                assert!((target_x - 123.5).abs() < 0.001);
                assert!((target_y + 45.25).abs() < 0.001);
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn aim_has_sixty_four_directions() {
        // Вправо — 0, вниз — 16, влево — 32, вверх — 48.
        assert_eq!(aim_from_vector(1.0, 0.0), 0);
        assert_eq!(aim_from_vector(0.0, 1.0), 16);
        assert_eq!(aim_from_vector(-1.0, 0.0), 32);
        assert_eq!(aim_from_vector(0.0, -1.0), 48);
        // Диагональ вправо-вниз — между 0 и 16, то есть 8.
        assert_eq!(aim_from_vector(1.0, 1.0), 8);
        let (dx, dy) = aim_delta(0);
        assert!((dx - 1.0).abs() < 0.001 && dy.abs() < 0.001);
        let (dx, dy) = aim_delta(16);
        assert!(dx.abs() < 0.001 && (dy - 1.0).abs() < 0.001);
    }

    #[test]
    fn aim_covers_full_circle_without_gaps() {
        // Шаг башни не должен оставлять «дыр» между лучами: иначе край
        // конуса обзора прыгает и блоки в тумане начинают моргать.
        let step = std::f32::consts::PI * 2.0 / AIM_COUNT as f32;
        for i in 0..AIM_COUNT {
            let (dx, dy) = aim_delta(i);
            let len = (dx * dx + dy * dy).sqrt();
            assert!((len - 1.0).abs() < 1e-5, "вектор направления не единичный");
            // Обратное преобразование должно вернуть тот же индекс.
            assert_eq!(aim_from_vector(dx, dy), i);
        }
        assert!((step * AIM_COUNT as f32 - std::f32::consts::PI * 2.0).abs() < 1e-5);
    }

    #[test]
    fn truncated_message_is_error_not_panic() {
        let bytes = vec![client_msg::INPUT, 0x01, 0x02];
        assert!(decode_client(&bytes).is_err());
        assert!(decode_client(&[]).is_err());
        assert!(decode_client(&[0xFF]).is_err());
    }

    #[test]
    fn nickname_is_trimmed_and_bounded() {
        assert_eq!(sanitize_nickname("  Иван  "), "Иван");
        // Лимит — в байтах UTF-8, поэтому кириллица обрезается быстрее.
        assert_eq!(sanitize_nickname("ОченьДлинныйНикнейм").len(), MAX_NICK_LEN);
        assert_eq!(sanitize_nickname("ОченьДлинныйНикнейм"), "ОченьДли");
        assert_eq!(sanitize_nickname("\u{7}bad\n"), "bad");
        assert_eq!(sanitize_nickname(""), "");
    }

    #[test]
    fn writer_truncates_strings_on_a_character_boundary() {
        // Девять кириллических букв — 18 байт. Лимит в 16 байт попадает
        // ровно посередине девятой буквы: обрезать по байтам нельзя.
        let mut w = Writer::new();
        w.string("абвгдеёжзи");
        let bytes = w.finish();
        let len = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
        assert_eq!(len, 16);
        let text = std::str::from_utf8(&bytes[2..]).expect("строка должна остаться UTF-8");
        assert_eq!(text.chars().count(), 8, "обрезка должна быть по символу");
        assert!(text.len() <= MAX_NICK_LEN);
    }

    #[test]
    fn snapshot_encodes_and_reports_length() {
        let snap = Snapshot {
            tick: 42,
            focus_x: 10.0,
            focus_y: 20.0,
            players: vec![PlayerView {
                id: 1,
                name: "Боб".into(),
                is_bot: false,
                x: 1.0,
                y: 2.0,
                dir: Direction::Left,
                aim: 5,
                deck: 0,
                level: 2,
                hp: 3,
                max_hp: 4,
                alive: true,
                shielded: false,
                score: 10,
                kills: 1,
                ability: ability::MORTAR,
                mortar_cd: 30,
                airstrike_cd: 0,
            }],
            bullets: vec![BulletView {
                id: 9,
                x: 5.0,
                y: 6.0,
                dir: 3,
                deck: 0,
                level: 1,
            }],
            pickups: Vec::new(),
            tiles: vec![TileUpdate {
                x: 3,
                y: 4,
                value: 8,
                deck: 0,
            }],
            events: vec![GameEvent::Kill { killer: 1, victim: 2 }],
            effects: vec![EffectView {
                id: 4,
                kind: 1,
                x: 1.0,
                y: 2.0,
                x2: 3.0,
                y2: 4.0,
                phase: 50,
                deck: 0,
            }],
            sounds: vec![SoundView {
                id: 5,
                x: 10.0,
                y: 20.0,
                kind: 0,
                owner: 7,
                deck: 0,
            }],
        };
        let bytes = encode_snapshot(&snap);
        assert_eq!(bytes[0], server_msg::SNAPSHOT);
        assert!(bytes.len() > 32);
    }
}
