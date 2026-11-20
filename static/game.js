/**
 * ТАНЧИКИ — клиент.
 *
 * Vanilla JS (ES-модули) + Canvas 2D.
 *
 * Что реализовано:
 *  - клиентское предсказание движения своего танка с мягкой коррекцией;
 *  - линейная интерполяция чужих танков и снарядов по буферу снапшотов;
 *  - interest management: клиент рисует только то, что прислал сервер;
 *  - HUD: мини-карта, ТОП-10, уровень/HP/пинг, экран приветствия и смерти.
 */

import {
  DIR,
  BIT,
  TILE,
  PICKUP,
  decodeServer,
  decodeRle,
  encodeJoin,
  encodeInput,
  encodeRespawn,
  encodePing,
} from './protocol.js';

// ---------------------------------------------------------------------------
// Константы (зеркалят src/game.rs)
// ---------------------------------------------------------------------------

const TANK_SIZE = 30;
const TILE_SIZE = 32;
const BASE_SPEED = 140;
const TICK_DT = 1 / 30;
const INTERP_DELAY = 100; // мс задержки для интерполяции чужих объектов
const SNAP_HISTORY = 24;

/** Угол поворота спрайта для каждого направления (радианы). */
const ROT = [
  0,
  Math.PI,
  -Math.PI / 2,
  Math.PI / 2,
  -Math.PI / 4,
  Math.PI / 4,
  (-3 * Math.PI) / 4,
  (3 * Math.PI) / 4,
];

/** Вектор направления для каждого направления (нормализованный). */
const S = Math.SQRT1_2;
const DIR_DX = [0, 0, -1, 1, -S, S, -S, S];
const DIR_DY = [-1, 1, 0, 0, -S, -S, S, S];

/** Перезарядка по уровню (в секундах) — зеркалит `level_fire_cooldown`. */
function fireCooldown(level) {
  return 1.65 - 0.3 * Math.max(0, level - 1);
}

// ---------------------------------------------------------------------------
// DOM
// ---------------------------------------------------------------------------

const canvas = document.getElementById('game');
const ctx = canvas.getContext('2d', { alpha: false });
const minimapCanvas = document.getElementById('minimap');
const mctx = minimapCanvas.getContext('2d');
const leaderboardEl = document.getElementById('leaderboard');
const connectionEl = document.getElementById('connection');
const overlayEl = document.getElementById('overlay');
const welcomeCard = document.getElementById('welcome-card');
const deathCard = document.getElementById('death-card');
const nicknameInput = document.getElementById('nickname');
const joinForm = document.getElementById('join-form');
const playButton = document.getElementById('play-button');
const respawnButton = document.getElementById('respawn-button');
const statLevel = document.getElementById('stat-level');
const statStars = document.getElementById('stat-stars');
const statHp = document.getElementById('stat-hp');
const statHpFill = document.getElementById('stat-hp-fill');
const statScore = document.getElementById('stat-score');
const statKills = document.getElementById('stat-kills');
const statPing = document.getElementById('stat-ping');
const statFps = document.getElementById('stat-fps');
const soundButton = document.getElementById('sound-toggle');

// ---------------------------------------------------------------------------
// Состояние
// ---------------------------------------------------------------------------

const state = {
  ws: null,
  connected: false,
  myId: 0,
  joined: false,
  dead: false,
  map: null,
  /** Последние известные состояния всех видимых игроков. */
  players: new Map(),
  /** Буфер снапшотов для интерполяции. */
  snapshots: [],
  leaderboard: [],
  ping: 0,
  pingSeen: false,
  score: 0,
  kills: 0,
  deaths: 0,
  focus: { x: 0, y: 0 },
  seq: 0,
};

/** Локально предсказанный танк игрока. */
const pred = {
  active: false,
  x: 0,
  y: 0,
  dir: DIR.DOWN,
  /** Направление движения: скорость направлена вдоль него. */
  moveDir: DIR.DOWN,
  vel: 0,
  level: 1,
  hp: 3,
  maxHp: 3,
  shielded: false,
  /** Остаток перезарядки — по нему клиент играет звук выстрела. */
  cooldown: 0,
};

/** Визуальное смещение, которое плавно гасится после серверной коррекции. */
const correction = { x: 0, y: 0 };

const camera = { x: 0, y: 0, zoom: 1 };

const keys = new Set();
let mouseDown = false;
let inputAccumulator = 0;

const particles = [];
let terrainCanvas = null;
let terrainCtx = null;
let sprites = null;

// ---------------------------------------------------------------------------
// Звук
// ---------------------------------------------------------------------------

/**
 * Простой звуковой движок на Web Audio API.
 *
 * Все файлы — CC0 (см. `static/sfx/CREDITS.txt`): можно использовать
 * в коммерческой игре без указания авторства и без оплаты.
 */
const audio = {
  ctx: null,
  master: null,
  buffers: new Map(),
  enabled: true,
  loading: false,
};

const SFX_NAMES = ['shot', 'shot_foe', 'explosion', 'hit', 'pickup', 'powerup', 'damage'];

/** Загружает и декодирует звуки. Вызывается по первому действию игрока. */
async function initAudio() {
  if (audio.ctx || audio.loading) return;
  audio.loading = true;
  const Ctx = window.AudioContext || window.webkitAudioContext;
  if (!Ctx) {
    audio.loading = false;
    return;
  }
  try {
    audio.ctx = new Ctx();
    audio.master = audio.ctx.createGain();
    audio.master.gain.value = 0.9;
    audio.master.connect(audio.ctx.destination);
    await Promise.all(
      SFX_NAMES.map(async (name) => {
        try {
          const res = await fetch(`/sfx/${name}.mp3`);
          if (!res.ok) return;
          const raw = await res.arrayBuffer();
          audio.buffers.set(name, await audio.ctx.decodeAudioData(raw));
        } catch (err) {
          console.warn('не удалось загрузить звук:', name, err);
        }
      }),
    );
  } catch (err) {
    console.warn('звук недоступен:', err);
  }
  audio.loading = false;
  updateSoundButton();
}

