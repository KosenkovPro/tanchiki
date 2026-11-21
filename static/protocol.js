/**
 * Бинарный протокол «Танчики» — клиентская часть.
 *
 * Точная копия формата из `src/protocol.rs` (версия 4): все целые числа
 * little-endian, строки кодируются как `u16 len + utf8 bytes`.
 *
 * Что изменилось в четвёртой версии:
 *  - башня смотрит в 64 направления (шаг 5.625°) вместо шестнадцати;
 *  - карта разделена на ярусы (этажи), у сущностей появился `deck`;
 *  - у способностей два отдельных кулдауна: миномёт и авиаподдержка;
 *  - звуки приходят отдельным каналом и слышны вне конуса обзора;
 *  - отдельного сообщения с картой (MAP) больше нет: карта приходит только
 *    в приветствии, и у неё сразу несколько ярусов.
 *
 * Декодирование никогда не паникует: битый пакет, неизвестный тип сообщения
 * или сломанная строка приводят к `ProtocolError`, а `decodeServerSafe`
 * возвращает в такой случай `null`.
 */

/** Версия протокола. Клиент и сервер обязаны иметь одинаковую. */
export const PROTOCOL_VERSION = 4;

/** Максимальная длина никнейма в байтах UTF-8. */
export const MAX_NICK_LEN = 16;

/** Клиент -> сервер. */
export const MSG = Object.freeze({
  JOIN: 0x01,
  INPUT: 0x02,
  RESPAWN: 0x03,
  PING: 0x04,
});

/** Сервер -> клиент. */
export const SVR = Object.freeze({
  /** Отдельной карты в версии 4 больше нет: карта приходит в `WELCOME`. */
  MAP: 0x10,
  WELCOME: 0x11,
  SNAPSHOT: 0x12,
  LEADERBOARD: 0x13,
  DEATH: 0x14,
  RESPAWNED: 0x15,
  PONG: 0x16,
  ERROR: 0x17,
});

/** Направления корпуса: четыре кардинальных и четыре диагональных. */
export const DIR = Object.freeze({
  UP: 0,
  DOWN: 1,
  LEFT: 2,
  RIGHT: 3,
  UP_LEFT: 4,
  UP_RIGHT: 5,
  DOWN_LEFT: 6,
  DOWN_RIGHT: 7,
});

/** Количество направлений наведения башни (шаг 5.625°). */
export const AIM_COUNT = 64;

/** Биты ввода. */
export const BIT = Object.freeze({ UP: 1, DOWN: 2, LEFT: 4, RIGHT: 8, FIRE: 16, USE: 32 });

/** Специальные способности. */
export const ABILITY = Object.freeze({ NONE: 0, MORTAR: 1, AIR_STRIKE: 2 });

/** Типы визуальных эффектов. */
export const EFFECT = Object.freeze({ MORTAR: 1, AIR_STRIKE: 2 });

/** Типы тайлов. */
export const TILE = Object.freeze({
  EMPTY: 0,
  BRICK: 1,
  STEEL: 2,
  WATER: 3,
  FOREST: 4,
  ICE: 5,
  /** Трамплин — переход между ярусами. */
  RAMP: 6,
});

/** Типы бонусов. */
export const PICKUP = Object.freeze({ STAR: 0, SHIELD: 1, REPAIR: 2 });

/** Биты флагов игрока в снапшоте. */
export const FLAG = Object.freeze({ BOT: 1, ALIVE: 2, SHIELDED: 4 });

/** Типы звуковых событий. */
export const SOUND = Object.freeze({ SHOT: 0, EXPLOSION: 1, HIT: 2 });

/** Типы событий мира (байт `kind` из `src/protocol.rs`). */
export const EVENT = Object.freeze({
  EXPLOSION: 0,
  TILE_BROKEN: 1,
  PICKUP_COLLECTED: 2,
  KILL: 3,
  LEVEL_UP: 4,
  ABILITY_USED: 5,
});

/** `f32::EPSILON` — порог, на котором вектор наведения считается нулевым. */
const F32_EPSILON = 1.1920929e-7;

/** Шаг наведения башни в радианах (5.625°). */
const AIM_STEP = (Math.PI * 2) / AIM_COUNT;

/**
 * Единичный вектор направления башни.
 * Угол 0 — вправо, дальше по часовой стрелке (ось Y вниз).
 * @param {number} aim
 * @returns {{x: number, y: number}}
 */
export function aimDelta(aim) {
  const a = aim * AIM_STEP;
  return { x: Math.cos(a), y: Math.sin(a) };
}

