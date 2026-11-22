//! Карта мира: тайлы, ярусы, повреждаемость, генерация, RLE, чанки.
//!
//! Карта состоит из нескольких **ярусов** (уровней по высоте). Вид сверху,
//! но этажей несколько: нижний ярус — земля, верхний — площадки и перекрытия.
//! Переход между ярусами — пандус (`TileKind::Ramp`): полоса из трёх тайлов,
//! у каждого тайла записаны направление полосы и его номер в ней. Пандус есть
//! сразу на обоих ярусах: проехав его целиком, танк меняет этаж.

pub const TILE_SIZE: f32 = 32.0;

pub const MAP_W: u32 = 160;
pub const MAP_H: u32 = 160;

pub const CHUNK_TILES: u32 = 16;
pub const CHUNKS_X: u32 = MAP_W / CHUNK_TILES;
pub const CHUNKS_Y: u32 = MAP_H / CHUNK_TILES;

pub const VIEW_CHUNK_RADIUS: i32 = 3;

pub const BRICK_MAX_DAMAGE: u8 = 4;

/// Количество слоёв карты (уровней по высоте).
pub const LEVELS: u8 = 3;

/// Слой ямок (окопов): дно ниже основной земли.
pub const LAYER_PIT: u8 = 0;
/// Слой основной земли.
pub const LAYER_GROUND: u8 = 1;
/// Слой возвышенностей и мостиков: настил над землёй.
pub const LAYER_UPPER: u8 = 2;

/// Высота ямки: ниже земли.
pub const ELEV_PIT: i8 = -1;
/// Высота основной земли.
pub const ELEV_GROUND: i8 = 0;
/// Высота возвышенностей и мостиков.
pub const ELEV_UPPER: i8 = 1;

/// Высота слоя: слой 0 — это минус один, слой 1 — ноль, слой 2 — плюс один.
#[inline]
pub fn layer_elev(layer: u8) -> i8 {
    layer as i8 - 1
}

/// Слой по высоте: −1 → 0, 0 → 1, +1 → 2.
#[inline]
pub fn elev_layer(elev: i8) -> u8 {
    (elev + 1).clamp(0, 2) as u8
}

/// Длина пандуса в тайлах: полоса из трёх тайлов.
pub const RAMP_LEN: u8 = 3;

/// Направление пандуса: подъём вправо.
pub const RAMP_DIR_RIGHT: u8 = 0;
/// Направление пандуса: спуск вниз.
pub const RAMP_DIR_DOWN: u8 = 1;
/// Направление пандуса: подъём влево.
pub const RAMP_DIR_LEFT: u8 = 2;
/// Направление пандуса: подъём вверх.
pub const RAMP_DIR_UP: u8 = 3;
/// Пандуса нет: значение для «танк не на пандусе».
///
/// Вынесено за пределы направлений (0..=3): иначе полосу «вправо» (0) нельзя
/// было бы отличить от простоя.
pub const RAMP_DIR_NONE: u8 = 4;

/// Тип тайла.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TileKind {
    /// Пустота — проходима для всех.
    Empty = 0,
    /// Кирпичная стена — разрушается выстрелами.
    Brick = 1,
    /// Бетонная стена — пробивается только снарядом 3-го уровня.
    Steel = 2,
    /// Вода — танки не проезжают, снаряды летят сквозь.
    Water = 3,
    /// Лес — проходим, скрывает танк от врагов снаружи массива.
    Forest = 4,
    /// Лёд — скользкая поверхность с инерцией.
    Ice = 5,
    /// Трамплин — переход между слоями.
    Ramp = 6,
    /// Дно ямки — проходимая поверхность ниже земли.
    Pit = 7,
}

impl TileKind {
    /// Разбирает базовый тип из битовой маски.
    pub fn from_bits(bits: u8) -> TileKind {
        match bits & 0x07 {
            1 => TileKind::Brick,
            2 => TileKind::Steel,
            3 => TileKind::Water,
            4 => TileKind::Forest,
            5 => TileKind::Ice,
            6 => TileKind::Ramp,
            7 => TileKind::Pit,
            _ => TileKind::Empty,
        }
    }

    /// Базовый тип в виде маски.
    pub fn bits(self) -> u8 {
        self as u8
    }

    /// Блокирует ли тайл движение танка.
    pub fn blocks_tank(self) -> bool {
        matches!(self, TileKind::Brick | TileKind::Steel | TileKind::Water)
    }

    /// Блокирует ли тайл снаряд.
    pub fn blocks_bullet(self) -> bool {
        matches!(self, TileKind::Brick | TileKind::Steel)
    }

    /// Скрывает ли тайл танк внутри себя.
    pub fn hides_tank(self) -> bool {
        matches!(self, TileKind::Forest)
    }

    /// Скользкий ли тайл (низкое трение).
    pub fn is_slippery(self) -> bool {
        matches!(self, TileKind::Ice)
    }

    /// Является ли тайл переходом между слоями.
    pub fn is_ramp(self) -> bool {
        matches!(self, TileKind::Ramp)
    }

    /// Является ли тайл дном ямки.
    pub fn is_pit_floor(self) -> bool {
        matches!(self, TileKind::Pit)
    }
}

// ---------------------------------------------------------------------------
// Формат тайла-пандуса
// ---------------------------------------------------------------------------

/// Направление пандуса, закодированное в значении тайла.
///
/// Младшие три бита занимает базовый тип тайла, биты 3..4 — направление:
/// от нижнего края полосы (номер 0) к верхнему (номер 2).
pub fn ramp_dir(value: u8) -> u8 {
    (value >> 3) & 0x03
}

/// Номер тайла в полосе пандуса: 0 — нижний край, 1 — середина, 2 — верхний.
pub fn ramp_index(value: u8) -> u8 {
    (value >> 5) & 0x03
}

/// Бит 7: пандус ведёт вниз, в ямку (земля ↔ ямка).
///
/// Без этого бита пандус всегда ведёт вверх, с земли на возвышенность.
pub const RAMP_FLAG_PIT: u8 = 0x80;

/// Бит 3 у пустого тайла верхнего яруса: это **настил** — по нему ездят.
///
/// Смысл бита зависит от типа тайла: у пандуса биты 3..4 заняты
/// направлением полосы, у кирпича — стадией повреждения.
pub const DECK_FLAG: u8 = 0x08;

/// Бит 4 у настила верхнего яруса: это **мостик** (деревянный), а не холм.
pub const BRIDGE_FLAG: u8 = 0x10;

/// Собирает значение тайла-пандуса: тип, направление и номер в полосе.
///
/// `to_pit` — ведёт ли полоса вниз, в ямку.
pub fn ramp_value(dir: u8, idx: u8, to_pit: bool) -> u8 {
    TileKind::Ramp.bits()
        | ((dir & 0x03) << 3)
        | ((idx & 0x03) << 5)
        | if to_pit { RAMP_FLAG_PIT } else { 0 }
}

/// Ведёт ли пандус вниз, в ямку.
pub fn ramp_is_pit(value: u8) -> bool {
    value & RAMP_FLAG_PIT != 0
}

/// Низкая сторона полосы: там, откуда полоса поднимается.
pub fn ramp_low_layer(value: u8) -> u8 {
    if ramp_is_pit(value) {
        LAYER_PIT
    } else {
        LAYER_GROUND
    }
}

/// Высокая сторона полосы: там, куда полоса ведёт.
pub fn ramp_high_layer(value: u8) -> u8 {
    if ramp_is_pit(value) {
        LAYER_GROUND
    } else {
        LAYER_UPPER
    }
}

/// Смещение соседнего тайла в направлении пандуса.
pub fn ramp_dir_delta(dir: u8) -> (i32, i32) {
    match dir & 0x03 {
        RAMP_DIR_RIGHT => (1, 0),
        RAMP_DIR_DOWN => (0, 1),
        RAMP_DIR_LEFT => (-1, 0),
        _ => (0, -1),
    }
}

