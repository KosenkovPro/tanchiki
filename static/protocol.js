/**
 * Бинарный протокол «Танчики» — клиентская часть.
 *
 * Точная копия формата из `src/protocol.rs`: все целые числа little-endian,
 * строки кодируются как `u16 len + utf8 bytes`.
 */

export const PROTOCOL_VERSION = 1;

/** Клиент -> сервер. */
export const MSG = Object.freeze({
  JOIN: 0x01,
  INPUT: 0x02,
  RESPAWN: 0x03,
  PING: 0x04,
});

/** Сервер -> клиент. */
export const SVR = Object.freeze({
  MAP: 0x10,
  WELCOME: 0x11,
  SNAPSHOT: 0x12,
  LEADERBOARD: 0x13,
  DEATH: 0x14,
  RESPAWNED: 0x15,
  PONG: 0x16,
  ERROR: 0x17,
});

/** Направления. */
export const DIR = Object.freeze({ UP: 0, DOWN: 1, LEFT: 2, RIGHT: 3 });

/** Биты ввода. */
export const BIT = Object.freeze({ UP: 1, DOWN: 2, LEFT: 4, RIGHT: 8, FIRE: 16 });

/** Типы тайлов. */
export const TILE = Object.freeze({
  EMPTY: 0,
  BRICK: 1,
  STEEL: 2,
  WATER: 3,
  FOREST: 4,
  ICE: 5,
});

/** Типы бонусов. */
export const PICKUP = Object.freeze({ STAR: 0, SHIELD: 1, REPAIR: 2 });

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
    let cap = this.buf.length * 2;
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

  /** Строка с префиксом-длиной `u16`, обрезанная до 16 байт. */
  str(v) {
    const encoded = new TextEncoder().encode(v);
    const len = Math.min(encoded.length, 16);
    this.u16(len);
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

export class Reader {
  constructor(bytes) {
    this.bytes = bytes;
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    this.pos = 0;
  }

  get remaining() {
    return this.bytes.length - this.pos;
  }

  need(n) {
    if (this.remaining < n) {
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

  str() {
    const len = this.u16();
    this.need(len);
    const s = new TextDecoder().decode(this.bytes.subarray(this.pos, this.pos + len));
    this.pos += len;
    return s;
  }
}

// ---------------------------------------------------------------------------
// Клиентские сообщения
// ---------------------------------------------------------------------------

/** `JOIN { nickname }` */
export function encodeJoin(nickname) {
  return new Writer().u8(MSG.JOIN).str(nickname).finish();
}

/** `INPUT { seq, flags }` */
export function encodeInput(seq, flags) {
  return new Writer().u8(MSG.INPUT).u32(seq).u8(flags).finish();
}

/** `RESPAWN` */
export function encodeRespawn() {
  return new Writer().u8(MSG.RESPAWN).finish();
}

/** `PING { client_time }` */
export function encodePing(clientTime) {
  return new Writer().u8(MSG.PING).u64(clientTime).finish();
}

// ---------------------------------------------------------------------------
// Серверные сообщения
// ---------------------------------------------------------------------------

function readMap(r) {
  const width = r.u32();
  const height = r.u32();
  const tileSize = r.u32();
  const len = r.u32();
  r.need(len);
  const rle = r.bytes.subarray(r.pos, r.pos + len);
  r.pos += len;
  return { width, height, tileSize, rle };
}

function readSnapshot(r) {
  const tick = r.u64();
  const focusX = r.f32();
  const focusY = r.f32();

  const players = [];
  const nPlayers = r.u16();
  for (let i = 0; i < nPlayers; i++) {
    const id = r.u32();
    const name = r.str();
    const flags = r.u8();
    const x = r.f32();
    const y = r.f32();
    const dir = r.u8();
    const level = r.u8();
    const hp = r.u8();
    const maxHp = r.u8();
    const score = r.u32();
    const kills = r.u32();
    players.push({
      id,
      name,
      isBot: (flags & 1) !== 0,
      alive: (flags & 2) !== 0,
      shielded: (flags & 4) !== 0,
      x,
      y,
      dir,
      level,
      hp,
      maxHp,
      score,
      kills,
    });
  }

  const bullets = [];
  const nBullets = r.u16();
  for (let i = 0; i < nBullets; i++) {
    bullets.push({
      id: r.u32(),
      x: r.f32(),
      y: r.f32(),
      dir: r.u8(),
      level: r.u8(),
    });
  }

  const pickups = [];
  const nPickups = r.u16();
  for (let i = 0; i < nPickups; i++) {
    pickups.push({
      id: r.u32(),
      x: r.f32(),
      y: r.f32(),
      kind: r.u8(),
      ttl: r.u32(),
    });
  }

  const tiles = [];
  const nTiles = r.u16();
  for (let i = 0; i < nTiles; i++) {
    tiles.push({ x: r.u16(), y: r.u16(), value: r.u8() });
  }

  const events = [];
  const nEvents = r.u16();
  for (let i = 0; i < nEvents; i++) {
    const kind = r.u8();
    switch (kind) {
      case 0:
        events.push({ kind: 'explosion', x: r.f32(), y: r.f32(), size: r.u8() });
        break;
      case 1:
        events.push({ kind: 'tileBroken', x: r.u16(), y: r.u16() });
        break;
      case 2:
        events.push({ kind: 'pickup', x: r.f32(), y: r.f32(), kind2: r.u8() });
        break;
      case 3:
        events.push({ kind: 'kill', killer: r.u32(), victim: r.u32() });
        break;
      case 4:
        events.push({ kind: 'levelUp', id: r.u32(), level: r.u8() });
        break;
      default:
        throw new ProtocolError('неизвестное событие: ' + kind);
    }
  }

  return { type: 'snapshot', tick, focusX, focusY, players, bullets, pickups, tiles, events };
}

function readLeaderboard(r) {
  const entries = [];
  const n = r.u16();
  for (let i = 0; i < n; i++) {
    entries.push({
      id: r.u32(),
      name: r.str(),
      score: r.u32(),
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
    case SVR.MAP: {
      const version = r.u16();
      return { type: 'map', version, map: readMap(r) };
    }
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
 * Разворачивает RLE-поток карты в плоский массив тайлов.
 * @param {Uint8Array} rle
 * @param {number} width
 * @param {number} height
 * @returns {Uint8Array}
 */
export function decodeRle(rle, width, height) {
  const total = width * height;
  const out = new Uint8Array(total);
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