/**
 * Округление как в Rust: половинки — от нуля.
 * @param {number} v
 * @returns {number}
 */
function roundHalfAway(v) {
  return v < 0 ? -Math.round(-v) : Math.round(v);
}

/**
 * Направление башни по вектору: округление до ближайшего из 64.
 * @param {number} dx
 * @param {number} dy
 * @returns {number}
 */
export function aimFromVector(dx, dy) {
  if (Math.abs(dx) < F32_EPSILON && Math.abs(dy) < F32_EPSILON) return 0;
  const idx = roundHalfAway(Math.atan2(dy, dx) / AIM_STEP);
  return ((idx % AIM_COUNT) + AIM_COUNT) % AIM_COUNT;
}

/**
 * Направление башни от одной точки к другой.
 * @param {number} x0
 * @param {number} y0
 * @param {number} x1
 * @param {number} y1
 * @returns {number}
 */
export function aimTowards(x0, y0, x1, y1) {
  return aimFromVector(x1 - x0, y1 - y0);
}

/** Битый пакет — обычная ошибка, а не исключение наружу. */
export class ProtocolError extends Error {}

// ---------------------------------------------------------------------------
// Писатель
// ---------------------------------------------------------------------------

export class Writer {
  constructor(capacity = 256) {
    this.buf = new Uint8Array(capacity);
    this.view = new DataView(this.buf.buffer);
    this.pos = 0;
  }

  ensure(n) {
    if (this.pos + n <= this.buf.length) return;
    let cap = Math.max(256, this.buf.length * 2);
    while (cap < this.pos + n) cap *= 2;
    const next = new Uint8Array(cap);
    next.set(this.buf.subarray(0, this.pos));
    this.buf = next;
    this.view = new DataView(this.buf.buffer);
  }

  u8(v) {
    this.ensure(1);
    this.view.setUint8(this.pos, v & 0xff);
    this.pos += 1;
    return this;
  }

  u16(v) {
    this.ensure(2);
    this.view.setUint16(this.pos, v & 0xffff, true);
    this.pos += 2;
    return this;
  }

  u32(v) {
    this.ensure(4);
    this.view.setUint32(this.pos, v >>> 0, true);
    this.pos += 4;
    return this;
  }

  u64(v) {
    this.ensure(8);
    this.view.setBigUint64(this.pos, BigInt(v), true);
    this.pos += 8;
    return this;
  }

  f32(v) {
    this.ensure(4);
    this.view.setFloat32(this.pos, v, true);
    this.pos += 4;
    return this;
  }

  bytes(v) {
    this.ensure(v.length);
    this.buf.set(v, this.pos);
    this.pos += v.length;
    return this;
  }

  /**
   * Строка с префиксом-длиной `u16`, обрезанная до 16 байт.
   * Обрезка идёт по границе символа, чтобы не порвать UTF-8.
   */
  str(v) {
    const encoded = new TextEncoder().encode(v);
    let len = Math.min(encoded.length, MAX_NICK_LEN);
    // Отступаем назад, чтобы не разрезать многобайтовый символ.
    while (len > 0 && (encoded[len] & 0xc0) === 0x80) len -= 1;
    this.u16(len);
    this.ensure(len);
    this.buf.set(encoded.subarray(0, len), this.pos);
    this.pos += len;
    return this;
  }

  finish() {
    return this.buf.slice(0, this.pos);
  }
}

// ---------------------------------------------------------------------------
// Читатель
// ---------------------------------------------------------------------------

const UTF8_DECODER = new TextDecoder('utf-8', { fatal: true });

/**
 * Разбирает UTF-8 строго: битые байты — ошибка протокола, а не «кракозябры».
 * @param {Uint8Array} bytes
 * @returns {string}
 */
function decodeUtf8(bytes) {
  try {
    return UTF8_DECODER.decode(bytes);
  } catch {
    throw new ProtocolError('некорректный UTF-8');
  }
}

export class Reader {
  constructor(bytes) {
    if (bytes instanceof Uint8Array) {
      this.bytes = bytes;
    } else if (bytes instanceof ArrayBuffer) {
      this.bytes = new Uint8Array(bytes);
    } else if (ArrayBuffer.isView(bytes)) {
      this.bytes = new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    } else {
      throw new ProtocolError('ожидались байты сообщения');
    }
    this.view = new DataView(this.bytes.buffer, this.bytes.byteOffset, this.bytes.byteLength);
    this.pos = 0;
  }

  get remaining() {
    return this.view.byteLength - this.pos;
  }

