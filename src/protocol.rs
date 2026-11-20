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
/// * `2` — восемь направлений (включая диагонали), в лидерборде есть убийства.
pub const PROTOCOL_VERSION: u16 = 2;

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
    pub dir: Direction,
    pub level: u8,
    pub hp: u8,
    pub max_hp: u8,
    pub alive: bool,
    pub shielded: bool,
    pub score: u32,
    pub kills: u32,
}

/// Состояние снаряда.
#[derive(Debug, Clone, Copy)]
pub struct BulletView {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub dir: Direction,
    pub level: u8,
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
}

/// Изменение одного тайла карты.
#[derive(Debug, Clone, Copy)]
pub struct TileUpdate {
    pub x: u16,
    pub y: u16,
    pub value: u8,
}

/// Событие мира для клиента.
#[derive(Debug, Clone, Copy)]
pub enum GameEvent {
    /// Взрыв.
    Explosion { x: f32, y: f32, size: u8 },
    /// Разрушенный тайл.
    TileBroken { x: u16, y: u16 },
    /// Подобранный бонус.
    PickupCollected { x: f32, y: f32, kind: u8 },
    /// Уничтожение танка.
    Kill { killer: u32, victim: u32 },
    /// Повышение уровня.
    LevelUp { id: u32, level: u8 },
}

impl GameEvent {
    fn kind_byte(self) -> u8 {
        match self {
            GameEvent::Explosion { .. } => 0,
            GameEvent::TileBroken { .. } => 1,
            GameEvent::PickupCollected { .. } => 2,
            GameEvent::Kill { .. } => 3,
            GameEvent::LevelUp { .. } => 4,
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
}

/// Данные карты для начальной синхронизации.
#[derive(Debug, Clone)]
pub struct MapData {
    pub width: u32,
    pub height: u32,
    pub tile_size: u32,
    /// RLE-поток: тройки `value:u8, len:u16`.
    pub rle: Vec<u8>,
}

/// Сообщение, разобранное из клиентского пакета.
#[derive(Debug, Clone)]
pub enum ClientMessage {
    Join { nickname: String },
    Input { seq: u32, input: InputFlags },
    Respawn,
    Ping { client_time: u64 },
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
    pub fn string(&mut self, v: &str) -> &mut Self {
        let bytes = v.as_bytes();
        let len = bytes.len().min(MAX_NICK_LEN);
        self.u16(len as u16);
        self.buf.extend_from_slice(&bytes[..len]);
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
            Ok(ClientMessage::Input {
                seq,
                input: InputFlags::from_byte(flags),
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
    w.u32(map.rle.len() as u32);
    w.bytes(&map.rle);
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
        w.u8(p.level);
        w.u8(p.hp);
        w.u8(p.max_hp);
        w.u32(p.score);
        w.u32(p.kills);
    }

    w.u16(snap.bullets.len() as u16);
    for b in &snap.bullets {
        w.u32(b.id);
        w.f32(b.x);
        w.f32(b.y);
        w.u8(b.dir as u8);
        w.u8(b.level);
    }

    w.u16(snap.pickups.len() as u16);
    for p in &snap.pickups {
        w.u32(p.id);
        w.f32(p.x);
        w.f32(p.y);
        w.u8(p.kind);
        w.u32(p.ttl);
    }

    w.u16(snap.tiles.len() as u16);
    for t in &snap.tiles {
        w.u16(t.x);
        w.u16(t.y);
        w.u8(t.value);
    }

    w.u16(snap.events.len() as u16);
    for e in &snap.events {
        w.u8(e.kind_byte());
        match *e {
            GameEvent::Explosion { x, y, size } => {
                w.f32(x);
                w.f32(y);
                w.u8(size);
            }
            GameEvent::TileBroken { x, y } => {
                w.u16(x);
                w.u16(y);
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
                level: 2,
                hp: 3,
                max_hp: 4,
                alive: true,
                shielded: false,
                score: 10,
                kills: 1,
            }],
            bullets: vec![BulletView {
                id: 9,
                x: 5.0,
                y: 6.0,
                dir: Direction::Up,
                level: 1,
            }],
            pickups: Vec::new(),
            tiles: vec![TileUpdate { x: 3, y: 4, value: 8 }],
            events: vec![GameEvent::Kill { killer: 1, victim: 2 }],
        };
        let bytes = encode_snapshot(&snap);
        assert_eq!(bytes[0], server_msg::SNAPSHOT);
        assert!(bytes.len() > 32);
    }
}