/// Направление пандуса по смещению: `(1, 0)` — вправо и так далее.
pub fn dir_from_delta(dx: i32, dy: i32) -> u8 {
    match (dx, dy) {
        (1, 0) => RAMP_DIR_RIGHT,
        (0, 1) => RAMP_DIR_DOWN,
        (-1, 0) => RAMP_DIR_LEFT,
        _ => RAMP_DIR_UP,
    }
}
// ---------------------------------------------------------------------------
// Генератор случайных чисел
// ---------------------------------------------------------------------------

/// Детерминированный xorshift64* — без внешних зависимостей.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Создаёт генератор с заданным зерном.
    pub fn new(seed: u64) -> Rng {
        Rng {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    /// Следующее 64-битное значение.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Следующее 32-битное значение.
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Случайное число из диапазона `0..n` (`n > 0`).
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        self.next_u32() % n
    }

    /// Случайное число из диапазона `lo..=hi`.
    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        if hi <= lo {
            return lo;
        }
        lo + self.below(hi - lo + 1)
    }

    /// Случайное число с плавающей точкой в диапазоне `0..1`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Вероятность в процентах.
    pub fn chance(&mut self, percent: u32) -> bool {
        self.below(100) < percent
    }
}

// ---------------------------------------------------------------------------
// Карта
// ---------------------------------------------------------------------------

/// Прямоугольник ямки в тайлах.
#[derive(Debug, Clone, Copy)]
struct PitRect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl PitRect {
    /// Входит ли тайл в ямку.
    fn contains(&self, tx: u32, ty: u32) -> bool {
        tx >= self.x && tx < self.x + self.w && ty >= self.y && ty < self.y + self.h
    }
}

/// Налезают ли две ямки друг на друга (с запасом в один тайл).
fn pits_overlap(a: &PitRect, b: &PitRect) -> bool {
    a.x < b.x + b.w + 2
        && b.x < a.x + a.w + 2
        && a.y < b.y + b.h + 2
        && b.y < a.y + a.h + 2
}

/// Тайловая карта мира с несколькими слоями по высоте.
#[derive(Debug, Clone)]
pub struct GameMap {
    width: u32,
    height: u32,
    /// Упакованные значения тайлов: по одному массиву на слой.
    layers: Vec<Vec<u8>>,
    /// Идентификатор лесного массива для каждого тайла (`u32::MAX` — не лес).
    forest_regions: Vec<Vec<u32>>,
    /// Прямоугольники ямок: нужны, чтобы мостики легли именно над ними.
    pits: Vec<PitRect>,
}

impl GameMap {
    /// Генерирует карту заданного размера с зерном.
    pub fn generate(width: u32, height: u32, seed: u64) -> GameMap {
        let total = (width * height) as usize;
        let mut map = GameMap {
            width,
            height,
            layers: vec![vec![0; total]; LEVELS as usize],
            forest_regions: vec![vec![u32::MAX; total]; LEVELS as usize],
            pits: Vec::new(),
        };
        map.build(seed);
        map.build_pits(seed ^ 0x1A2B_3C4D_5E6F_7788);
        // Связность проверяем после ямок: дыры в земле тоже перекрывают путь.
        map.ensure_connectivity(width / 2, height / 2);
        map.build_upper_level(seed ^ 0x5DEE_CE66_D1CE_4B0D);
        map.rebuild_forest_regions();
        map
    }

    /// Ширина карты в тайлах.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Высота карты в тайлах.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Количество ярусов.
    pub fn levels(&self) -> u8 {
        self.layers.len() as u8
    }

    /// Количество тайлов на одном ярусе.
    pub fn len(&self) -> usize {
        (self.width * self.height) as usize
    }

    #[inline]
    fn index(&self, tx: u32, ty: u32) -> Option<usize> {
        if tx >= self.width || ty >= self.height {
            return None;
        }
        Some((ty * self.width + tx) as usize)
    }

    /// Нормализует номер яруса.
    #[inline]
    fn level_index(&self, level: u8) -> usize {
        (level as usize).min(self.layers.len() - 1)
    }

    /// Упакованное значение тайла.
    pub fn value(&self, tx: u32, ty: u32, level: u8) -> u8 {
        match self.index(tx, ty) {
            Some(i) => self.layers[self.level_index(level)][i],
            None => TileKind::Steel.bits(),
        }
    }

    /// Базовый тип тайла.
    pub fn kind(&self, tx: u32, ty: u32, level: u8) -> TileKind {
        TileKind::from_bits(self.value(tx, ty, level))
    }

    /// Стадия повреждения тайла.
    ///
    /// Повреждения бывают только у кирпича: у пандуса старшие биты заняты
    /// направлением полосы и номером тайла в ней.
    pub fn damage(&self, tx: u32, ty: u32, level: u8) -> u8 {
        let value = self.value(tx, ty, level);
        if TileKind::from_bits(value) != TileKind::Brick {
            return 0;
        }
        value >> 3
    }

    /// Записывает упакованное значение тайла.
    pub fn set_value(&mut self, tx: u32, ty: u32, level: u8, value: u8) {
        if let Some(i) = self.index(tx, ty) {
            let li = self.level_index(level);
            self.layers[li][i] = value;
        }
    }

    /// Записывает базовый тип тайла, сбрасывая повреждение.
    pub fn set_kind(&mut self, tx: u32, ty: u32, level: u8, kind: TileKind) {
        self.set_value(tx, ty, level, kind.bits());
    }

    /// Блокирует ли тайл движение танка — с учётом слоя.
    ///
    /// На слое ямок проходимы только дно (`Pit`) и пандусы: всё остальное —
    /// сплошная земля над головой. На земле тайл непроходим, если под ним
    /// ямка: дыру в землю случайно не проехать, только по спускному пандусу.
    /// На верхнем ярусе пустота — пропасть: ездить можно только по настилам
    /// мостов и холмов.
    pub fn blocks_tank(&self, tx: u32, ty: u32, layer: u8) -> bool {
        let kind = self.kind(tx, ty, layer);
        match layer {
            LAYER_PIT => !matches!(kind, TileKind::Pit | TileKind::Ramp),
            LAYER_GROUND => {
                if kind.blocks_tank() {
                    return true;
                }
                let below = self.kind(tx, ty, LAYER_PIT);
                // Дыра в земле: под тайлом дно ямки или её стенка.
                below.is_pit_floor() || below == TileKind::Brick
            }
            _ => {
                if kind.blocks_tank() {
                    return true;
                }
                // Пустота верхнего яруса — пропасть: проезжают только настилы.
                kind == TileKind::Empty && !self.is_deck(tx, ty)
            }
        }
    }

    /// Является ли тайл верхнего яруса настилом, по которому можно ездить.
    pub fn is_deck(&self, tx: u32, ty: u32) -> bool {
        self.value(tx, ty, LAYER_UPPER) & DECK_FLAG != 0
    }

    /// Помечает тайл верхнего яруса как настил.
    pub fn set_deck(&mut self, tx: u32, ty: u32, bridge: bool) {
        let value = self.value(tx, ty, LAYER_UPPER);
        let mut next = value | DECK_FLAG;
        if bridge {
            next |= BRIDGE_FLAG;
        }
        self.set_value(tx, ty, LAYER_UPPER, next);
    }

    /// Является ли тайл переходом между ярусами.
    pub fn is_ramp(&self, tx: u32, ty: u32, level: u8) -> bool {
        self.kind(tx, ty, level).is_ramp()
    }

    /// Находится ли точка (в пикселях) внутри лесного массива.
    pub fn forest_region(&self, tx: u32, ty: u32, level: u8) -> Option<u32> {
        let idx = self.index(tx, ty)?;
        let region = self.forest_regions[self.level_index(level)][idx];
        if region == u32::MAX {
            None
        } else {
            Some(region)
        }
    }

    /// Тайл под мировой точкой (в пикселях).
    pub fn tile_at(&self, x: f32, y: f32) -> (u32, u32) {
        let tx = (x / TILE_SIZE).floor().max(0.0) as u32;
        let ty = (y / TILE_SIZE).floor().max(0.0) as u32;
        (tx.min(self.width.saturating_sub(1)), ty.min(self.height.saturating_sub(1)))
    }