/** Проигрывает звук с заданной громкостью (0..1) и скоростью. */
function playSound(name, volume = 1, rate = 1) {
  if (!audio.enabled || !audio.ctx) return;
  const buf = audio.buffers.get(name);
  if (!buf) return;
  if (audio.ctx.state === 'suspended') audio.ctx.resume();
  const src = audio.ctx.createBufferSource();
  src.buffer = buf;
  src.playbackRate.value = rate;
  const gain = audio.ctx.createGain();
  gain.gain.value = volume;
  src.connect(gain);
  gain.connect(audio.master);
  src.start();
}

function toggleSound() {
  audio.enabled = !audio.enabled;
  if (audio.enabled && audio.ctx && audio.ctx.state === 'suspended') audio.ctx.resume();
  updateSoundButton();
}

function updateSoundButton() {
  if (!soundButton) return;
  soundButton.textContent = audio.enabled ? '🔊 ЗВУК' : '🔇 ЗВУК ВЫКЛ';
  soundButton.classList.toggle('off', !audio.enabled);
}

// ---------------------------------------------------------------------------
// Подключение
// ---------------------------------------------------------------------------

function connect() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/ws`);
  ws.binaryType = 'arraybuffer';
  state.ws = ws;

  ws.onopen = () => {
    state.connected = true;
    connectionEl.classList.add('hidden');
    playButton.disabled = false;
    respawnButton.disabled = false;
  };

  ws.onmessage = (ev) => {
    const bytes = new Uint8Array(ev.data);
    let msg;
    try {
      msg = decodeServer(bytes);
    } catch (err) {
      console.warn('битое сообщение от сервера:', err);
      return;
    }
    handleMessage(msg);
  };

  ws.onclose = () => {
    state.connected = false;
    state.joined = false;
    state.dead = false;
    pred.active = false;
    connectionEl.classList.remove('hidden');
    showWelcome();
    // Переподключаемся автоматически.
    setTimeout(connect, 2500);
  };

  ws.onerror = () => {
    /* onclose обработает переподключение */
  };
}

function send(bytes) {
  if (state.ws && state.ws.readyState === WebSocket.OPEN) {
    state.ws.send(bytes);
  }
}

function handleMessage(msg) {
  switch (msg.type) {
    case 'welcome':
      state.myId = msg.playerId;
      applyMap(msg.map);
      break;
    case 'map':
      applyMap(msg.map);
      break;
    case 'snapshot':
      applySnapshot(msg);
      break;
    case 'leaderboard':
      state.leaderboard = msg.entries;
      renderLeaderboard();
      break;
    case 'death':
      state.dead = true;
      pred.active = false;
      showDeath(msg);
      break;
    case 'respawned':
      state.dead = false;
      pred.active = true;
      pred.x = msg.x - TANK_SIZE / 2;
      pred.y = msg.y - TANK_SIZE / 2;
      pred.vel = 0;
      correction.x = 0;
      correction.y = 0;
      hideOverlay();
      break;
    case 'pong':
      state.ping = Math.max(0, Math.round(performance.now() - msg.clientTime));
      state.pingSeen = true;
      break;
    case 'error':
      console.warn('ошибка сервера:', msg.code, msg.message);
      break;
    default:
      break;
  }
}

function applyMap(map) {
  const tiles = decodeRle(map.rle, map.width, map.height);
  state.map = {
    width: map.width,
    height: map.height,
    tileSize: map.tileSize || TILE_SIZE,
    tiles,
  };
  buildTerrainCanvas();
  // Если карта сменилась — сбросим предсказание, чтобы не телепортировать танк.
  pred.active = false;
}

// ---------------------------------------------------------------------------
// Снапшоты
// ---------------------------------------------------------------------------

function applySnapshot(msg) {
  state.focus.x = msg.focusX;
  state.focus.y = msg.focusY;

  const players = new Map();
  for (const p of msg.players) {
    players.set(p.id, p);
  }
  const bullets = new Map();
  for (const b of msg.bullets) bullets.set(b.id, b);
  const pickups = new Map();
  for (const p of msg.pickups) pickups.set(p.id, p);

  state.snapshots.push({
    t: performance.now(),
    players,
    bullets,
    pickups,
  });
  if (state.snapshots.length > SNAP_HISTORY) state.snapshots.shift();

  // Выстрел противника рядом с нами — короткий щелчок.
  const prevSnap = state.snapshots.length >= 2 ? state.snapshots[state.snapshots.length - 2] : null;
  if (prevSnap && pred.active) {
    const pcx = pred.x + TANK_SIZE / 2;
    const pcy = pred.y + TANK_SIZE / 2;
    for (const b of msg.bullets) {
      if (prevSnap.bullets.has(b.id)) continue;
      if (Math.hypot(b.x - pcx, b.y - pcy) < 260) {
        playSound('shot_foe', 0.22, 0.92 + Math.random() * 0.16);
        break;
      }
    }
  }

  // Обновляем карту дельтами.
  if (msg.tiles.length && state.map) {
    for (const t of msg.tiles) {
      if (t.x < state.map.width && t.y < state.map.height) {
        state.map.tiles[t.y * state.map.width + t.x] = t.value;
        paintTerrainTile(t.x, t.y);
      }
    }
  }

  // События мира.
  for (const ev of msg.events) {
    if (ev.kind === 'explosion') {
      spawnExplosion(ev.x, ev.y, ev.size);
      // Громкость взрыва зависит от расстояния до камеры.
      const dist = Math.hypot(ev.x - camera.x, ev.y - camera.y);
      if (dist < 700) playSound('explosion', 0.6 * (1 - dist / 700));
    } else if (ev.kind === 'tileBroken') {
      spawnDebris(ev.x * state.map.tileSize + 16, ev.y * state.map.tileSize + 16);
      const dist = Math.hypot(
        ev.x * state.map.tileSize + 16 - camera.x,
        ev.y * state.map.tileSize + 16 - camera.y,
      );
      if (dist < 420) playSound('hit', 0.3 * (1 - dist / 420));
    } else if (ev.kind === 'pickup') {
      spawnSparkle(ev.x, ev.y);
      playSound('pickup', 0.55);
    } else if (ev.kind === 'levelUp') {
      if (ev.id === state.myId) playSound('powerup', 0.75);
    }
  }

  // Свой танк: сверяем предсказание с сервером.
  const mine = msg.players.find((p) => p.id === state.myId);
  if (mine) {
    state.score = mine.score;
    state.kills = mine.kills;
    pred.level = mine.level;
    const hpBefore = pred.hp;
    pred.hp = mine.hp;
    pred.maxHp = mine.maxHp;
    pred.shielded = mine.shielded;
    if (mine.hp < hpBefore) playSound('damage', 0.7);

    if (!pred.active) {
      // Первый раз сервер показал наш танк — синхронизируем предсказание.
      pred.active = true;
      pred.x = mine.x;
      pred.y = mine.y;
      pred.dir = mine.dir;
      pred.vel = 0;
      correction.x = 0;
      correction.y = 0;
    } else {
      const dx = mine.x - pred.x;
      const dy = mine.y - pred.y;
      const dist = Math.hypot(dx, dy);
      if (dist > 64) {
        // Расхождение слишком велико — жёсткая коррекция.
        pred.x = mine.x;
        pred.y = mine.y;
        correction.x = 0;
        correction.y = 0;
      } else {
        // Мягкая коррекция: смещение гасится постепенно.
        correction.x = dx;
        correction.y = dy;
      }
    }
    updateStatus();
  }

  state.players = players;
  renderMinimap();
}

// ---------------------------------------------------------------------------
// Ввод
// ---------------------------------------------------------------------------

function currentInput() {
  let bits = 0;
  if (keys.has('KeyW') || keys.has('ArrowUp')) bits |= BIT.UP;
  if (keys.has('KeyS') || keys.has('ArrowDown')) bits |= BIT.DOWN;
  if (keys.has('KeyA') || keys.has('ArrowLeft')) bits |= BIT.LEFT;
  if (keys.has('KeyD') || keys.has('ArrowRight')) bits |= BIT.RIGHT;
  if (keys.has('Space') || mouseDown) bits |= BIT.FIRE;
  return bits;
}

/**
 * Знаки осей ввода: вертикаль (-1 вверх, 1 вниз) и горизонталь
 * (-1 влево, 1 вправо). Нажатие двух осей сразу даёт диагональ.
 */
function inputAxes() {
  const v =
    (keys.has('KeyW') || keys.has('ArrowUp') ? -1 : 0) +
    (keys.has('KeyS') || keys.has('ArrowDown') ? 1 : 0);
  const h =
    (keys.has('KeyA') || keys.has('ArrowLeft') ? -1 : 0) +
    (keys.has('KeyD') || keys.has('ArrowRight') ? 1 : 0);
  return [v, h];
}

/** Направление по знакам осей — тот же алгоритм, что на сервере. */
function directionFromAxes(v, h) {
  if (v === 0 && h === 0) return null;
  if (v < 0 && h === 0) return DIR.UP;
  if (v > 0 && h === 0) return DIR.DOWN;
  if (v === 0 && h < 0) return DIR.LEFT;
  if (v === 0 && h > 0) return DIR.RIGHT;
  if (v < 0 && h < 0) return DIR.UP_LEFT;
  if (v < 0 && h > 0) return DIR.UP_RIGHT;
  if (v > 0 && h < 0) return DIR.DOWN_LEFT;
  return DIR.DOWN_RIGHT;
}

function isDiagonal(dir) {
  return dir >= DIR.UP_LEFT;
}

window.addEventListener('keydown', (e) => {
  if (e.code === 'Space' || e.code.startsWith('Arrow')) e.preventDefault();
  if (e.code === 'KeyM') {
    e.preventDefault();
    initAudio();
    toggleSound();
    return;
  }
  if (e.code === 'Enter' && !overlayEl.classList.contains('hidden')) {
    e.preventDefault();
    initAudio();
    if (state.joined) doRespawn();
    else doJoin();
    return;
  }
  keys.add(e.code);
});

window.addEventListener('keyup', (e) => keys.delete(e.code));
window.addEventListener('blur', () => keys.clear());

if (soundButton) {
  soundButton.addEventListener('click', () => {
    initAudio();
    toggleSound();
  });
}

canvas.addEventListener('mousedown', (e) => {
  if (e.button === 0) mouseDown = true;
});
window.addEventListener('mouseup', () => {
  mouseDown = false;
});
canvas.addEventListener('contextmenu', (e) => e.preventDefault());

canvas.addEventListener(
  'wheel',
  (e) => {
    e.preventDefault();
    camera.zoom = Math.max(0.55, Math.min(1.6, camera.zoom * (e.deltaY > 0 ? 0.9 : 1.1)));
  },
  { passive: false },
);

// ---------------------------------------------------------------------------
// Предсказание движения
// ---------------------------------------------------------------------------

function mapRectFree(x, y, size) {
  const m = state.map;
  if (!m) return true;
  const ts = m.tileSize;
  const x0 = Math.floor(x / ts);
  const y0 = Math.floor(y / ts);
  const x1 = Math.floor((x + size - 0.001) / ts);
  const y1 = Math.floor((y + size - 0.001) / ts);
  for (let ty = y0; ty <= y1; ty++) {
    for (let tx = x0; tx <= x1; tx++) {
      if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return false;
      const kind = m.tiles[ty * m.width + tx] & 7;
      if (kind === TILE.BRICK || kind === TILE.STEEL || kind === TILE.WATER) return false;
    }
  }
  return true;
}

function isOnIce(x, y) {
  const m = state.map;
  if (!m) return false;
  const tx = Math.floor((x + TANK_SIZE / 2) / m.tileSize);
  const ty = Math.floor((y + TANK_SIZE / 2) / m.tileSize);
  if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return false;
  return (m.tiles[ty * m.width + tx] & 7) === TILE.ICE;
}

function alignToGrid(dir) {
  const m = state.map;
  if (!m) return;
  const ts = m.tileSize;
  const vertical = dir === DIR.UP || dir === DIR.DOWN;
  const axisVal = vertical ? pred.x : pred.y;
  const base = Math.round(axisVal / ts);
  for (const delta of [0, -1, 1, -2, 2]) {
    const tile = base + delta;
    if (tile < 0) continue;
    const candidate = tile * ts;
    const x = vertical ? candidate : pred.x;
    const y = vertical ? pred.y : candidate;
    if (x >= 0 && y >= 0 && mapRectFree(x, y, TANK_SIZE)) {
      pred.x = x;
      pred.y = y;
      return;
    }
  }
}

function movePredicted(dist) {
  const dx = DIR_DX[pred.moveDir];
  const dy = DIR_DY[pred.moveDir];
  const steps = Math.max(1, Math.ceil(Math.abs(dist)));
  const step = dist / steps;
  for (let i = 0; i < steps; i++) {
    const nx = pred.x + dx * step;
    const ny = pred.y + dy * step;
    if (!mapRectFree(nx, ny, TANK_SIZE)) {
      pred.vel = 0;
      return;
    }
    pred.x = nx;
    pred.y = ny;
  }
}

function stepPrediction(dt) {
  if (!pred.active || state.dead) return;
  const input = currentInput();
  const [av, ah] = inputAxes();
  const desired = directionFromAxes(av, ah);
  const turned = desired !== null && desired !== pred.moveDir;

  const onIce = isOnIce(pred.x, pred.y);
  const speed = BASE_SPEED * (1 + 0.08 * (pred.level - 1));
  const accel = onIce ? 220 : 1100;
  const decel = onIce ? 120 : 1800;

  const target = desired !== null ? speed : 0;
  const baseVel = turned ? 0 : pred.vel;
  const delta = target - baseVel;
  const rate = delta > 0 ? accel : decel;
  const maxStep = rate * dt;
  let vel = baseVel + Math.max(-maxStep, Math.min(maxStep, delta));
  if (vel < 1) vel = 0;

  if (turned) {
    // Выравнивание по сетке — только между кардинальными направлениями.
    const wasCardinal = !isDiagonal(pred.moveDir);
    const nowCardinal = !isDiagonal(desired);
    pred.moveDir = desired;
    pred.vel = 0;
    if (nowCardinal && wasCardinal) alignToGrid(desired);
  } else if (desired !== null) {
    pred.moveDir = desired;
  }

  pred.dir = desired !== null ? desired : pred.dir;
  pred.vel = vel;

  const dist = vel * dt;
  if (dist > 0.001) movePredicted(dist);

  // Перезарядка: по ней клиент понимает, когда прозвучит выстрел.
  pred.cooldown = Math.max(0, pred.cooldown - dt);
  if ((input & BIT.FIRE) !== 0 && pred.cooldown <= 0) {
    playSound('shot', 0.5);
    pred.cooldown = fireCooldown(pred.level);
  }
}

// ---------------------------------------------------------------------------
// Оверлеи
// ---------------------------------------------------------------------------

function showWelcome() {
  welcomeCard.classList.remove('hidden');
  deathCard.classList.add('hidden');
  overlayEl.classList.remove('hidden');
  playButton.disabled = !state.connected;
  setTimeout(() => nicknameInput.focus(), 50);
}

function showDeath(msg) {
  deathCard.classList.remove('hidden');
  welcomeCard.classList.add('hidden');
  overlayEl.classList.remove('hidden');
  document.getElementById('death-score').textContent = msg.score;
  document.getElementById('death-kills').textContent = msg.kills;
  document.getElementById('death-deaths').textContent = msg.deaths;
  document.getElementById('death-time').textContent = `${Math.round(msg.survivedMs / 1000)} с`;
  respawnButton.disabled = !state.connected;
}

function hideOverlay() {
  overlayEl.classList.add('hidden');
}

function doJoin() {
  const name = nicknameInput.value.trim();
  if (!state.connected) return;
  initAudio();
  playButton.disabled = true;
  state.joined = true;
  localStorage.setItem('tanchiki.nick', name);
  send(encodeJoin(name));
  hideOverlay();
}

function doRespawn() {
  if (!state.connected) return;
  initAudio();
  respawnButton.disabled = true;
  send(encodeRespawn());
  hideOverlay();
}

joinForm.addEventListener('submit', (e) => {
  e.preventDefault();
  doJoin();
});
respawnButton.addEventListener('click', doRespawn);

// ---------------------------------------------------------------------------
// Спрайты тайлов
// ---------------------------------------------------------------------------

function buildSprites() {
  const TS = 32;
  const c = document.createElement('canvas');
  // 10 спрайтов в ряд: 4 стадии кирпича, бетон, лес, лёд и 3 кадра воды.
  c.width = TS * 10;
  c.height = TS;
  buildSpritesInto(c.getContext('2d'), TS);
  sprites = {
    steel: 4,
    forest: 5,
    ice: 6,
    water: [7, 8, 9],
    ts: TS,
    canvas: c,
  };
}

function spriteCell(index) {
  const ts = sprites.ts;
  return { sx: index * ts, sy: 0 };
}

/** Отдельный холст с «рельефом» карты для мини-карты. */
function buildTerrainCanvas() {
  const m = state.map;
  if (!m) return;
  terrainCanvas = document.createElement('canvas');
  terrainCanvas.width = m.width;
  terrainCanvas.height = m.height;
  const g = terrainCanvas.getContext('2d');
  terrainCtx = g;
  g.fillStyle = '#0a0e13';
  g.fillRect(0, 0, m.width, m.height);
  for (let ty = 0; ty < m.height; ty++) {
    for (let tx = 0; tx < m.width; tx++) {
      paintTerrainTileTo(g, tx, ty);
    }
  }
}

function paintTerrainTile(tx, ty) {
  if (!terrainCtx || !state.map) return;
  paintTerrainTileTo(terrainCtx, tx, ty);
}

function paintTerrainTileTo(g, tx, ty) {
  const m = state.map;
  const v = m.tiles[ty * m.width + tx];
  const kind = v & 7;
  let color = null;
  switch (kind) {
    case TILE.BRICK:
      color = '#7a3714';
      break;
    case TILE.STEEL:
      color = '#767f8c';
      break;
    case TILE.WATER:
      color = '#17518f';
      break;
    case TILE.FOREST:
      color = '#1d5c2a';
      break;
    case TILE.ICE:
      color = '#a8d8ef';
      break;
    default:
      color = '#0a0e13';
  }
  g.fillStyle = color;
  g.fillRect(tx, ty, 1, 1);
}

// ---------------------------------------------------------------------------
// Частицы
// ---------------------------------------------------------------------------

function spawnExplosion(x, y, size) {
  const n = 12 + size * 8;
  for (let i = 0; i < n; i++) {
    const angle = Math.random() * Math.PI * 2;
    const speed = 40 + Math.random() * 150 * (size / 3);
    particles.push({
      x,
      y,
      vx: Math.cos(angle) * speed,
      vy: Math.sin(angle) * speed,
      life: 0.35 + Math.random() * 0.45,
      max: 0.8,
      size: 2 + Math.random() * 3,
      color: Math.random() < 0.5 ? '#ffb703' : '#f85149',
      gravity: 90,
    });
  }
  for (let i = 0; i < 8; i++) {
    particles.push({
      x,
      y,
      vx: (Math.random() - 0.5) * 40,
      vy: (Math.random() - 0.5) * 40,
      life: 0.6,
      max: 0.6,
      size: 3 + Math.random() * 3,
      color: 'rgba(90,90,95,0.9)',
      gravity: 0,
    });
  }
}

function spawnDebris(x, y) {
  for (let i = 0; i < 6; i++) {
    particles.push({
      x,
      y,
      vx: (Math.random() - 0.5) * 90,
      vy: (Math.random() - 0.5) * 90 - 40,
      life: 0.45,
      max: 0.45,
      size: 2 + Math.random() * 2,
      color: '#a8541f',
      gravity: 260,
    });
  }
}

function spawnSparkle(x, y) {
  for (let i = 0; i < 10; i++) {
    const angle = (i / 10) * Math.PI * 2;
    particles.push({
      x,
      y,
      vx: Math.cos(angle) * 60,
      vy: Math.sin(angle) * 60,
      life: 0.35,
      max: 0.35,
      size: 2,
      color: '#ffd166',
      gravity: 0,
    });
  }
}

function updateParticles(dt) {
  for (let i = particles.length - 1; i >= 0; i--) {
    const p = particles[i];
    p.life -= dt;
    if (p.life <= 0) {
      particles.splice(i, 1);
      continue;
    }
    p.x += p.vx * dt;
    p.y += p.vy * dt;
    p.vy += p.gravity * dt;
    p.vx *= 0.98;
  }
}

// ---------------------------------------------------------------------------
// Отрисовка
// ---------------------------------------------------------------------------

function worldToScreenX(wx) {
  return (wx - camera.x) * camera.zoom + canvas.clientWidth / 2;
}

function worldToScreenY(wy) {
  return (wy - camera.y) * camera.zoom + canvas.clientHeight / 2;
}

function drawTiles(now) {
  const m = state.map;
  if (!m || !sprites) return;
  const ts = m.tileSize;
  const halfW = canvas.clientWidth / 2 / camera.zoom;
  const halfH = canvas.clientHeight / 2 / camera.zoom;

  const x0 = Math.max(0, Math.floor((camera.x - halfW) / ts));
  const x1 = Math.min(m.width - 1, Math.ceil((camera.x + halfW) / ts));
  const y0 = Math.max(0, Math.floor((camera.y - halfH) / ts));
  const y1 = Math.min(m.height - 1, Math.ceil((camera.y + halfH) / ts));

  const s = ts * camera.zoom;
  const waterFrame = Math.floor(now / 380) % 3;

  for (let ty = y0; ty <= y1; ty++) {
    for (let tx = x0; tx <= x1; tx++) {
      const v = m.tiles[ty * m.width + tx];
      const kind = v & 7;
      if (kind === TILE.EMPTY) continue;
      const damage = v >> 3;

      let index;
      if (kind === TILE.BRICK) index = Math.min(3, damage);
      else if (kind === TILE.STEEL) index = sprites.steel;
      else if (kind === TILE.FOREST) index = sprites.forest;
      else if (kind === TILE.ICE) index = sprites.ice;
      else if (kind === TILE.WATER) index = sprites.water[waterFrame];
      else continue;

      const { sx, sy } = spriteCell(index);
      ctx.drawImage(sprites.canvas, sx, sy, 32, 32, worldToScreenX(tx * ts), worldToScreenY(ty * ts), s, s);
    }
  }
}

function drawPickups(pickups) {
  for (const p of pickups.values()) {
    const x = worldToScreenX(p.x);
    const y = worldToScreenY(p.y);
    const r = 9 * camera.zoom;
    const pulse = 1 + Math.sin(performance.now() / 220 + p.id) * 0.12;

    ctx.save();
    ctx.translate(x, y);
    ctx.scale(pulse, pulse);
    if (p.kind === PICKUP.STAR) {
      drawStar(0, 0, r, '#ffd166', '#c98b00');
    } else if (p.kind === PICKUP.SHIELD) {
      ctx.beginPath();
      ctx.arc(0, 0, r * 0.85, 0, Math.PI * 2);
      ctx.fillStyle = 'rgba(88,166,255,0.35)';
      ctx.fill();
      ctx.lineWidth = 2.5 * camera.zoom;
      ctx.strokeStyle = '#58a6ff';
      ctx.stroke();
    } else {
      ctx.fillStyle = '#3fb950';
      ctx.fillRect(-r * 0.3, -r * 0.85, r * 0.6, r * 1.7);
      ctx.fillRect(-r * 0.85, -r * 0.3, r * 1.7, r * 0.6);
      ctx.strokeStyle = '#a6f0b8';
      ctx.lineWidth = 1.5;
      ctx.strokeRect(-r * 0.85, -r * 0.85, r * 1.7, r * 1.7);
    }
    ctx.restore();
  }
}

function drawStar(cx, cy, r, fill, stroke) {
  ctx.beginPath();
  for (let i = 0; i < 10; i++) {
    const rad = i % 2 === 0 ? r : r * 0.45;
    const a = (Math.PI / 5) * i - Math.PI / 2;
    const px = cx + Math.cos(a) * rad;
    const py = cy + Math.sin(a) * rad;
    if (i === 0) ctx.moveTo(px, py);
    else ctx.lineTo(px, py);
  }
  ctx.closePath();
  ctx.fillStyle = fill;
  ctx.fill();
  ctx.strokeStyle = stroke;
  ctx.lineWidth = 1.5;
  ctx.stroke();
}

function drawBullets(bullets) {
  for (const b of bullets.values()) {
    const x = worldToScreenX(b.x);
    const y = worldToScreenY(b.y);
    const dx = DIR_DX[b.dir] ?? 0;
    const dy = DIR_DY[b.dir] ?? 0;
    const size = 6 * camera.zoom;

    // След снаряда.
    ctx.strokeStyle = 'rgba(255,214,102,0.35)';
    ctx.lineWidth = size * 0.55;
    ctx.beginPath();
    ctx.moveTo(x - dx * size * 2.2, y - dy * size * 2.2);
    ctx.lineTo(x, y);
    ctx.stroke();

    ctx.fillStyle = b.level >= 3 ? '#fff3c4' : '#ffd166';
    ctx.fillRect(x - size / 2, y - size / 2, size, size);
    ctx.fillStyle = '#ffffff';
    ctx.fillRect(x - size / 4, y - size / 4, size / 2, size / 2);
  }
}

/** Находится ли точка внутри леса (тайлы кустов). */
function isInForest(x, y) {
  const m = state.map;
  if (!m) return false;
  const tx = Math.floor((x + TANK_SIZE / 2) / m.tileSize);
  const ty = Math.floor((y + TANK_SIZE / 2) / m.tileSize);
  if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return false;
  return (m.tiles[ty * m.width + tx] & 7) === TILE.FOREST;
}

function drawTank(p, opts = {}) {
  const size = TANK_SIZE * camera.zoom;
  const x = worldToScreenX(p.x);
  const y = worldToScreenY(p.y);
  const hue = opts.hue ?? 40;
  const level = p.level ?? 1;

  ctx.save();
  // В чаще танк частично маскируется — становится полупрозрачным.
  ctx.globalAlpha = isInForest(p.x, p.y) ? (opts.self ? 0.62 : 0.45) : 1;
  ctx.translate(x + size / 2, y + size / 2);
  ctx.rotate(ROT[p.dir] ?? 0);

  const half = size / 2;
  const trackW = size * 0.22;

  // Гусеницы.
  ctx.fillStyle = '#16191d';
  ctx.fillRect(-half, -half, trackW, size);
  ctx.fillRect(half - trackW, -half, trackW, size);
  ctx.fillStyle = '#39414d';
  const tread = ((opts.trackPhase ?? 0) * 4) % 6;
  for (let i = -half + tread; i < half; i += 6 * camera.zoom) {
    ctx.fillRect(-half, i, trackW, 2 * camera.zoom);
    ctx.fillRect(half - trackW, i + 3 * camera.zoom, trackW, 2 * camera.zoom);
  }

  // Корпус.
  ctx.fillStyle = `hsl(${hue}, 58%, 40%)`;
  ctx.fillRect(-half + trackW, -half + size * 0.14, size - trackW * 2, size * 0.82);
  ctx.fillStyle = `hsl(${hue}, 62%, 52%)`;
  ctx.fillRect(-half + trackW, -half + size * 0.14, size - trackW * 2, size * 0.16);

  // Стволы: на втором уровне появляется вторая пушка.
  // Обе симметричны относительно продольной оси танка.
  ctx.fillStyle = `hsl(${hue}, 45%, 26%)`;
  if (level >= 2) {
    ctx.fillRect(-size * 0.313, -half, size * 0.16, size * 0.34);
    ctx.fillRect(size * 0.153, -half, size * 0.16, size * 0.34);
  } else {
    ctx.fillRect(-size * 0.1, -half, size * 0.2, size * 0.34);
  }

  // Башня.
  ctx.fillStyle = `hsl(${hue}, 66%, 56%)`;
  ctx.fillRect(-size * 0.18, -half + size * 0.24, size * 0.36, size * 0.34);

  ctx.restore();
  ctx.globalAlpha = 1;

  // Уровень прокачки — звёздочки на башне.
  if (p.level > 1) {
    for (let i = 0; i < p.level - 1; i++) {
      const px = x + size / 2 - (p.level - 2) * 4 * camera.zoom + i * 8 * camera.zoom;
      const py = y - size * 0.12;
      drawStar(px, py, 3.4 * camera.zoom, '#ffd166', '#8a5b00');
    }
  }

  // Щит.
  if (p.shielded) {
    ctx.beginPath();
    ctx.arc(x + size / 2, y + size / 2, size * 0.72, 0, Math.PI * 2);
    ctx.strokeStyle = 'rgba(88,166,255,0.85)';
    ctx.lineWidth = 2.5 * camera.zoom;
    ctx.stroke();
    ctx.fillStyle = 'rgba(88,166,255,0.14)';
    ctx.fill();
  }

  // Полоска здоровья.
  if (!opts.self && p.hp < p.maxHp) {
    const bw = size;
    const bx = x;
    const by = y - 8 * camera.zoom;
    ctx.fillStyle = 'rgba(0,0,0,0.6)';
    ctx.fillRect(bx, by, bw, 3.5 * camera.zoom);
    ctx.fillStyle = p.hp > p.maxHp / 2 ? '#3fb950' : '#f85149';
    ctx.fillRect(bx, by, (bw * p.hp) / p.maxHp, 3.5 * camera.zoom);
  }

  // Ник.
  if (!opts.self && p.name) {
    ctx.font = `${Math.max(9, 11 * camera.zoom)}px Consolas, monospace`;
    ctx.textAlign = 'center';
    ctx.fillStyle = 'rgba(0,0,0,0.75)';
    ctx.fillText(p.name, x + size / 2 + 1, y - 11 * camera.zoom + 1);
    ctx.fillStyle = p.isBot ? '#f0a202' : '#e6edf3';
    ctx.fillText(p.name, x + size / 2, y - 11 * camera.zoom);
  }
}

function drawParticles() {
  for (const p of particles) {
    const alpha = Math.max(0, p.life / p.max);
    ctx.globalAlpha = alpha;
    ctx.fillStyle = p.color;
    const s = p.size * camera.zoom;
    ctx.fillRect(worldToScreenX(p.x) - s / 2, worldToScreenY(p.y) - s / 2, s, s);
  }
  ctx.globalAlpha = 1;
}

/** Возвращает два снапшота, между которыми нужно интерполировать. */
function interpolationPair(renderTime) {
  const s = state.snapshots;
  if (s.length === 0) return null;
  if (s.length === 1) return { a: s[0], b: s[0], f: 0 };
  for (let i = s.length - 1; i > 0; i--) {
    const a = s[i - 1];
    const b = s[i];
    if (renderTime >= a.t && renderTime <= b.t) {
      const f = (renderTime - a.t) / Math.max(1, b.t - a.t);
      return { a, b, f };
    }
  }
  return { a: s[s.length - 2], b: s[s.length - 1], f: 1 };
}

function renderWorld(now) {
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;

  ctx.fillStyle = '#0a0e13';
  ctx.fillRect(0, 0, w, h);

  drawTiles(now);

  const renderTime = performance.now() - INTERP_DELAY;
  const pair = interpolationPair(renderTime);

  if (pair) {
    const { a, b, f } = pair;

    // --- Бонусы ---
    drawPickups(b.pickups);

    // --- Снаряды ---
    drawBullets(b.bullets);

    // --- Танки ---
    for (const [id, pb] of b.players) {
      if (id === state.myId) continue;
      const pa = a.players.get(id);
      const p = {
        ...pb,
        x: pa ? pa.x + (pb.x - pa.x) * f : pb.x,
        y: pa ? pa.y + (pb.y - pa.y) * f : pb.y,
      };
      drawTank(p, { hue: hueFor(id), trackPhase: (performance.now() / 60) % 6 });
    }
  }

  // --- Свой танк (предсказание + мягкая коррекция) ---
  if (pred.active) {
    drawTank(
      {
        x: pred.x + correction.x,
        y: pred.y + correction.y,
        dir: pred.dir,
        level: pred.level,
        hp: pred.hp,
        maxHp: pred.maxHp,
        shielded: pred.shielded,
        name: '',
      },
      { hue: hueFor(state.myId), self: true, trackPhase: (performance.now() / 60) % 6 },
    );
  }

  drawParticles();
}

/** Стабильный цвет танка по id. */
function hueFor(id) {
  const palette = [40, 0, 150, 200, 280, 330, 95, 15, 175, 255, 60, 310];
  return palette[Math.abs(id) % palette.length];
}

// ---------------------------------------------------------------------------
// HUD
// ---------------------------------------------------------------------------

function updateStatus() {
  statLevel.textContent = String(pred.level);
  statStars.textContent = '★'.repeat(pred.level);
  statHp.textContent = `${pred.hp}/${pred.maxHp}`;
  statHpFill.style.width = `${Math.max(0, (pred.hp / Math.max(1, pred.maxHp)) * 100)}%`;
  statHpFill.classList.toggle('low', pred.hp <= pred.maxHp / 2);
  statScore.textContent = String(state.score);
  statKills.textContent = String(state.kills);
  statPing.textContent = state.pingSeen ? `${state.ping} мс` : '—';
}

function renderLeaderboard() {
  const entries = state.leaderboard;
  leaderboardEl.innerHTML = '';
  for (const e of entries) {
    const li = document.createElement('li');
    if (e.id === state.myId) li.classList.add('me');
    const who = document.createElement('span');
    who.className = e.isBot ? 'who bot' : 'who';
    who.textContent = e.name || '—';
    // Главное число — число уничтоженных танков.
    const kills = document.createElement('span');
    kills.className = 'kills';
    kills.textContent = String(e.kills ?? 0);
    const pts = document.createElement('span');
    pts.className = 'pts';
    pts.textContent = `${e.score}`;
    li.append(who, kills, pts);
    leaderboardEl.append(li);
  }
}

function renderMinimap() {
  if (!terrainCanvas || !state.map) return;
  const m = state.map;
  mctx.clearRect(0, 0, minimapCanvas.width, minimapCanvas.height);
  mctx.drawImage(terrainCanvas, 0, 0);

  // Бонусы.
  const snap = state.snapshots[state.snapshots.length - 1];
  if (snap) {
    mctx.fillStyle = '#58a6ff';
    for (const p of snap.pickups.values()) {
      mctx.fillRect((p.x / m.tileSize) - 0.5, (p.y / m.tileSize) - 0.5, 2, 2);
    }
    // Танки.
    for (const p of snap.players.values()) {
      const cx = p.x / m.tileSize;
      const cy = p.y / m.tileSize;
      if (p.id === state.myId) {
        mctx.fillStyle = '#58d68d';
        mctx.fillRect(cx - 1.5, cy - 1.5, 3, 3);
        mctx.strokeStyle = 'rgba(88,214,141,0.6)';
        mctx.lineWidth = 1;
        mctx.beginPath();
        mctx.arc(cx, cy, 6, 0, Math.PI * 2);
        mctx.stroke();
      } else {
        mctx.fillStyle = p.isBot ? '#f0a202' : '#f85149';
        mctx.fillRect(cx - 1, cy - 1, 2, 2);
      }
    }
  }
}

// ---------------------------------------------------------------------------
// Главный цикл
// ---------------------------------------------------------------------------

let lastFrame = performance.now();
let fpsAccum = 0;
let fpsFrames = 0;

function frame(now) {
  const dt = Math.min(0.1, (now - lastFrame) / 1000);
  lastFrame = now;

  // Предсказание идёт фиксированным шагом 30 Гц — как на сервере.
  inputAccumulator += dt;
  while (inputAccumulator >= TICK_DT) {
    stepPrediction(TICK_DT);
    inputAccumulator -= TICK_DT;
  }

  // Камера.
  let targetX = state.focus.x;
  let targetY = state.focus.y;
  if (pred.active) {
    targetX = pred.x + correction.x + TANK_SIZE / 2;
    targetY = pred.y + correction.y + TANK_SIZE / 2;
  }
  const follow = Math.min(1, dt * 9);
  camera.x += (targetX - camera.x) * follow;
  camera.y += (targetY - camera.y) * follow;

  // Коррекция плавно гасится.
  correction.x *= Math.pow(0.0025, dt);
  correction.y *= Math.pow(0.0025, dt);

  updateParticles(dt);
  renderWorld(now);

  fpsAccum += dt;
  fpsFrames += 1;
  if (fpsAccum >= 0.5) {
    statFps.textContent = String(Math.round(fpsFrames / fpsAccum));
    fpsAccum = 0;
    fpsFrames = 0;
  }

  requestAnimationFrame(frame);
}

function resizeCanvas() {
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  canvas.width = Math.floor(canvas.clientWidth * dpr);
  canvas.height = Math.floor(canvas.clientHeight * dpr);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.imageSmoothingEnabled = false;
}

// ---------------------------------------------------------------------------
// Запуск
// ---------------------------------------------------------------------------

function boot() {
  const saved = localStorage.getItem('tanchiki.nick');
  if (saved) nicknameInput.value = saved;

  // Спрайты тайлов рисуем один раз в offscreen-холст.
  buildSprites();

  resizeCanvas();
  window.addEventListener('resize', resizeCanvas);

  showWelcome();
  connect();

  // Ввод отправляем с той же частотой, что и серверные тики.
  setInterval(() => {
    if (!state.connected) return;
    state.seq += 1;
    send(encodeInput(state.seq, currentInput()));
  }, 1000 / 30);

  // Пинг для измерения RTT.
  setInterval(() => {
    if (!state.connected) return;
    send(encodePing(Math.round(performance.now())));
  }, 2000);

  requestAnimationFrame(frame);
}

/** Спрайты тайлов (см. buildSprites). */
function buildSpritesInto(g, TS) {
  const c = g.canvas;
  // Кирпич: 4 стадии.
  for (let d = 0; d < 4; d++) {
    const x = d * TS;
    g.fillStyle = '#9c4a1c';
    g.fillRect(x, 0, TS, TS);
    g.fillStyle = '#b8591f';
    g.fillRect(x + 1, 1, TS - 2, TS - 2);
    g.strokeStyle = '#7a3714';
    g.lineWidth = 1;
    for (let row = 0; row < 4; row++) {
      const y = row * 8 + 0.5;
      g.beginPath();
      g.moveTo(x, y);
      g.lineTo(x + TS, y);
      g.stroke();
      const offset = row % 2 === 0 ? 0 : 8;
      for (let col = 0; col < 2; col++) {
        const vx = x + offset + col * 16 + 0.5;
        if (vx > x && vx < x + TS) {
          g.beginPath();
          g.moveTo(vx, y);
          g.lineTo(vx, y + 8);
          g.stroke();
        }
      }
    }
    if (d > 0) {
      g.strokeStyle = 'rgba(30,15,8,0.85)';
      g.lineWidth = d;
      g.beginPath();
      g.moveTo(x + 6, 2);
      g.lineTo(x + 13, 14);
      g.lineTo(x + 8, 24);
      g.stroke();
    }
    if (d > 1) {
      g.beginPath();
      g.moveTo(x + 26, 4);
      g.lineTo(x + 19, 16);
      g.lineTo(x + 25, 30);
      g.stroke();
    }
    if (d > 2) {
      g.fillStyle = 'rgba(20,10,5,0.55)';
      g.fillRect(x + 4, 6, 9, 7);
      g.fillRect(x + 19, 18, 8, 9);
    }
    g.strokeStyle = 'rgba(0,0,0,0.45)';
    g.lineWidth = 1;
    g.strokeRect(x + 0.5, 0.5, TS - 1, TS - 1);
  }

  // Бетон.
  {
    const x = 4 * TS;
    g.fillStyle = '#767f8c';
    g.fillRect(x, 0, TS, TS);
    g.fillStyle = '#aeb8c6';
    g.fillRect(x + 1, 1, TS - 2, 7);
    g.fillStyle = '#5d6673';
    g.fillRect(x + 1, TS - 8, TS - 2, 7);
    g.fillStyle = '#8d97a5';
    g.fillRect(x + 2, 9, TS - 4, TS - 18);
    g.fillStyle = '#c9d3df';
    g.fillRect(x + 4, 4, 5, 4);
    g.fillRect(x + TS - 9, 4, 5, 4);
    g.strokeStyle = '#454d58';
    g.lineWidth = 1;
    g.strokeRect(x + 0.5, 0.5, TS - 1, TS - 1);
  }

  // Лес.
  {
    const x = 5 * TS;
    const blobs = [
      [8, 10, 8],
      [22, 9, 9],
      [15, 20, 10],
      [6, 22, 7],
      [26, 21, 8],
    ];
    for (const [bx, by, r] of blobs) {
      g.fillStyle = '#1d5c2a';
      g.beginPath();
      g.arc(x + bx, by, r, 0, Math.PI * 2);
      g.fill();
    }
    for (const [bx, by, r] of blobs) {
      g.fillStyle = '#2b7d3b';
      g.beginPath();
      g.arc(x + bx - 1.5, by - 2, r * 0.62, 0, Math.PI * 2);
      g.fill();
    }
  }

  // Лёд.
  {
    const x = 6 * TS;
    g.fillStyle = '#a8d8ef';
    g.fillRect(x, 0, TS, TS);
    g.fillStyle = '#c8ecff';
    g.beginPath();
    g.moveTo(x + 2, 22);
    g.lineTo(x + 13, 6);
    g.lineTo(x + 18, 6);
    g.lineTo(x + 7, 22);
    g.fill();
    g.beginPath();
    g.moveTo(x + 17, 30);
    g.lineTo(x + 27, 15);
    g.lineTo(x + 31, 15);
    g.lineTo(x + 21, 30);
    g.fill();
    g.strokeStyle = 'rgba(90,150,190,0.7)';
    g.lineWidth = 1;
    g.strokeRect(x + 0.5, 0.5, TS - 1, TS - 1);
  }

  // Вода: 3 кадра.
  for (let f = 0; f < 3; f++) {
    const x = (7 + f) * TS;
    g.fillStyle = '#17518f';
    g.fillRect(x, 0, TS, TS);
    g.fillStyle = '#1f6cb8';
    g.fillRect(x, 0, TS, 16);
    g.strokeStyle = '#5fb2f0';
    g.lineWidth = 2;
    for (let w = 0; w < 3; w++) {
      const yy = 6 + w * 10 + f * 2;
      g.beginPath();
      g.moveTo(x, yy);
      g.bezierCurveTo(x + 8, yy - 4, x + 18, yy + 4, x + 32, yy);
      g.stroke();
    }
  }
}

boot();