  need(n) {
    if (!(n >= 0) || this.pos + n > this.view.byteLength) {
      throw new ProtocolError('сообщение обрезано');
    }
  }

  u8() {
    this.need(1);
    const v = this.view.getUint8(this.pos);
    this.pos += 1;
    return v;
  }

  u16() {
    this.need(2);
    const v = this.view.getUint16(this.pos, true);
    this.pos += 2;
    return v;
  }

  u32() {
    this.need(4);
    const v = this.view.getUint32(this.pos, true);
    this.pos += 4;
    return v;
  }

  u64() {
    this.need(8);
    const v = this.view.getBigUint64(this.pos, true);
    this.pos += 8;
    return Number(v);
  }

  f32() {
    this.need(4);
    const v = this.view.getFloat32(this.pos, true);
    this.pos += 4;
    return v;
  }

  /** Сырые байты без копирования. */
  take(n) {
    this.need(n);
    const out = this.bytes.subarray(this.pos, this.pos + n);
    this.pos += n;
    return out;
  }

  str() {
    const len = this.u16();
    const slice = this.take(len);
    return decodeUtf8(slice);
  }
}

// ---------------------------------------------------------------------------
// Клиентские сообщения
// ---------------------------------------------------------------------------

/**
 * Нормализует никнейм так же, как сервер: убирает управляющие символы,
 * обрезает по длине (в байтах UTF-8) и по границам символов.
 * @param {string} raw
 * @returns {string}
 */
export function sanitizeNickname(raw) {
  let cleaned = '';
  for (const ch of String(raw ?? '')) {
    const code = ch.codePointAt(0);
    const isControl = code < 0x20 || (code >= 0x7f && code <= 0x9f);
    if (!isControl) cleaned += ch;
  }
  cleaned = cleaned.trim();
  let out = '';
  let outLen = 0;
  for (const ch of cleaned) {
    const size = new TextEncoder().encode(ch).length;
    if (outLen + size > MAX_NICK_LEN) break;
    out += ch;
    outLen += size;
  }
  return out;
}

/** `JOIN { nickname }` */
export function encodeJoin(nickname) {
  return new Writer().u8(MSG.JOIN).str(sanitizeNickname(nickname)).finish();
}

/**
 * `INPUT { seq, flags, aim, ability, target_x, target_y }`
 *
 * `aim` — направление башни (0..63), `ability` — выбранная способность,
 * `target_x/target_y` — мировая точка, куда её применить.
 */
export function encodeInput(seq, flags, aim, ability, targetX, targetY) {
  return new Writer()
    .u8(MSG.INPUT)
    .u32(seq)
    .u8(flags)
    .u8(aim)
    .u8(ability)
    .f32(targetX)
    .f32(targetY)
    .finish();
}

/** `RESPAWN` */
export function encodeRespawn() {
  return new Writer().u8(MSG.RESPAWN).finish();
}

/** `PING { client_time }` */
export function encodePing(clientTime) {
  return new Writer().u8(MSG.PING).u64(clientTime).finish();
}

/**
 * Распаковывает байт ввода в набор булевых флагов.
 * @param {number} b
 * @returns {{up: boolean, down: boolean, left: boolean, right: boolean, fire: boolean, useAbility: boolean, v: number, h: number}}
 */
export function decodeInputFlags(b) {
  return {
    up: (b & BIT.UP) !== 0,
    down: (b & BIT.DOWN) !== 0,
    left: (b & BIT.LEFT) !== 0,
    right: (b & BIT.RIGHT) !== 0,
    fire: (b & BIT.FIRE) !== 0,
    useAbility: (b & BIT.USE) !== 0,
    // Знаки осей: вертикаль (-1 вверх, 1 вниз), горизонталь (-1 влево, 1 вправо).
    v: (b & BIT.UP) !== 0 && (b & BIT.DOWN) === 0 ? -1 : (b & BIT.DOWN) !== 0 && (b & BIT.UP) === 0 ? 1 : 0,
    h: (b & BIT.LEFT) !== 0 && (b & BIT.RIGHT) === 0 ? -1 : (b & BIT.RIGHT) !== 0 && (b & BIT.LEFT) === 0 ? 1 : 0,
  };
}

// ---------------------------------------------------------------------------
// Серверные сообщения
// ---------------------------------------------------------------------------

/**
 * Читает карту: размеры, размер тайла и RLE-потоки по ярусам.
 * @param {Reader} r
 */