    /// Индекс чанка для мировой точки (в пикселях).
    pub fn chunk_at(&self, x: f32, y: f32) -> (u32, u32) {
        let (tx, ty) = self.tile_at(x, y);
        (tx / CHUNK_TILES, ty / CHUNK_TILES)
    }

    /// RLE-кодирование яруса: тройки `value:u8, len:u16`.
    pub fn rle_encode_level(&self, level: u8) -> Vec<u8> {
        let tiles = &self.layers[self.level_index(level)];
        let mut out = Vec::with_capacity(tiles.len() / 2);
        let mut i = 0usize;
        while i < tiles.len() {
            let value = tiles[i];
            let mut run = 1u16;
            while (i + run as usize) < tiles.len() && tiles[i + run as usize] == value && run < u16::MAX
            {
                run += 1;
            }
            out.push(value);
            out.extend_from_slice(&run.to_le_bytes());
            i += run as usize;
        }
        out
    }

    /// RLE-кодирование всех ярусов.
    pub fn rle_encode(&self) -> Vec<Vec<u8>> {
        (0..self.layers.len() as u8)
            .map(|level| self.rle_encode_level(level))
            .collect()
    }

    /// Расшифровывает RLE-поток обратно в массив тайлов.
    pub fn rle_decode(rle: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
        let total = (width as usize) * (height as usize);
        let mut out = Vec::with_capacity(total);
        let mut i = 0usize;
        while i + 3 <= rle.len() {
            let value = rle[i];
            let run = u16::from_le_bytes([rle[i + 1], rle[i + 2]]) as usize;
            i += 3;
            if out.len() + run > total {
                return None;
            }
            out.extend(std::iter::repeat_n(value, run));
        }
        if out.len() != total {
            return None;
        }
        Some(out)
    }

    // -----------------------------------------------------------------------
    // Генерация
    // -----------------------------------------------------------------------
    fn build(&mut self, seed: u64) {
        let mut rng = Rng::new(seed);
        let w = self.width;
        let h = self.height;

        // 1. Рамка: внешний слой — бетон, внутренний — кирпич.
        for y in 0..h {
            for x in 0..w {
                let edge = x == 0 || y == 0 || x == w - 1 || y == h - 1;
                let near_edge = x == 1 || y == 1 || x == w - 2 || y == h - 2;
                if edge {
                    self.set_kind(x, y, LAYER_GROUND, TileKind::Steel);
                } else if near_edge {
                    self.set_kind(x, y, LAYER_GROUND, TileKind::Brick);
                }
            }
        }

        // 2. Симметричные блоки структур. Строим только левый верхний квадрант
        //    и зеркалим в остальные, чтобы карта была симметричной, как в Battle City.
        let qw = w / 2;
        let qh = h / 2;
        for y in 2..qh {
            for x in 2..qw {
                if rng.chance(6) {
                    let kind = match rng.below(100) {
                        0..=34 => TileKind::Brick,
                        35..=54 => TileKind::Forest,
                        55..=64 => TileKind::Water,
                        65..=74 => TileKind::Ice,
                        _ => continue,
                    };
                    let bw = match rng.below(100) {
                        0..=54 => 2,
                        55..=84 => 3,
                        _ => 4,
                    };
                    let bh = match rng.below(100) {
                        0..=54 => 2,
                        55..=84 => 3,
                        _ => 4,
                    };
                    self.place_symmetric(x, y, bw, bh, kind);
                }
            }
        }

        // 3. Открываем центральную зону, чтобы всегда было место для манёвра.
        let cx = w / 2;
        let cy = h / 2;
        for y in cy - 4..cy + 5 {
            for x in cx - 4..cx + 5 {
                if x > 1 && y > 1 && x < w - 2 && y < h - 2 {
                    self.set_kind(x, y, LAYER_GROUND, TileKind::Empty);
                }
            }
        }

        // 4. Связность земли проверяется после построения ямок:
        //    дыры в земле тоже перекрывают путь.
    }

    /// Ставит блок тайлов с зеркалированием по четырём квадрантам.
    fn place_symmetric(&mut self, x: u32, y: u32, bw: u32, bh: u32, kind: TileKind) {
        let w = self.width;
        let h = self.height;
        for dy in 0..bh {
            for dx in 0..bw {
                let px = x + dx;
                let py = y + dy;
                if px < 2 || py < 2 || px >= w - 2 || py >= h - 2 {
                    continue;
                }
                // Отступаем от зеркальной оси, иначе блоки схлопываются.
                let right = w - 1 - px;
                let bottom = h - 1 - py;
                if px < right && py < bottom {
                    self.set_kind(px, py, LAYER_GROUND, kind);
                    self.set_kind(right, py, LAYER_GROUND, kind);
                    self.set_kind(px, bottom, LAYER_GROUND, kind);
                    self.set_kind(right, bottom, LAYER_GROUND, kind);
                }
            }
        }
    }

    /// Гарантирует, что все пустые области связаны с центром карты.
    ///
    /// Дыры ямок проезжаемы только по спускному пандусу, поэтому обход
    /// считает их закрытыми; сами ямки при этом не замуровываются.
    ///
    /// Ищутся области связности свободных тайлов земли. Основной считается
    /// та, что содержит центр (если центр свободен и не зажат в крошечный
    /// карман), иначе — самая большая. Остальные области — «карманы»: их
    /// тайлы замуровываются кирпичом.
    ///
    /// Так карта остаётся цельной даже тогда, когда центр оказался внутри
    /// ямки: раньше заливка от центра обрывалась сразу, и весь мир
    /// превращался в кирпич.
    fn ensure_connectivity(&mut self, cx: u32, cy: u32) {
        let total = (self.width * self.height) as usize;
        let mut comp = vec![u32::MAX; total];
        let mut comps: Vec<Vec<u32>> = Vec::new();
        let mut stack: Vec<u32> = Vec::new();

        for start in 0..total {
            if comp[start] != u32::MAX {
                continue;
            }
            let sx = (start % self.width as usize) as u32;
            let sy = (start / self.width as usize) as u32;
            if self.blocks_tank(sx, sy, LAYER_GROUND) {
                continue;
            }
            let id = comps.len() as u32;
            let mut cells: Vec<u32> = Vec::with_capacity(64);
            comp[start] = id;
            stack.push(start as u32);
            while let Some(idx) = stack.pop() {
                cells.push(idx);
                let x = (idx as u32) % self.width;
                let y = (idx as u32) / self.width;
                let neighbours = [
                    (x.wrapping_sub(1), y),
                    (x + 1, y),
                    (x, y.wrapping_sub(1)),
                    (x, y + 1),
                ];
                for (nx, ny) in neighbours {
                    if nx >= self.width || ny >= self.height {
                        continue;
                    }
                    let nidx = ny as usize * self.width as usize + nx as usize;
                    if comp[nidx] != u32::MAX {
                        continue;
                    }
                    if self.blocks_tank(nx, ny, LAYER_GROUND) {
                        continue;
                    }
                    comp[nidx] = id;
                    stack.push(nidx as u32);
                }
            }
            comps.push(cells);
        }

        // Основная область: самая большая, а если центр входит в область
        // сопоставимого размера — та, в которой находится центр.
        let mut best = 0usize;
        let mut best_len = 0usize;
        for (i, cells) in comps.iter().enumerate() {
            if cells.len() > best_len {
                best_len = cells.len();
                best = i;
            }
        }
        let center_idx = (cy as usize * self.width as usize + cx as usize) as u32;
        let center_comp = comp[center_idx as usize];
        if center_comp != u32::MAX {
            let center_len = comps[center_comp as usize].len();
            if center_len * 2 >= best_len {
                best = center_comp as usize;
            }
        }

        // Остальные области — глухие карманы: замуровываем их кирпичом.
        for (i, cells) in comps.iter().enumerate() {
            if i == best {
                continue;
            }
            for &idx in cells {
                let x = (idx as u32) % self.width;
                let y = (idx as u32) / self.width;
                // Ямки не замуровываем: они и есть «дыры» в земле.
                if self.is_pit(x, y) {
                    continue;
                }
                self.set_kind(x, y, LAYER_GROUND, TileKind::Brick);
            }
        }
    }

