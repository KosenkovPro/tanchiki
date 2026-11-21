//! Карта мира: тайлы, ярусы, повреждаемость, генерация, RLE, чанки.
//!
//! Карта состоит из нескольких **ярусов** (уровней по высоте). Вид сверху,
//! но этажей несколько: нижний ярус — земля, верхний — площадки и перекрытия.
//! Переход между ярусами — тайл-«трамплин» (`TileKind::Ramp`), он есть
//! сразу на обоих ярусах: заехав на него, танк меняет этаж.

pub const TILE_SIZE: f32 = 32.0;

pub const MAP_W: u32 = 160;
pub const MAP_H: u32 = 160;

pub const CHUNK_TILES: u32 = 16;
pub const CHUNKS_X: u32 = MAP_W / CHUNK_TILES;
pub const CHUNKS_Y: u32 = MAP_H / CHUNK_TILES;

pub const VIEW_CHUNK_RADIUS: i32 = 3;

pub const BRICK_MAX_DAMAGE: u8 = 4;

/// Количество ярусов (этажей) карты.
pub const LEVELS: u8 = 2;

/// Нижний ярус — земля.
pub const LEVEL_GROUND: u8 = 0;
/// Верхний ярус — площадки и перекрытия.
pub const LEVEL_UPPER: u8 = 1;

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
    /// Трамплин — переход между ярусами.
    Ramp = 6,
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

    /// Является ли тайл переходом между ярусами.
    pub fn is_ramp(self) -> bool {
        matches!(self, TileKind::Ramp)
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

/// Тайловая карта мира с несколькими ярусами.
#[derive(Debug, Clone)]
pub struct GameMap {
    width: u32,
    height: u32,
    /// Упакованные значения тайлов: по одному массиву на ярус.
    layers: Vec<Vec<u8>>,
    /// Идентификатор лесного массива для каждого тайла (`u32::MAX` — не лес).
    forest_regions: Vec<Vec<u32>>,
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
        };
        map.build(seed);
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
    pub fn damage(&self, tx: u32, ty: u32, level: u8) -> u8 {
        self.value(tx, ty, level) >> 3
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

    /// Блокирует ли тайл движение танка.
    pub fn blocks_tank(&self, tx: u32, ty: u32, level: u8) -> bool {
        self.kind(tx, ty, level).blocks_tank()
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
                    self.set_kind(x, y, LEVEL_GROUND, TileKind::Steel);
                } else if near_edge {
                    self.set_kind(x, y, LEVEL_GROUND, TileKind::Brick);
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
                    self.set_kind(x, y, LEVEL_GROUND, TileKind::Empty);
                }
            }
        }

        // 4. Убираем «острова» пустоты, недоступные из центра, чтобы карта
        //    оставалась связной и игроки не застревали в закрытых карманах.
        self.ensure_connectivity(cx, cy);
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
                    self.set_kind(px, py, LEVEL_GROUND, kind);
                    self.set_kind(right, py, LEVEL_GROUND, kind);
                    self.set_kind(px, bottom, LEVEL_GROUND, kind);
                    self.set_kind(right, bottom, LEVEL_GROUND, kind);
                }
            }
        }
    }

    /// Гарантирует, что все пустые области связаны с центром карты.
    fn ensure_connectivity(&mut self, cx: u32, cy: u32) {
        let total = (self.width * self.height) as usize;
        let mut visited = vec![false; total];
        let mut queue: Vec<u32> = Vec::with_capacity(total);

        let start = (cy * self.width + cx) as usize;
        visited[start] = true;
        queue.push(start as u32);

        while let Some(idx) = queue.pop() {
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
                let nidx = (ny * self.width + nx) as usize;
                if visited[nidx] {
                    continue;
                }
                if self.kind(nx, ny, LEVEL_GROUND).blocks_tank() {
                    continue;
                }
                visited[nidx] = true;
                queue.push(nidx as u32);
            }
        }

        // Недостижимые пустые тайлы превращаем в кирпич — закрываем карманы.
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = (y * self.width + x) as usize;
                if !visited[idx] && !self.kind(x, y, LEVEL_GROUND).blocks_tank() {
                    self.set_kind(x, y, LEVEL_GROUND, TileKind::Brick);
                }
            }
        }
    }

    /// Строит верхний ярус: площадки-перекрытия и трамплины вниз.
    fn build_upper_level(&mut self, seed: u64) {
        let mut rng = Rng::new(seed);
        let w = self.width;
        let h = self.height;

        // 1. Площадки на верхнем ярусе. Ставим их над свободными местами
        //    нижнего яруса, чтобы они не «замуровывали» нижний этаж.
        let blocks = 14 + rng.below(8);
        for _ in 0..blocks {
            let bw = rng.range(3, 9);
            let bh = rng.range(3, 7);
            let bx = rng.range(3, w.saturating_sub(bw + 3));
            let by = rng.range(3, h.saturating_sub(bh + 3));
            let kind = if rng.chance(70) {
                TileKind::Brick
            } else {
                TileKind::Steel
            };
            for dy in 0..bh {
                for dx in 0..bw {
                    let px = bx + dx;
                    let py = by + dy;
                    if px < 2 || py < 2 || px >= w - 2 || py >= h - 2 {
                        continue;
                    }
                    // Под площадкой должно быть проходимо — иначе нижний
                    // ярус получит непроходимый «слепой» карман.
                    if self.kind(px, py, LEVEL_GROUND).blocks_tank() {
                        continue;
                    }
                    self.set_kind(px, py, LEVEL_UPPER, kind);
                }
            }
        }

        // 2. Трамплины: переходы между ярусами. Один и тот же тайл есть
        //    на обоих ярусах, поэтому заехав на него, танк меняет этаж.
        let ramps = 10 + rng.below(6);
        let mut placed = 0u32;
        let mut attempts = 0u32;
        while placed < ramps && attempts < 4096 {
            attempts += 1;
            let tx = rng.range(4, w.saturating_sub(5));
            let ty = rng.range(4, h.saturating_sub(5));
            // Место должно быть свободно на обоих ярусах.
            if self.kind(tx, ty, LEVEL_GROUND).blocks_tank()
                || self.kind(tx, ty, LEVEL_UPPER).blocks_tank()
            {
                continue;
            }
            // И вокруг должен быть проход, чтобы к трамплину можно было подъехать.
            let mut ok = true;
            for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                let nx = tx as i64 + dx;
                let ny = ty as i64 + dy;
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    ok = false;
                    break;
                }
                let (nx, ny) = (nx as u32, ny as u32);
                if self.kind(nx, ny, LEVEL_GROUND).blocks_tank()
                    || self.kind(nx, ny, LEVEL_UPPER).blocks_tank()
                {
                    ok = false;
                    break;
                }
            }
            if !ok {
                continue;
            }
            self.set_kind(tx, ty, LEVEL_GROUND, TileKind::Ramp);
            self.set_kind(tx, ty, LEVEL_UPPER, TileKind::Ramp);
            placed += 1;
        }
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
            if self.rect_is_free(x, y, tank_size, LEVEL_GROUND) {
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
                if self.rect_is_free(x, y, tank_size, LEVEL_GROUND) {
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
        assert_eq!(map.kind(0, 0, LEVEL_GROUND), TileKind::Steel);
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
        assert!(map.rect_is_free(cx as f32 * TILE_SIZE, cy as f32 * TILE_SIZE, 30.0, LEVEL_GROUND));
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
                if visited[idx] || map.blocks_tank(nx, ny, LEVEL_GROUND) {
                    continue;
                }
                visited[idx] = true;
                stack.push((nx, ny));
            }
        }
        for y in 0..MAP_H {
            for x in 0..MAP_W {
                if !map.blocks_tank(x, y, LEVEL_GROUND) {
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
            assert!(map.rect_is_free(x, y, 30.0, LEVEL_GROUND));
        }
    }

    #[test]
    fn upper_level_has_ramps_on_both_levels() {
        let map = GameMap::generate(MAP_W, MAP_H, 11);
        let mut ramps = 0;
        for y in 0..MAP_H {
            for x in 0..MAP_W {
                if map.kind(x, y, LEVEL_GROUND) == TileKind::Ramp {
                    ramps += 1;
                    // Трамплин есть на обоих ярусах: заехав на него, меняем этаж.
                    assert_eq!(map.kind(x, y, LEVEL_UPPER), TileKind::Ramp);
                }
            }
        }
        assert!(ramps > 0, "на карте должны быть переходы между ярусами");
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
        map.set_value(5, 5, LEVEL_GROUND, TileKind::Brick.bits() | (2 << 3));
        assert_eq!(map.kind(5, 5, LEVEL_GROUND), TileKind::Brick);
        assert_eq!(map.damage(5, 5, LEVEL_GROUND), 2);
    }
}