function readMap(r) {
  const width = r.u32();
  const height = r.u32();
  const tileSize = r.u32();
  const layers = r.u8();
  const rle = [];
  for (let i = 0; i < layers; i++) {
    const len = r.u32();
    rle.push(r.take(len).slice());
  }
  return { width, height, tileSize, layers, rle };
}

/**
 * Читает снимок мира.
 * @param {Reader} r
 */
function readSnapshot(r) {
  const tick = r.u64();
  const focusX = r.f32();
  const focusY = r.f32();

  // --- Игроки ---
  const players = [];
  const nPlayers = r.u16();
  for (let i = 0; i < nPlayers; i++) {
    const id = r.u32();
    const name = r.str();
    const flags = r.u8();
    const x = r.f32();
    const y = r.f32();
    const dir = r.u8();
    const aim = r.u8();
    const deck = r.u8();
    const level = r.u8();
    const hp = r.u8();
    const maxHp = r.u8();
    const score = r.u32();
    const kills = r.u32();
    const ability = r.u8();
    const mortarCd = r.u16();
    const airstrikeCd = r.u16();
    players.push({
      id,
      name,
      flags,
      isBot: (flags & FLAG.BOT) !== 0,
      alive: (flags & FLAG.ALIVE) !== 0,
      shielded: (flags & FLAG.SHIELDED) !== 0,
      x,
      y,
      dir,
      aim,
      deck,
      level,
      hp,
      maxHp,
      score,
      kills,
      ability,
      mortarCd,
      airstrikeCd,
      // Совместимость с третьей версией: время до готовности любой способности.
      abilityCd: Math.min(mortarCd, airstrikeCd),
    });
  }

  // --- Снаряды ---
  const bullets = [];
  const nBullets = r.u16();
  for (let i = 0; i < nBullets; i++) {
    bullets.push({
      id: r.u32(),
      x: r.f32(),
      y: r.f32(),
      dir: r.u8(),
      deck: r.u8(),
      level: r.u8(),
    });
  }

  // --- Бонусы ---
  const pickups = [];
  const nPickups = r.u16();
  for (let i = 0; i < nPickups; i++) {
    pickups.push({
      id: r.u32(),
      x: r.f32(),
      y: r.f32(),
      kind: r.u8(),
      ttl: r.u32(),
      deck: r.u8(),
    });
  }

  // --- Изменения тайлов ---
  const tiles = [];
  const nTiles = r.u16();
  for (let i = 0; i < nTiles; i++) {
    tiles.push({
      x: r.u16(),
      y: r.u16(),
      value: r.u8(),
      deck: r.u8(),
    });
  }

  // --- Визуальные эффекты ---
  const effects = [];
  const nEffects = r.u16();
  for (let i = 0; i < nEffects; i++) {
    effects.push({
      id: r.u32(),
      kind: r.u8(),
      x: r.f32(),
      y: r.f32(),
      x2: r.f32(),
      y2: r.f32(),
      phase: r.u8(),
      deck: r.u8(),
    });
  }

  // --- Звуки ---
  const sounds = [];
  const nSounds = r.u16();
  for (let i = 0; i < nSounds; i++) {
    sounds.push({
      id: r.u32(),
      x: r.f32(),
      y: r.f32(),
      kind: r.u8(),
      owner: r.u32(),
      deck: r.u8(),
    });
  }

  // --- События мира ---
  const events = [];
  const nEvents = r.u16();
  for (let i = 0; i < nEvents; i++) {
    const kind = r.u8();
    switch (kind) {
      case EVENT.EXPLOSION:
        events.push({ kind: 'explosion', x: r.f32(), y: r.f32(), size: r.u8(), deck: r.u8() });
        break;
      case EVENT.TILE_BROKEN:
        events.push({ kind: 'tileBroken', x: r.u16(), y: r.u16(), deck: r.u8() });
        break;
      case EVENT.PICKUP_COLLECTED:
        events.push({ kind: 'pickup', x: r.f32(), y: r.f32(), kind2: r.u8() });
        break;
      case EVENT.KILL:
        events.push({ kind: 'kill', killer: r.u32(), victim: r.u32() });
        break;
      case EVENT.LEVEL_UP:
        events.push({ kind: 'levelUp', id: r.u32(), level: r.u8() });
        break;
      case EVENT.ABILITY_USED:
        events.push({ kind: 'abilityUsed', x: r.f32(), y: r.f32(), ability: r.u8() });
        break;
      default:
        throw new ProtocolError('неизвестное событие: ' + kind);
    }
  }

  return {
    type: 'snapshot',
    tick,
    focusX,
    focusY,
    players,
    bullets,
    pickups,
    tiles,
    effects,
    sounds,
    events,
  };
}