    /// Строит ямки (окопы) ниже земли: дыра в земле, дно и спускной пандус.
    ///
    /// Ямка устроена так:
    ///  - на слое **земли** её тайлы пустые — это дыра, в которую с поверхности
    ///    нельзя уехать, только по спускному пандусу;
    ///  - на слое **ямок** лежит дно (`Pit`) — по нему можно ездить;
    ///  - спускной пандус — полоса из трёх тайлов: край земли, откос, дно.
    fn build_pits(&mut self, seed: u64) {
        let mut rng = Rng::new(seed);
        let w = self.width;
        let h = self.height;
        let count = 5 + rng.below(4);
        let mut attempts = 0u32;
        // Центральная зона — место для манёвра и появления игроков: ямка здесь
        // перекрыла бы путь из центра и разорвала карту.
        let protect = 8i64;
        let (bx0, bx1) = (w as i64 / 2 - protect, w as i64 / 2 + protect);
        let (by0, by1) = (h as i64 / 2 - protect, h as i64 / 2 + protect);

        while self.pits.len() < count as usize && attempts < 800 {
            attempts += 1;
            // Размер ямки задан с запасом на стенки окопа: внутри кольца из
            // кирпича должно остаться место, где танк свободно разворачивается.
            let pw = rng.range(6, 9);
            let ph = rng.range(5, 7);
            let px = rng.range(3, w.saturating_sub(pw + 3));
            let py = rng.range(3, h.saturating_sub(ph + 3));
            let pit = PitRect {
                x: px,
                y: py,
                w: pw,
                h: ph,
            };

            // Ямки не наезжают друг на друга.
            if self.pits.iter().any(|p| pits_overlap(p, &pit)) {
                continue;
            }
            // Ямка не должна закрывать центр карты.
            let (pxi, pyi, pwi, phi) = (px as i64, py as i64, pw as i64, ph as i64);
            if pxi < bx1 && pxi + pwi > bx0 && pyi < by1 && pyi + phi > by0 {
                continue;
            }
            // Полоса спуска: край земли (сверху), откос, дно (в ямке).
            // Полоса идёт на тайл глубже стенки окопа, чтобы пересечь её
            // насквозь: вход в ямку — это пролом в кладке.
            let strip = [(px + 1, py - 1), (px + 1, py), (px + 1, py + 1)];
            // Подход к пандусу должен быть свободен.
            if !self.ramp_area_free(px + 1, py - 1) {
                continue;
            }
            // Под ямкой не должно быть глухих карманов: земля под ней обычная.
            let mut blocked = false;
            for ty in py..py + ph {
                for tx in px..px + pw {
                    if self.kind(tx, ty, LAYER_GROUND).blocks_tank() {
                        blocked = true;
                    }
                }
            }
            if blocked {
                continue;
            }

            // Дыра в земле: тайлы ямки на слое земли пустые.
            let pit_layer = elev_layer(ELEV_PIT);
            for ty in py..py + ph {
                for tx in px..px + pw {
                    self.set_kind(tx, ty, LAYER_GROUND, TileKind::Empty);
                }
            }
            // Стенки окопа: кольцо по периметру ямки на слое ямок — кирпич.
            // Внутри кольца — дно (`Pit`), по нему можно ездить.
            for ty in py..py + ph {
                for tx in px..px + pw {
                    let edge = tx == px || tx == px + pw - 1 || ty == py || ty == py + ph - 1;
                    if edge {
                        self.set_kind(tx, ty, pit_layer, TileKind::Brick);
                    }
                }
            }
            // Дно ямки на слое ямок.
            for ty in py..py + ph {
                for tx in px..px + pw {
                    let edge = tx == px || tx == px + pw - 1 || ty == py || ty == py + ph - 1;
                    if !edge {
                        self.set_kind(tx, ty, pit_layer, TileKind::Pit);
                    }
                }
            }
            // Спускной пандус: те же тайлы на обоих слоях пары. Он проходит
            // сквозь стенку окопа — это и есть вход в ямку.
            for (k, &(tx, ty)) in strip.iter().enumerate() {
                // Номер в полосе: 2 — край земли, 0 — дно ямки.
                let idx = (RAMP_LEN - 1 - k as u8) as u8;
                let value = ramp_value(RAMP_DIR_UP, idx, true);
                self.set_value(tx, ty, LAYER_GROUND, value);
                self.set_value(tx, ty, pit_layer, value);
            }

            self.pits.push(pit);
        }
    }

    /// Является ли тайл частью ямки.
    pub fn is_pit(&self, tx: u32, ty: u32) -> bool {
        self.pits.iter().any(|p| p.contains(tx, ty))
    }

    /// Количество ямок на карте.
    pub fn pits_count(&self) -> usize {
        self.pits.len()
    }

    /// Строит верхний ярус: холмы, мостики и пандусы между слоями.
    ///
    /// Верхний ярус — **не сплошное поле**, а тактическая сеть:
    ///  - **холмы** — возвышенные платформы, по ним ездят, на них стоят
    ///    кирпичные заграждения;
    ///  - **мостики** — узкие настилы, перекинутые над землёй и над ямками.
    ///
    /// И то, и другое соединено с землёй пандусами: без пандуса холм
    /// недостижим, поэтому такой холм просто не ставится.
    fn build_upper_level(&mut self, seed: u64) {
        let mut rng = Rng::new(seed);
        let w = self.width;
        let h = self.height;

        // 1. Холмы: возвышенные платформы верхнего яруса.
        let blocks = 10 + rng.below(8);
        let mut placed = 0u32;
        let mut attempts = 0u32;
        while placed < blocks && attempts < 600 {
            attempts += 1;
            let bw = rng.range(3, 9);
            let bh = rng.range(3, 7);
            let bx = rng.range(3, w.saturating_sub(bw + 3));
            let by = rng.range(3, h.saturating_sub(bh + 3));

            // Тайлы платформы: над проходимой землёй, без ямок.
            let mut tiles: Vec<(u32, u32)> = Vec::new();
            for dy in 0..bh {
                for dx in 0..bw {
                    let px = bx + dx;
                    let py = by + dy;
                    if px < 2 || py < 2 || px >= w - 2 || py >= h - 2 {
                        continue;
                    }
                    // Под холмом должно быть проходимо — иначе нижний
                    // ярус получит непроходимый «слепой» карман.
                    if self.kind(px, py, LAYER_GROUND).blocks_tank() || self.is_pit(px, py) {
                        continue;
                    }
                    tiles.push((px, py));
                }
            }
            if tiles.len() < 6 {
                continue;
            }
            // Платформа — настил верхнего яруса.
            for &(px, py) in &tiles {
                self.set_kind(px, py, LAYER_UPPER, TileKind::Empty);
                self.set_deck(px, py, false);
            }
            // Пара кирпичных заграждений на самой платформе.
            let obstacles = rng.below(3);
            for _ in 0..obstacles {
                if tiles.is_empty() {
                    break;
                }
                let idx = rng.below(tiles.len() as u32) as usize;
                let (px, py) = tiles[idx];
                self.set_kind(px, py, LAYER_UPPER, TileKind::Brick);
            }
            // Пандус с земли на холм. Если он не помещается — холм убираем.
            if !self.place_hill_ramp(&tiles) {
                for &(px, py) in &tiles {
                    self.set_value(px, py, LAYER_UPPER, TileKind::Empty.bits());
                }
                continue;
            }
            placed += 1;
        }

        // 2. Мостики. Сначала — через ямки, потом — несколько над землёй.
        let pits = self.pits.clone();
        for pit in &pits {
            if pit.w >= pit.h {
                let y = pit.y + pit.h / 2;
                let x0 = pit.x.saturating_sub(1);
                let len = pit.w + 2;
                self.place_bridge(x0, y, 1, 0, len);
            } else {
                let x = pit.x + pit.w / 2;
                let y0 = pit.y.saturating_sub(1);
                let len = pit.h + 2;
                self.place_bridge(x, y0, 0, 1, len);
            }
        }
        // Мостиков над землёй — тем больше, чем больше ямок на карте.
        let extra_bridges = 4 + self.pits_count() as u32 / 2;
        for _ in 0..extra_bridges {
            let len = rng.range(3, 7);
            if rng.chance(50) {
                let y = rng.range(3, h.saturating_sub(len + 3));
                let x0 = rng.range(3, w.saturating_sub(len + 3));
                self.place_bridge(x0, y, 1, 0, len);
            } else {
                let x = rng.range(3, w.saturating_sub(len + 3));
                let y0 = rng.range(3, h.saturating_sub(len + 3));
                self.place_bridge(x, y0, 0, 1, len);
            }
        }
    }

    /// Пристраивает к платформе холма пандус с земли.
    ///
    /// Пробуются все крайние тайлы платформы и все четыре направления:
    /// полоса из трёх тайлов должна поместиться на свободном месте.
    fn place_hill_ramp(&mut self, tiles: &[(u32, u32)]) -> bool {
        let w = self.width;
        let h = self.height;
        for &(px, py) in tiles {
            // К холму пристраиваем пандус только к чистому настилу: кирпичное
            // заграждение на выходе из полосы перекрыло бы проезд.
            if !self.is_deck(px, py) {
                continue;
            }
            for out in [
                RAMP_DIR_RIGHT,
                RAMP_DIR_DOWN,
                RAMP_DIR_LEFT,
                RAMP_DIR_UP,
            ] {
                let (dx, dy) = ramp_dir_delta(out);
                let mut strip = [(0u32, 0u32); RAMP_LEN as usize];
                let mut fits = true;
                for k in 0..RAMP_LEN as usize {
                    let step = k as i64 + 1;
                    let (nx, ny) = (px as i64 + dx as i64 * step, py as i64 + dy as i64 * step);
                    if nx < 2 || ny < 2 || nx >= w as i64 - 2 || ny >= h as i64 - 2 {
                        fits = false;
                        break;
                    }
                    let (tx, ty) = (nx as u32, ny as u32);
                    // Полоса идёт наружу от холма, а не по самой платформе.
                    if tiles.contains(&(tx, ty)) {
                        fits = false;
                        break;
                    }
                    // Полоса должна быть свободна на обоих слоях пары.
                    if self.kind(tx, ty, LAYER_UPPER) != TileKind::Empty
                        || self.kind(tx, ty, LAYER_GROUND).is_ramp()
                        || self.is_pit(tx, ty)
                        || !self.ramp_area_free(tx, ty)
                    {
                        fits = false;
                        break;
                    }
                    strip[k] = (tx, ty);
                }
                if !fits {
                    continue;
                }
                // Подъём по пандусу идёт в сторону, противоположную его уходу.
                let dir = (out + 2) & 0x03;
                for (k, &(tx, ty)) in strip.iter().enumerate() {
                    let value = ramp_value(dir, RAMP_LEN - 1 - k as u8, false);
                    self.set_value(tx, ty, LAYER_GROUND, value);
                    self.set_value(tx, ty, LAYER_UPPER, value);
                }
                return true;
            }
        }
        false
    }

    /// Ставит мостик верхнего яруса: проходимый коридор и пандус к нему.
    ///
    /// Коридор идёт от точки `(x0, y0)` в направлении `(dx, dy)` длиной
    /// `len` тайлов. Пандус пристраивается к началу коридора с противоположной
    /// стороны и ведёт с земли на верхний ярус.
    ///
    /// Под мостиком может быть ямка: мост идёт над ней, а земля под ним —
    /// дыра, в которую с земли не уедешь.
    fn place_bridge(&mut self, x0: u32, y0: u32, dx: i32, dy: i32, len: u32) -> bool {
        let w = self.width;
        let h = self.height;

        // Тайлы коридора.
        let mut tiles: Vec<(u32, u32)> = Vec::with_capacity(len as usize);
        for k in 0..len {
            let nx = x0 as i64 + dx as i64 * k as i64;
            let ny = y0 as i64 + dy as i64 * k as i64;
            if nx < 2 || ny < 2 || nx >= w as i64 - 2 || ny >= h as i64 - 2 {
                return false;
            }
            tiles.push((nx as u32, ny as u32));
        }

        // Полоса пандуса перед началом коридора.
        let mut strip = [(0u32, 0u32); RAMP_LEN as usize];
        for k in 0..RAMP_LEN as usize {
            let step = k as i64 + 1;
            let nx = x0 as i64 - dx as i64 * step;
            let ny = y0 as i64 - dy as i64 * step;
            if nx < 2 || ny < 2 || nx >= w as i64 - 2 || ny >= h as i64 - 2 {
                return false;
            }
            strip[k] = (nx as u32, ny as u32);
        }

        // Коридор и пандус не должны занимать занятые тайлы верхнего яруса.
        for &(tx, ty) in tiles.iter().chain(strip.iter()) {
            if self.kind(tx, ty, LAYER_UPPER) != TileKind::Empty {
                return false;
            }
            // Спуск в ямку лежит на земле — его не затираем.
            if self.kind(tx, ty, LAYER_GROUND).is_ramp() {
                return false;
            }
        }
        // Под пандусом земля обязана быть проходима: по нему едет танк.
        for &(tx, ty) in strip.iter() {
            if self.blocks_tank(tx, ty, LAYER_GROUND) {
                return false;
            }
            // Пандус не кладём над ямкой: там дыра в земле.
            if self.is_pit(tx, ty) {
                return false;
            }
        }
        // Под коридором стен быть не должно (яма — можно: мост идёт над ней).
        for &(tx, ty) in tiles.iter() {
            if self.kind(tx, ty, LAYER_GROUND).blocks_tank() {
                return false;
            }
        }

        // Подход к пандусу с земли должен быть свободен: иначе на полосу
        // не въехать.
        let ax = x0 as i64 - dx as i64 * (RAMP_LEN as i64 + 1);
        let ay = y0 as i64 - dy as i64 * (RAMP_LEN as i64 + 1);
        if ax < 2 || ay < 2 || ax >= w as i64 - 2 || ay >= h as i64 - 2 {
            return false;
        }
        if self.blocks_tank(ax as u32, ay as u32, LAYER_GROUND) {
            return false;
        }

        // Коридор — проходимый настил верхнего яруса.
        for &(tx, ty) in tiles.iter() {
            self.set_kind(tx, ty, LAYER_UPPER, TileKind::Empty);
            self.set_deck(tx, ty, true);
        }
        // Пандус: полоса из трёх тайлов, подъём от земли к мостику.
        let dir = dir_from_delta(dx, dy);
        for (k, &(tx, ty)) in strip.iter().enumerate() {
            let value = ramp_value(dir, RAMP_LEN - 1 - k as u8, false);
            self.set_value(tx, ty, LAYER_GROUND, value);
            self.set_value(tx, ty, LAYER_UPPER, value);
        }
        true
    }

    /// Проходимы ли тайл и его соседи на обоих слоях.
    ///
    /// Такое условие нужно пандусу: к нему должно быть где подъехать, и он
    /// не должен упираться в стену. Проверка слой-зависимая: над ямкой
    /// проезжает только её собственный спуск, а на верхнем ярусе мешают
    /// только стены — пустота там это пропасть, по которой едут по полосе.
    fn ramp_area_free(&self, tx: u32, ty: u32) -> bool {
        for (dx, dy) in [(0i64, 0i64), (-1, 0), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (tx as i64 + dx, ty as i64 + dy);
            if nx < 0 || ny < 0 || nx >= self.width as i64 || ny >= self.height as i64 {
                return false;
            }
            let (nx, ny) = (nx as u32, ny as u32);
            if self.blocks_tank(nx, ny, LAYER_GROUND)
                || self.kind(nx, ny, LAYER_UPPER).blocks_tank()
            {
                return false;
            }
        }
        true
    }