/**
 * Читает таблицу лидеров.
 * @param {Reader} r
 */
function readLeaderboard(r) {
  const entries = [];
  const n = r.u16();
  for (let i = 0; i < n; i++) {
    entries.push({
      id: r.u32(),
      name: r.str(),
      score: r.u32(),
      kills: r.u32(),
      isBot: r.u8() !== 0,
    });
  }
  return { type: 'leaderboard', entries };
}

/**
 * Разбирает серверное сообщение.
 * @param {Uint8Array} bytes
 * @returns {object}
 */
export function decodeServer(bytes) {
  const r = new Reader(bytes);
  const type = r.u8();
  switch (type) {
    case SVR.WELCOME: {
      const version = r.u16();
      const playerId = r.u32();
      const tick = r.u64();
      return { type: 'welcome', version, playerId, tick, map: readMap(r) };
    }
    case SVR.SNAPSHOT:
      return readSnapshot(r);
    case SVR.LEADERBOARD:
      return readLeaderboard(r);
    case SVR.DEATH:
      return {
        type: 'death',
        score: r.u32(),
        kills: r.u32(),
        deaths: r.u32(),
        survivedMs: r.u64(),
      };
    case SVR.RESPAWNED:
      return { type: 'respawned', tick: r.u64(), x: r.f32(), y: r.f32() };
    case SVR.PONG:
      return { type: 'pong', clientTime: r.u64(), serverTime: r.u64() };
    case SVR.ERROR:
      return { type: 'error', code: r.u16(), message: r.str() };
    default:
      throw new ProtocolError('неизвестный тип сообщения: ' + type);
  }
}

/**
 * То же, что `decodeServer`, но без исключений: битое сообщение даёт `null`.
 * @param {Uint8Array} bytes
 * @returns {object|null}
 */
export function decodeServerSafe(bytes) {
  try {
    return decodeServer(bytes);
  } catch {
    return null;
  }
}

/**
 * Разбирает пакет клиента (зеркало `decode_client` из `src/protocol.rs`).
 * @param {Uint8Array} bytes
 * @returns {object}
 */
export function decodeClient(bytes) {
  const r = new Reader(bytes);
  const type = r.u8();
  switch (type) {
    case MSG.JOIN:
      return { type: 'join', nickname: r.str() };
    case MSG.INPUT: {
      const seq = r.u32();
      const flags = r.u8();
      const aim = r.u8();
      const ability = r.u8();
      const targetX = r.f32();
      const targetY = r.f32();
      return {
        type: 'input',
        seq,
        flags,
        input: decodeInputFlags(flags),
        aim: aim % AIM_COUNT,
        ability,
        targetX,
        targetY,
      };
    }
    case MSG.RESPAWN:
      return { type: 'respawn' };
    case MSG.PING:
      return { type: 'ping', clientTime: r.u64() };
    default:
      throw new ProtocolError('неизвестный тип сообщения: ' + type);
  }
}

// ---------------------------------------------------------------------------
// Карта
// ---------------------------------------------------------------------------

/**
 * Разворачивает RLE-поток яруса в плоский массив тайлов.
 *
 * Поток — тройки `value: u8, run: u16` (little-endian), пока сумма `run`
 * не сравняется с `width * height`. Лишнее игнорируется, недостающее
 * остаётся нулями: декодер не бросает исключений.
 *
 * @param {Uint8Array|Uint8Array[]} rle поток яруса или массив потоков
 * @param {number} width
 * @param {number} height
 * @returns {Uint8Array|Uint8Array[]}
 */
export function decodeRle(rle, width, height) {
  if (Array.isArray(rle)) {
    return rle.map((layer) => decodeRle(layer, width, height));
  }
  const total = Math.max(0, width * height);
  const out = new Uint8Array(total);
  if (!rle) return out;
  let p = 0;
  let o = 0;
  while (p + 3 <= rle.length && o < total) {
    const value = rle[p];
    const run = rle[p + 1] | (rle[p + 2] << 8);
    p += 3;
    const end = Math.min(o + run, total);
    out.fill(value, o, end);
    o = end;
  }
  return out;
}

/**
 * Разворачивает все ярусы карты.
 * @param {Uint8Array[]} rleLayers
 * @param {number} width
 * @param {number} height
 * @returns {Uint8Array[]}
 */
export function decodeRleLayers(rleLayers, width, height) {
  if (!Array.isArray(rleLayers)) return [];
  return rleLayers.map((layer) => decodeRle(layer, width, height));
}