    /// Пересчитывает идентификаторы лесных массивов (заливка по связности).
    fn rebuild_forest_regions(&mut self) {
        for level in 0..self.layers.len() {
            for v in self.forest_regions[level].iter_mut() {
                *v = u32::MAX;
            }
            let mut next_id = 0u32;
            let mut stack: Vec<(u32, u32)> = Vec::new();
            for y in 0..self.height {
                for x in 0..self.width {
                    if !self.kind(x, y, level as u8).hides_tank() {
                        continue;
                    }
                    if let Some(i) = self.index(x, y) {
                        if self.forest_regions[level][i] != u32::MAX {
                            continue;
                        }
                    }
                    let region = next_id;
                    next_id += 1;
                    stack.push((x, y));
                    while let Some((cx, cy)) = stack.pop() {
                        let idx = (cy * self.width + cx) as usize;
                        if self.forest_regions[level][idx] != u32::MAX {
                            continue;
                        }
                        self.forest_regions[level][idx] = region;
                        for (nx, ny) in [
                            (cx.wrapping_sub(1), cy),
                            (cx + 1, cy),
                            (cx, cy.wrapping_sub(1)),
                            (cx, cy + 1),
                        ] {
                            if nx >= self.width || ny >= self.height {
                                continue;
                            }
                            if self.kind(nx, ny, level as u8) == TileKind::Forest {
                                stack.push((nx, ny));
                            }
                        }
                    }
                }
            }
        }
    }

    /// Подбирает точки спавна, разнесённые по карте и свободные от препятствий.
    pub fn spawn_points(&self, count: usize, tank_size: f32) -> Vec<(f32, f32)> {
        let mut points: Vec<(f32, f32)> = Vec::with_capacity(count);
        let step_x = (self.width / 5).max(1);
        let step_y = (self.height / 5).max(1);
        let mut candidates: Vec<(u32, u32)> = Vec::new();
        for gy in 1..5 {
            for gx in 1..5 {
                candidates.push((gx * step_x, gy * step_y));
            }
        }
        for (tx, ty) in candidates {
            if points.len() >= count {
                break;
            }
            let x = tx as f32 * TILE_SIZE + (TILE_SIZE - tank_size) * 0.5;
            let y = ty as f32 * TILE_SIZE + (TILE_SIZE - tank_size) * 0.5;
            // Точка появления — только сплошная земля: над ямкой танк появился бы
            // на её дне, а на спускном пандусе — посреди спуска.
            if self.rect_is_free(x, y, tank_size, LAYER_GROUND)
                && self.is_solid_ground(x, y, tank_size)
            {
                points.push((x, y));
            }
        }
        // Если кандидатов не хватило — ищем любые свободные тайлы.
        if points.len() < count {
            let mut rng = Rng::new(0xC0FFEE ^ self.width as u64);
            let mut attempts = 0;
            while points.len() < count && attempts < 4096 {
                attempts += 1;
                let tx = rng.range(3, self.width.saturating_sub(4));
                let ty = rng.range(3, self.height.saturating_sub(4));
                let x = tx as f32 * TILE_SIZE + (TILE_SIZE - tank_size) * 0.5;
                let y = ty as f32 * TILE_SIZE + (TILE_SIZE - tank_size) * 0.5;
                if self.rect_is_free(x, y, tank_size, LAYER_GROUND)
                    && self.is_solid_ground(x, y, tank_size)
                {
                    points.push((x, y));
                }
            }
        }
        points
    }

    /// Проверяет, что прямоугольник не пересекается с блокирующими тайлами.
    pub fn rect_is_free(&self, x: f32, y: f32, size: f32, level: u8) -> bool {
        let x0 = (x / TILE_SIZE).floor() as i64;
        let y0 = (y / TILE_SIZE).floor() as i64;
        let x1 = ((x + size - 0.001) / TILE_SIZE).floor() as i64;
        let y1 = ((y + size - 0.001) / TILE_SIZE).floor() as i64;
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                if tx < 0 || ty < 0 {
                    return false;
                }
                let (tx, ty) = (tx as u32, ty as u32);
                if tx >= self.width || ty >= self.height {
                    return false;
                }
                if self.blocks_tank(tx, ty, level) {
                    return false;
                }
            }
        }
        true
    }

    /// Стоит ли прямоугольник на сплошной земле — без ямки под ним.
    ///
    /// На слое ямок `Empty` означает «ямы тут нет», то есть сплошную землю над
    /// головой. Любое другое значение под землёй — это дыра (дно ямки), стенка
    /// окопа или спускной пандус.
    ///
    /// Появляться над дырой нельзя: танк сразу окажется на дне ямки, а на
    /// спускном пандусе — ещё и на середине спуска.
    pub fn is_solid_ground(&self, x: f32, y: f32, size: f32) -> bool {
        let x0 = (x / TILE_SIZE).floor() as i64;
        let y0 = (y / TILE_SIZE).floor() as i64;
        let x1 = ((x + size - 0.001) / TILE_SIZE).floor() as i64;
        let y1 = ((y + size - 0.001) / TILE_SIZE).floor() as i64;
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                if tx < 0 || ty < 0 {
                    return false;
                }
                let (tx, ty) = (tx as u32, ty as u32);
                if tx >= self.width || ty >= self.height {
                    return false;
                }
                if self.kind(tx, ty, LAYER_PIT) != TileKind::Empty {
                    return false;
                }
            }
        }
        true
    }

    /// Свободна ли прямая между двумя точками для снаряда (бетон и кирпич мешают).
    pub fn bullet_line_of_sight(&self, x0: f32, y0: f32, x1: f32, y1: f32, level: u8) -> bool {
        let dx = x1 - x0;
        let dy = y1 - y0;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < 1.0 {
            return true;
        }
        let steps = (dist / (TILE_SIZE * 0.4)).ceil().max(1.0) as u32;
        for i in 1..steps {
            let t = i as f32 / steps as f32;
            let px = x0 + dx * t;
            let py = y0 + dy * t;
            let (tx, ty) = self.tile_at(px, py);
            if self.kind(tx, ty, level).blocks_bullet() {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Пространственное разбиение
// ---------------------------------------------------------------------------

/// Пространственная сетка: сущности раскладываются по чанкам.
///
/// Нужна для interest management: сервер отправляет игроку только сущности
/// из его чанка и восьми соседних.
#[derive(Debug, Clone)]
pub struct SpatialGrid {
    cells: Vec<Vec<u32>>,
}

impl Default for SpatialGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl SpatialGrid {
    /// Создаёт пустую сетку.
    pub fn new() -> SpatialGrid {
        SpatialGrid {
            cells: vec![Vec::new(); (CHUNKS_X * CHUNKS_Y) as usize],
        }
    }

    /// Очищает все чанки.
    pub fn clear(&mut self) {
        for cell in self.cells.iter_mut() {
            cell.clear();
        }
    }

    /// Кладёт сущность в чанк по мировым координатам.
    pub fn insert(&mut self, id: u32, x: f32, y: f32) {
        let cx = (x / (TILE_SIZE * CHUNK_TILES as f32)).floor().max(0.0) as u32;
        let cy = (y / (TILE_SIZE * CHUNK_TILES as f32)).floor().max(0.0) as u32;
        if cx >= CHUNKS_X || cy >= CHUNKS_Y {
            return;
        }
        let idx = (cy * CHUNKS_X + cx) as usize;
        self.cells[idx].push(id);
    }

    /// Сущности в чанке.
    pub fn chunk(&self, cx: u32, cy: u32) -> &[u32] {
        if cx >= CHUNKS_X || cy >= CHUNKS_Y {
            return &[];
        }
        &self.cells[(cy * CHUNKS_X + cx) as usize]
    }

    /// Сущности в квадрате чанков `[cx0..=cx1] x [cy0..=cy1]`.
    pub fn query(&self, cx0: u32, cy0: u32, cx1: u32, cy1: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let cy0 = cy0.max(0);
        let cx0 = cx0.max(0);
        let cy1 = cy1.min(CHUNKS_Y.saturating_sub(1));
        let cx1 = cx1.min(CHUNKS_X.saturating_sub(1));
        for cy in cy0..=cy1 {
            for cx in cx0..cx1 {
                out.extend_from_slice(self.chunk(cx, cy));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_generates_with_expected_size() {
        let map = GameMap::generate(MAP_W, MAP_H, 42);
        assert_eq!(map.len(), (MAP_W * MAP_H) as usize);
        assert_eq!(map.kind(0, 0, LAYER_GROUND), TileKind::Steel);
        assert_eq!(map.levels(), LEVELS);
    }

    #[test]
    fn rle_roundtrip() {
        let map = GameMap::generate(MAP_W, MAP_H, 7);
        for level in 0..LEVELS {
            let rle = map.rle_encode_level(level);
            let decoded = GameMap::rle_decode(&rle, MAP_W, MAP_H).expect("decode");
            assert_eq!(decoded.len(), map.len());
            for (i, v) in decoded.iter().enumerate() {
                assert_eq!(*v, map.layers[level as usize][i]);
            }
        }
    }

    #[test]
    fn rle_decode_rejects_bad_input() {
        assert!(GameMap::rle_decode(&[1, 0], 10, 10).is_none());
        assert!(GameMap::rle_decode(&[1, 0, 0, 2, 0, 0], 2, 2).is_none());
    }

    #[test]
    fn map_is_connected_from_center() {
        let map = GameMap::generate(MAP_W, MAP_H, 99);
        let (cx, cy) = (MAP_W / 2, MAP_H / 2);
        assert!(map.rect_is_free(cx as f32 * TILE_SIZE, cy as f32 * TILE_SIZE, 30.0, LAYER_GROUND));
        // Каждая пустая клетка достижима: проверяем, что «карманов» не осталось.
        let mut visited = vec![false; map.len()];
        let mut stack = vec![(cx, cy)];
        visited[(cy * MAP_W + cx) as usize] = true;
        while let Some((x, y)) = stack.pop() {
            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx >= MAP_W || ny >= MAP_H {
                    continue;
                }
                let idx = (ny * MAP_W + nx) as usize;
                if visited[idx] || map.blocks_tank(nx, ny, LAYER_GROUND) {
                    continue;
                }
                visited[idx] = true;
                stack.push((nx, ny));
            }
        }
        for y in 0..MAP_H {
            for x in 0..MAP_W {
                if !map.blocks_tank(x, y, LAYER_GROUND) {
                    assert!(visited[(y * MAP_W + x) as usize], "недостижимая клетка {x},{y}");
                }
            }
        }
    }

    #[test]
    fn spawn_points_are_free() {
        let map = GameMap::generate(MAP_W, MAP_H, 5);
        let points = map.spawn_points(25, 30.0);
        assert!(!points.is_empty());
        for (x, y) in points {
            assert!(map.rect_is_free(x, y, 30.0, LAYER_GROUND));
        }
    }

    /// Точки появления не должны лежать над ямкой.
    ///
    /// Ямка — это дыра в земле: появившийся над ней танк сразу оказался бы на
    /// дне (или посреди спуска по пандусу), а не на ровной земле.
    #[test]
    fn spawn_points_are_on_solid_ground() {
        for seed in [5u64, 11, 0x7A4C_1E55_D1CE_2026] {
            let map = GameMap::generate(MAP_W, MAP_H, seed);
            let points = map.spawn_points(25, 30.0);
            assert!(!points.is_empty());
            for (x, y) in points {
                assert!(
                    map.is_solid_ground(x, y, 30.0),
                    "точка появления ({x}, {y}) оказалась над ямкой"
                );
                assert!(map.rect_is_free(x, y, 30.0, LAYER_GROUND));
            }
        }
    }

    /// Карта не должна превращаться в кирпичную стену.
    ///
    /// Раньше ямка могла закрыть центр карты: заливка связности от центра
    /// обрывалась сразу, и весь мир замуровывался кирпичом — играть было
    /// негде. Теперь центр защищён от ямок, а связность ищется по всей карте.
    #[test]
    fn map_is_not_mostly_brick() {
        for seed in [5u64, 11, 42, 0x7A4C_1E55_D1CE_2026] {
            let map = GameMap::generate(MAP_W, MAP_H, seed);
            let total = (MAP_W * MAP_H) as usize;
            let mut free = 0usize;
            for y in 0..MAP_H {
                for x in 0..MAP_W {
                    if !map.blocks_tank(x, y, LAYER_GROUND) {
                        free += 1;
                    }
                }
            }
            let share = free * 100 / total;
            assert!(share >= 40, "сид {seed}: свободно только {share}% карты");
        }
    }

    /// Собирает все тайлы-пандусы нижнего яруса: координаты, направление
    /// полосы и номер тайла в ней.
    fn ramp_tiles(map: &GameMap) -> Vec<(u32, u32, u8, u8)> {
        let mut out = Vec::new();
        for y in 0..map.height() {
            for x in 0..map.width() {
                let value = map.value(x, y, LAYER_GROUND);
                if map.kind(x, y, LAYER_GROUND) == TileKind::Ramp {
                    out.push((x, y, ramp_dir(value), ramp_index(value)));
                }
            }
        }
        out
    }

    #[test]
    fn upper_level_has_ramps_on_both_levels() {
        let map = GameMap::generate(MAP_W, MAP_H, 11);
        let mut ramps = 0;
        for y in 0..MAP_H {
            for x in 0..MAP_W {
                if map.kind(x, y, LAYER_GROUND) != TileKind::Ramp {
                    continue;
                }
                ramps += 1;
                let value = map.value(x, y, LAYER_GROUND);
                // У каждого пандуса есть пара слоёв: низкая и высокая.
                // Подъёмный ведёт с земли наверх, спускной — с земли в ямку.
                let low = ramp_low_layer(value);
                let high = ramp_high_layer(value);
                assert_ne!(low, high, "у пандуса должны быть разные слои");
                // Пандус есть на обоих слоях пары: заехав на него, меняем слой.
                assert_eq!(map.kind(x, y, low), TileKind::Ramp);
                assert_eq!(map.kind(x, y, high), TileKind::Ramp);
                // Значение совпадает: тип, направление и номер в полосе.
                assert_eq!(
                    map.value(x, y, low),
                    map.value(x, y, high),
                    "пандус на тайле {x},{y} различается на слоях пары"
                );
            }
        }
        assert!(ramps > 0, "на карте должны быть переходы между слоями");
    }

    #[test]
    fn ramp_tile_value_packs_direction_and_index() {
        for dir in 0..4u8 {
            for idx in 0..RAMP_LEN {
                let value = ramp_value(dir, idx, false);
                // Базовый тип тайла не ломается: читаются только младшие биты.
                assert_eq!(TileKind::from_bits(value), TileKind::Ramp);
                assert_eq!(ramp_dir(value), dir);
                assert_eq!(ramp_index(value), idx);
            }
        }
        // Повреждения у пандуса нет: старшие биты заняты направлением.
        let mut map = GameMap::generate(64, 64, 3);
        map.set_value(5, 5, LAYER_GROUND, ramp_value(RAMP_DIR_UP, 2, false));
        assert_eq!(map.damage(5, 5, LAYER_GROUND), 0);
    }

    /// Есть ли тайл пандуса в позиции, смещённой от (x, y) на (dx, dy).
    fn has_ramp_at(
        ramps: &[(u32, u32, u8, u8)],
        x: i64,
        y: i64,
        dx: i64,
        dy: i64,
        dir: u8,
        idx: u8,
    ) -> bool {
        let (nx, ny) = (x + dx, y + dy);
        if nx < 0 || ny < 0 || nx > u32::MAX as i64 || ny > u32::MAX as i64 {
            return false;
        }
        ramps
            .iter()
            .any(|(rx, ry, rd, ri)| *rx as i64 == nx && *ry as i64 == ny && *rd == dir && *ri == idx)
    }

    #[test]
    fn ramp_strips_have_ordered_indexes() {
        let map = GameMap::generate(MAP_W, MAP_H, 11);
        let ramps = ramp_tiles(&map);
        assert!(!ramps.is_empty(), "на карте должны быть пандусы");
        for (x, y, dir, idx) in &ramps {
            if *idx != 1 {
                continue;
            }
            let (dx, dy) = ramp_dir_delta(*dir);
            // У середины полосы снизу — начало (номер 0), сверху — конец (2).
            assert!(
                has_ramp_at(&ramps, *x as i64, *y as i64, -dx as i64, -dy as i64, *dir, 0),
                "у пандуса должен быть нижний тайл"
            );
            assert!(
                has_ramp_at(&ramps, *x as i64, *y as i64, dx as i64, dy as i64, *dir, 2),
                "у пандуса должен быть верхний тайл"
            );
        }
    }

    #[test]
    fn ramp_strips_do_not_overlap() {
        for seed in [11u64, 42, 7] {
            let map = GameMap::generate(MAP_W, MAP_H, seed);
            let ramps = ramp_tiles(&map);
            let mut strips = 0;
            for (x, y, dir, idx) in &ramps {
                if *idx != 0 {
                    continue;
                }
                strips += 1;
                let (dx, dy) = ramp_dir_delta(*dir);
                assert!(
                    has_ramp_at(&ramps, *x as i64, *y as i64, dx as i64, dy as i64, *dir, 1),
                    "у полосы должен быть средний тайл"
                );
                assert!(
                    has_ramp_at(
                        &ramps,
                        *x as i64,
                        *y as i64,
                        2 * dx as i64,
                        2 * dy as i64,
                        *dir,
                        2
                    ),
                    "у полосы должен быть верхний тайл"
                );
            }
            assert!(
                (12..=44).contains(&strips),
                "полос должно быть 12..=44, а их {strips}"
            );
            // Полоса состоит ровно из трёх тайлов — значит, тайлы не делятся
            // между разными полосами.
            assert_eq!(ramps.len(), strips * RAMP_LEN as usize);
            // У каждого среднего тайла ровно один сосед-низ и ровно один
            // сосед-верх: полосы не пересекаются.
            for (x, y, dir, idx) in &ramps {
                if *idx != 1 {
                    continue;
                }
                let (dx, dy) = ramp_dir_delta(*dir);
                let low = ramps
                    .iter()
                    .filter(|(lx, ly, ld, li)| {
                        *lx as i64 == *x as i64 - dx as i64
                            && *ly as i64 == *y as i64 - dy as i64
                            && *ld == *dir
                            && *li == 0
                    })
                    .count();
                let high = ramps
                    .iter()
                    .filter(|(lx, ly, ld, li)| {
                        *lx as i64 == *x as i64 + dx as i64
                            && *ly as i64 == *y as i64 + dy as i64
                            && *ld == *dir
                            && *li == 2
                    })
                    .count();
                assert_eq!(low, 1, "у среднего тайла должен быть один нижний");
                assert_eq!(high, 1, "у среднего тайла должен быть один верхний");
            }
        }
    }

    #[test]
    fn ramp_strips_have_free_approach_on_both_levels() {
        for seed in [11u64, 42, 7] {
            let map = GameMap::generate(MAP_W, MAP_H, seed);
            for (x, y, dir, _) in ramp_tiles(&map) {
                let value = map.value(x, y, LAYER_GROUND);
                let low = ramp_low_layer(value);
                let high = ramp_high_layer(value);
                let (dx, dy) = ramp_dir_delta(dir);
                // Подъезд к полосе — с низкой стороны: танк едет по низкому
                // слою и въезжает на полосу с её края.
                let ax = x as i64 - dx as i64;
                let ay = y as i64 - dy as i64;
                assert!(
                    ax >= 0 && ay >= 0 && ax < MAP_W as i64 && ay < MAP_H as i64,
                    "пандус у края карты: {x},{y}"
                );
                assert!(
                    !map.blocks_tank(ax as u32, ay as u32, low),
                    "к пандусу {x},{y} нельзя подъехать с низкой стороны: \
                     тайл {ax},{ay} на слое {low} — {:?}, верх {high}, {:?}",
                    map.kind(ax as u32, ay as u32, low),
                    map.kind(ax as u32, ay as u32, high)
                );
                // Выход с полосы — на высокую сторону: там должен быть путь.
                let bx = x as i64 + dx as i64;
                let by = y as i64 + dy as i64;
                assert!(
                    bx >= 0 && by >= 0 && bx < MAP_W as i64 && by < MAP_H as i64,
                    "пандус у края карты: {x},{y}"
                );
                assert!(
                    !map.blocks_tank(bx as u32, by as u32, high),
                    "с пандуса {x},{y} нельзя уехать на высокую сторону: \
                     тайл {bx},{by} на слое {high} — {:?}, низкая сторона {low}",
                    map.kind(bx as u32, by as u32, high)
                );
            }
        }
    }

    #[test]
    fn spatial_grid_queries_neighbouring_chunks() {
        let mut grid = SpatialGrid::new();
        grid.insert(1, 100.0, 100.0);
        grid.insert(2, 5000.0, 5000.0);
        let found = grid.query(0, 0, 2, 2);
        assert!(found.contains(&1));
        assert!(!found.contains(&2));
    }

    #[test]
    fn tile_damage_bits_are_packed() {
        let mut map = GameMap::generate(64, 64, 1);
        map.set_value(5, 5, LAYER_GROUND, TileKind::Brick.bits() | (2 << 3));
        assert_eq!(map.kind(5, 5, LAYER_GROUND), TileKind::Brick);
        assert_eq!(map.damage(5, 5, LAYER_GROUND), 2);
    }

    #[test]
    fn pits_are_holes_with_a_descent_ramp() {
        let map = GameMap::generate(MAP_W, MAP_H, 11);
        assert!(map.pits_count() >= 3, "на карте должны быть ямки");
        for pit in &map.pits {
            let mut ramp_found = false;
            for ty in pit.y..pit.y + pit.h {
                for tx in pit.x..pit.x + pit.w {
                    let kind = map.kind(tx, ty, LAYER_GROUND);
                    if kind == TileKind::Ramp {
                        // Спуск в ямку: полоса ведёт с земли вниз.
                        assert!(ramp_is_pit(map.value(tx, ty, LAYER_GROUND)));
                        ramp_found = true;
                        continue;
                    }
                    assert_eq!(kind, TileKind::Empty, "в ямке на земле — дыра");
                    assert!(
                        map.blocks_tank(tx, ty, LAYER_GROUND),
                        "в ямку с земли уехать нельзя"
                    );
                    // Под землёй — стенки окопа по периметру и дно внутри.
                    let below = map.kind(tx, ty, LAYER_PIT);
                    let edge = tx == pit.x || tx == pit.x + pit.w - 1 || ty == pit.y || ty == pit.y + pit.h - 1;
                    if edge && below != TileKind::Ramp {
                        // Стенка окопа: по ней не ездят.
                        assert_eq!(below, TileKind::Brick, "по периметру ямки — стенка");
                        assert!(map.blocks_tank(tx, ty, LAYER_PIT));
                    } else {
                        // Дно ямки: по нему можно ездить.
                        assert!(
                            !map.blocks_tank(tx, ty, LAYER_PIT),
                            "по дну ямки ездить можно"
                        );
                    }                }
            }
            assert!(ramp_found, "у ямки должен быть спускной пандус");
        }
    }

    #[test]
    fn ground_stays_drivable_under_the_upper_level() {
        let map = GameMap::generate(MAP_W, MAP_H, 11);
        // Мостик и холм лежат на верхнем слое — земля под ними остаётся
        // проходимой: под мостиком можно проехать.
        let mut bridges = 0;
        for ty in 0..MAP_H {
            for tx in 0..MAP_W {
                if map.kind(tx, ty, LAYER_UPPER) == TileKind::Empty
                    && map.kind(tx, ty, LAYER_GROUND) == TileKind::Empty
                {
                    bridges += 1;
                }
            }
        }
        assert!(bridges > 0, "на верхнем ярусе должны быть мостики");
    }
}
