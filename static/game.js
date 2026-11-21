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
  ABILITY,
  EFFECT,
  AIM_COUNT,
  aimDelta,
  aimFromVector,
  decodeServer,
  decodeRle,
  encodeJoin,
  encodeInput,
  encodeRespawn,
  encodePing,
} from './protocol.js';

import { initI18n, t } from './i18n.js';

// ---------------------------------------------------------------------------
// Константы (зеркалят src/game.rs)
// ---------------------------------------------------------------------------

const TANK_SIZE = 30;
const TILE_SIZE = 32;
const BASE_SPEED = 140;
const TICK_DT = 1 / 30;
const INTERP_DELAY = 100; // мс задержки для интерполяции чужих объектов
const SNAP_HISTORY = 24;

/** Дальность конуса обзора в пикселях. */
const VISION_RANGE = 900;
/** Половина угла конуса обзора (60°, то есть конус 120°). */
const VISION_HALF_ANGLE = Math.PI / 3;
/** Дальность применения способности. */
const ABILITY_RANGE = 1400;

/** Нижний ярус — земля. */
const LEVEL_GROUND = 0;
/** Верхний ярус — площадки и перекрытия. */
const LEVEL_UPPER = 1;
/** Сколько ярусов на карте. */
const LEVEL_COUNT = 2;

/**
 * Длина полосы пандуса в тайлах.
 *
 * Полоса идёт от нижнего этажа к верхнему: тайл с индексом 0 — это
 * «вход» на земле, тайл с индексом 2 — вершина у верхней площадки.
 */
const RAMP_LEN = 3;
/** Индекс тайла в начале полосы (нижний край). */
const RAMP_BOTTOM = 0;
/** Индекс тайла в конце полосы (верхний край). */
const RAMP_TOP = RAMP_LEN - 1;
/**
 * Высота подъёма пандуса в мировых пикселях.
 *
 * Это чисто визуальная величина: коллизии остаются плоскими, но танк
 * на полосе плавно поднимается, а под ним идёт тень по земле.
 */
const RAMP_HEIGHT = 26;

/**
 * Насколько быстро проявляется и гаснет тайл тумана.
 *
 * Плавная интерполяция вместо мгновенного переключения — главное
 * средство от «моргания»: край конуса не щёлкает, а плавно темнеет.
 * Значение — доля пути к цели за кадр: 0.14 даёт примерно 120 мс
 * на проявление тайла, и при повороте взгляда ничего не вспыхивает.
 */
const FOG_FADE = 0.14;

/** Насколько сильно исследованный, но не видимый тайл приглушается. */
const FOG_MEMORY = 0.45;

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
const minimapPanel = document.getElementById('minimap-panel');
const minimapToggle = document.getElementById('minimap-toggle');
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
const statDeck = document.getElementById('stat-deck');
const soundButton = document.getElementById('sound-toggle');
const abilityBar = document.getElementById('ability-bar');
const abilityHint = document.getElementById('ability-hint');

/**
 * Регуляторы громкости: отдельный слайдер на каждую категорию звука.
 *
 * Ключ объекта совпадает с ключом в `audio.gains`.
 */
const VOLUME_INPUTS = {
  master: document.getElementById('vol-master'),
  shots: document.getElementById('vol-shots'),
  explosions: document.getElementById('vol-explosions'),
  engine: document.getElementById('vol-engine'),
  hits: document.getElementById('vol-hits'),
};


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
  /** Выбранная способность: миномёт или авиаподдержка. */
  ability: ABILITY.MORTAR,
  /** Флаг «применить способность» — одноразовый, живёт до отправки пакета. */
  pendingUse: false,
  /** Остаток кулдауна миномёта в тиках — присылается всегда. */
  mortarCd: 0,
  /** Остаток кулдауна авиаподдержки в тиках — присылается всегда. */
  airstrikeCd: 0,
  /** Ярус, на котором сейчас находится игрок. */
  deck: LEVEL_GROUND,
};

/**
 * Туман войны в духе Age of Empires 2.
 *
 * Для каждого яруса храним:
 *  - `explored` — тайлы, где игрок уже был;
 *  - `light`    — текущая освещённость тайла (0..1), плавно меняется;
 *  - `target`   — куда освещённость стремится (0 — темно, 1 — светло);
 *  - `seen`     — «старый» вид тайла: тем, что было в момент последнего взгляда.
 *
 * Важно: видимость считается **по каждому тайлу отдельно** (проверка
 * линии видимости), а не «веером лучей». Веер давал рваный край конуса,
 * и тайлы на его границе то попадали в свет, то нет — это и было моргание.
 * Поверх плавная интерполяция `light` убирает остаточные скачки.
 */
const fog = {
  explored: [],
  light: [],
  target: [],
  seen: [],
  canvas: null,
  ctx: null,
  image: null,
  /** Версия карты, для которой построены массивы. */
  width: 0,
  height: 0,
};

/** Локально предсказанный танк игрока. */
const pred = {
  active: false,
  x: 0,
  y: 0,
  dir: DIR.DOWN,
  /** Направление башни: 64 направления, задаётся курсором мыши. */
  aim: 16,
  /** Направление движения: скорость направлена вдоль него. */
  moveDir: DIR.DOWN,
  vel: 0,
  level: 1,
  hp: 3,
  maxHp: 3,
  shielded: false,
  /** Ярус, на котором находится танк. */
  deck: LEVEL_GROUND,
  /** Направление полосы пандуса под танком (0..3) или -1, если пандуса нет. */
  rampDir: -1,
  /** Индекс тайла пандуса под танком (0 — низ, 2 — вершина) или -1. */
  rampIndex: -1,
  /** Край полосы, на котором меняется ярус: RAMP_TOP вверх, RAMP_BOTTOM вниз, -1 — нет. */
  rampTarget: -1,
  /** Пандус уже сменил ярус — повторно переключаться нельзя, пока стоим на тайле. */
  rampSwitched: false,
  /** Остаток перезарядки — по нему клиент играет звук выстрела. */
  cooldown: 0,
};

/** Курсор мыши: экранные координаты и пересчитанные мировые. */
const mouse = { sx: 0, sy: 0, x: 0, y: 0, inside: false };

/** Визуальное смещение, которое плавно гасится после серверной коррекции. */
const correction = { x: 0, y: 0 };

const camera = { x: 0, y: 0, zoom: 1 };

const keys = new Set();
let mouseDown = false;
let inputAccumulator = 0;

const particles = [];
let terrainCanvases = [];
let terrainCtxs = [];
let sprites = null;
/** Развёрнута ли мини-карта на весь экран. */
let minimapExpanded = false;

// ---------------------------------------------------------------------------
// Звук
// ---------------------------------------------------------------------------

/**
 * Категории звука: у каждой свой регулятор громкости.
 *
 * 'master' — общая громкость, остальные — отдельные регуляторы под каждое
 * действие: выстрелы, взрывы, мотор, попадания и интерфейс.
 */
const VOLUME_CATEGORIES = ['master', 'shots', 'explosions', 'engine', 'hits'];

/** Значения регуляторов по умолчанию. */
const VOLUME_DEFAULTS = Object.freeze({
  master: 0.9,
  shots: 0.8,
  explosions: 0.8,
  engine: 0.7,
  hits: 0.8,
});

/** К какой категории относится тот или иной звук. */
const SFX_CATEGORY = Object.freeze({
  shot: 'shots',
  shot_foe: 'shots',
  explosion: 'explosions',
  hit: 'hits',
  damage: 'hits',
  pickup: 'hits',
  powerup: 'hits',
});

/**
 * Простой звуковой движок на Web Audio API.
 *
 * Все файлы — CC0 (см. `static/sfx/CREDITS.txt`): можно использовать
 * в коммерческой игре без указания авторства и без оплаты.
 */
const audio = {
  ctx: null,
  master: null,
  /** Узлы громкости по категориям: shots / explosions / engine / hits. */
  gains: {},
  /** Текущие значения регуляторов (0..1). */
  volumes: { ...VOLUME_DEFAULTS },
  buffers: new Map(),
  enabled: true,
  loading: false,
  /** Процедурный звук мотора (без внешних файлов). */
  engine: null,
  /** Общий буфер белого шума — используется выстрелами и взрывами. */
  noise: null,
};

/**
 * Звуки, которые грузятся с диска.
 *
 * Выстрела здесь нет: он синтезируется процедурно (`playTankShot`), чтобы
 * звучал как танковый, а не как лазерный.
 */
const SFX_NAMES = ['explosion', 'hit', 'pickup', 'powerup', 'damage'];

/** Звуки из серверного канала: 0 — выстрел, 1 — взрыв, 2 — попадание. */
const SPATIAL_SFX = ['shot_foe', 'explosion', 'hit'];

/**
 * Перевод строки с запасным вариантом.
 *
 * Если ключа нет в словаре, `t()` вернёт сам ключ — тогда показываем
 * русский текст по умолчанию, чтобы интерфейс не остался пустым.
 */
function tr(key, fallback) {
  let value = '';
  try {
    value = t(key);
  } catch (err) {
    value = '';
  }
  return !value || value === key ? fallback : value;
}

/** Читает сохранённые регуляторы из localStorage. */
function loadVolumes() {
  try {
    for (const cat of VOLUME_CATEGORIES) {
      const raw = localStorage.getItem(`tanchiki.vol.${cat}`);
      if (raw === null) continue;
      const num = Number(raw);
      if (Number.isFinite(num)) {
        audio.volumes[cat] = Math.max(0, Math.min(1, num));
      }
    }
  } catch (err) {
    // Приватный режим браузера — просто остаёмся на значениях по умолчанию.
  }
}

/** Сохраняет регуляторы в localStorage. */
function saveVolumes() {
  try {
    for (const cat of VOLUME_CATEGORIES) {
      localStorage.setItem(`tanchiki.vol.${cat}`, String(audio.volumes[cat]));
    }
  } catch (err) {
    // Сохранение недоступно — на игру это не влияет.
  }
}

/** Применяет текущие значения регуляторов к узлам громкости. */
function applyVolumes() {
  if (!audio.ctx || !audio.master) return;
  const now = audio.ctx.currentTime;
  // Общая громкость — общий регулятор плюс кнопка «звук вкл/выкл».
  const master = audio.enabled ? audio.volumes.master : 0;
  audio.master.gain.setTargetAtTime(master, now, 0.03);
  for (const cat of ['shots', 'explosions', 'engine', 'hits']) {
    const node = audio.gains[cat];
    if (node) node.gain.setTargetAtTime(audio.volumes[cat], now, 0.03);
  }
}

/**
 * Подключает слайдеры панели к регуляторам звука.
 *
 * Значения сразу подставляются в input и в подписи, поэтому панель
 * показывает реальные настройки, а не пустые дефолты.
 */
function bindVolumeControls() {
  for (const cat of VOLUME_CATEGORIES) {
    const input = VOLUME_INPUTS[cat];
    if (!input) continue;
    input.value = String(Math.round(audio.volumes[cat] * 100));
    const output = input.parentElement ? input.parentElement.querySelector('output') : null;
    if (output) output.textContent = `${Math.round(audio.volumes[cat] * 100)}%`;
    input.addEventListener('input', () => {
      const num = Number(input.value);
      if (!Number.isFinite(num)) return;
      audio.volumes[cat] = Math.max(0, Math.min(1, num / 100));
      if (output) output.textContent = `${Math.round(num)}%`;
      applyVolumes();
      saveVolumes();
    });
  }
}

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
    audio.master.connect(audio.ctx.destination);
    // По узлу на каждую категорию — так регуляторы независимы друг от друга.
    for (const cat of ['shots', 'explosions', 'engine', 'hits']) {
      const gain = audio.ctx.createGain();
      gain.connect(audio.master);
      audio.gains[cat] = gain;
    }
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
    initEngineSound();
    applyVolumes();
  } catch (err) {
    console.warn('звук недоступен:', err);
  }
  audio.loading = false;
  updateSoundButton();
}

/**
 * Создаёт процедурный звук мотора.
 *
 * Два генератора (пила и меандр) через низкочастотный фильтр: звук
 * «гудит» на холостых оборотах и поднимает тон по мере разгона.
 */
function initEngineSound() {
  if (!audio.ctx || audio.engine) return;
  const osc = audio.ctx.createOscillator();
  osc.type = 'sawtooth';
  osc.frequency.value = 40;
  const sub = audio.ctx.createOscillator();
  sub.type = 'square';
  sub.frequency.value = 20;
  const filter = audio.ctx.createBiquadFilter();
  filter.type = 'lowpass';
  filter.frequency.value = 300;
  filter.Q.value = 3;
  const gain = audio.ctx.createGain();
  gain.gain.value = 0;
  osc.connect(filter);
  sub.connect(filter);
  filter.connect(gain);
  // Мотор идёт через собственный регулятор громкости.
  gain.connect(audio.gains.engine ?? audio.master);
  osc.start();
  sub.start();
  audio.engine = { osc, sub, filter, gain };
}

/**
 * Обновляет звук мотора: громкость и тон зависят от скорости танка.
 *
 * @param {number} speed текущая скорость в пикселях в секунду
 */
function updateEngineSound(speed) {
  const e = audio.engine;
  if (!e || !audio.ctx) return;
  const now = audio.ctx.currentTime;
  const norm = Math.min(1, Math.abs(speed) / BASE_SPEED);
  const level = audio.enabled ? 0.03 + norm * 0.075 : 0;
  e.gain.gain.setTargetAtTime(level, now, 0.08);
  e.osc.frequency.setTargetAtTime(38 + norm * 54, now, 0.12);
  e.sub.frequency.setTargetAtTime(19 + norm * 27, now, 0.12);
  e.filter.frequency.setTargetAtTime(220 + norm * 540, now, 0.12);
}

/** Ставит источник звука в мировую точку (плоскость мира — XZ). */
function setPannerPosition(node, x, y) {
  if (node.positionX) {
    node.positionX.value = x;
    node.positionY.value = 0;
    node.positionZ.value = y;
  } else if (node.setPosition) {
    node.setPosition(x, 0, y);
  }
}

/**
 * Ставит «слушателя» в позицию игрока и направляет его по башне.
 *
 * Благодаря этому выстрел противника звучит слева, справа, спереди или
 * сзади — игрок понимает, с какой стороны атакуют.
 */
function updateAudioListener() {
  if (!audio.ctx) return;
  const l = audio.ctx.listener;
  const px = pred.x + TANK_SIZE / 2;
  const py = pred.y + TANK_SIZE / 2;
  const dir = aimDelta(pred.aim);
  if (l.positionX) {
    l.positionX.value = px;
    l.positionY.value = 0;
    l.positionZ.value = py;
    l.forwardX.value = dir.x;
    l.forwardY.value = 0;
    l.forwardZ.value = dir.y;
    l.upX.value = 0;
    l.upY.value = 1;
    l.upZ.value = 0;
  } else if (l.setPosition) {
    l.setPosition(px, 0, py);
    l.setOrientation(dir.x, 0, dir.y, 0, 1, 0);
  }
}

/** Общий буфер белого шума (создаётся один раз). */
function noiseBuffer() {
  if (audio.noise) return audio.noise;
  const ctx = audio.ctx;
  const len = Math.max(1, Math.floor(ctx.sampleRate * 0.6));
  const buf = ctx.createBuffer(1, len, ctx.sampleRate);
  const data = buf.getChannelData(0);
  for (let i = 0; i < len; i++) data[i] = Math.random() * 2 - 1;
  audio.noise = buf;
  return buf;
}

/**
 * Процедурный звук танкового выстрела.
 *
 * Три слоя складываются в характерный «бум» пушки:
 *  1) низкочастотный свип синуса 150 → 45 Гц — сама тяга выстрела;
 *  2) шумовой всплеск через lowpass — выстрел как таковой;
 *  3) короткий металлический отзвук — ствол и отдача.
 *
 * Звук собирается из генераторов прямо в браузере: никаких внешних
 * файлов, лицензионных вопросов и полного контроля над тембром.
 *
 * @param {AudioNode} dest  куда подключить (узел категории или панораму);
 * @param {number}   volume общая громкость (0..1);
 * @param {number}   pitch  сдвиг тона (1 — штатный выстрел);
 */
function playTankShot(dest, volume = 1, pitch = 1) {
  if (!audio.enabled || !audio.ctx || !dest) return;
  const ctx = audio.ctx;
  if (ctx.state === 'suspended') ctx.resume();
  const now = ctx.currentTime;

  // 1) Низкочастотная «ударная волна».
  const osc = ctx.createOscillator();
  osc.type = 'sine';
  osc.frequency.setValueAtTime(150 * pitch, now);
  osc.frequency.exponentialRampToValueAtTime(45 * pitch, now + 0.3);
  const og = ctx.createGain();
  og.gain.setValueAtTime(0.0001, now);
  og.gain.exponentialRampToValueAtTime(0.95 * volume, now + 0.014);
  og.gain.exponentialRampToValueAtTime(0.0001, now + 0.45);

  // 2) Шумовой всплеск — «порох».
  const noise = ctx.createBufferSource();
  noise.buffer = noiseBuffer();
  const lp = ctx.createBiquadFilter();
  lp.type = 'lowpass';
  lp.frequency.setValueAtTime(2800, now);
  lp.frequency.exponentialRampToValueAtTime(300, now + 0.28);
  lp.Q.value = 0.9;
  const ng = ctx.createGain();
  ng.gain.setValueAtTime(0.0001, now);
  ng.gain.exponentialRampToValueAtTime(0.5 * volume, now + 0.008);
  ng.gain.exponentialRampToValueAtTime(0.0001, now + 0.3);

  // 3) Металлический отзвук ствола.
  const ring = ctx.createOscillator();
  ring.type = 'triangle';
  ring.frequency.setValueAtTime(430 * pitch, now);
  const rg = ctx.createGain();
  rg.gain.setValueAtTime(0.0001, now);
  rg.gain.exponentialRampToValueAtTime(0.2 * volume, now + 0.006);
  rg.gain.exponentialRampToValueAtTime(0.0001, now + 0.16);

  osc.connect(og);
  noise.connect(lp);
  lp.connect(ng);
  ring.connect(rg);
  og.connect(dest);
  ng.connect(dest);
  rg.connect(dest);

  osc.start(now);
  osc.stop(now + 0.5);
  noise.start(now);
  noise.stop(now + 0.35);
  ring.start(now);
  ring.stop(now + 0.2);
}

/**
 * Проигрывает звук с пространственным позиционированием.
 *
 * @param {number} kind  0 — выстрел, 1 — взрыв, 2 — попадание;
 * @param {number} x     мировая координата источника;
 * @param {number} y     мировая координата источника;
 * @param {number} deck  ярус источника.
 */
function playSpatialSound(kind, x, y, deck) {
  if (!audio.enabled || !audio.ctx) return;
  if (audio.ctx.state === 'suspended') audio.ctx.resume();

  const panner = audio.ctx.createPanner();
  panner.panningModel = 'HRTF';
  panner.distanceModel = 'inverse';
  panner.refDistance = 240;
  panner.maxDistance = 2400;
  panner.rolloffFactor = 1.05;
  setPannerPosition(panner, x, y);

  const gain = audio.ctx.createGain();
  // Звук с другого этажа слышен глуше — этаж перекрывает.
  gain.gain.value = deck === pred.deck ? 0.8 : 0.45;
  panner.connect(gain);

  const category = kind === 0 ? 'shots' : kind === 1 ? 'explosions' : 'hits';
  gain.connect(audio.gains[category] ?? audio.master);

  if (kind === 0) {
    // Выстрел противника — тот же танковый бум, только в пространстве.
    playTankShot(panner, 0.95, 0.9 + Math.random() * 0.16);
    return;
  }
  const name = SPATIAL_SFX[kind];
  const buf = audio.buffers.get(name);
  if (!buf) return;
  const src = audio.ctx.createBufferSource();
  src.buffer = buf;
  src.playbackRate.value = 0.88 + Math.random() * 0.24;
  src.connect(panner);
  src.start();
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
  const category = SFX_CATEGORY[name] ?? 'hits';
  gain.connect(audio.gains[category] ?? audio.master);
  src.start();
}

/** Выстрел из пушки игрока — процедурный танковый бум. */
function playOwnShot() {
  playTankShot(audio.gains.shots ?? audio.master, 1);
}

/** Короткий глухой «пуск» способности (миномёт, авиаподдержка). */
function playAbilityLaunch() {
  playTankShot(audio.gains.shots ?? audio.master, 0.4, 1.7);
}

function toggleSound() {
  audio.enabled = !audio.enabled;
  if (audio.enabled && audio.ctx && audio.ctx.state === 'suspended') audio.ctx.resume();
  applyVolumes();
  updateSoundButton();
}

function updateSoundButton() {
  if (!soundButton) return;
  soundButton.textContent = audio.enabled ? tr('hud.soundOn', '🔊 ЗВУК') : tr('hud.soundOff', '🔇 ЗВУК ВЫКЛ');
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
      // Возрождение всегда на нижнем ярусе.
      pred.deck = LEVEL_GROUND;
      resetRampState();
      state.deck = LEVEL_GROUND;
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
  // Карта приходит по ярусам: отдельный RLE-поток на каждый этаж.
  const streams = Array.isArray(map.rle) ? map.rle : [map.rle];
  const layers = [];
  for (let i = 0; i < LEVEL_COUNT; i++) {
    const stream = streams[i];
    layers.push(
      stream
        ? decodeRle(stream, map.width, map.height)
        : new Uint8Array(map.width * map.height),
    );
  }
  state.map = {
    width: map.width,
    height: map.height,
    tileSize: map.tileSize || TILE_SIZE,
    levels: layers.length,
    layers,
  };
  buildTerrainCanvases();
  // Новая карта — память исследованного сбрасывается.
  fog.explored = [];
  fog.light = [];
  fog.target = [];
  fog.seen = [];
  fog.width = 0;
  fog.height = 0;
  ensureFog();
  // Полосы пандуса заранее считаем исследованными на обоих этажах:
  // иначе при подъёме экран на мгновение проваливался бы в черноту.
  markRampsExplored();
  // Если карта сменилась — сбросим предсказание, чтобы не телепортировать танк.
  pred.active = false;
  resetRampState();
  resizeMinimap();
}

/**
 * Помечает тайлы пандусов исследованными на всех этажах.
 *
 * Пандус — единственное место, где игрок переходит между этажами, поэтому
 * видеть его нужно заранее, а не в момент перехода.
 */
function markRampsExplored() {
  const m = state.map;
  if (!m) return;
  for (let d = 0; d < m.levels; d++) {
    const layer = m.layers[d];
    const explored = fog.explored[d];
    if (!layer || !explored) continue;
    for (let i = 0; i < layer.length; i++) {
      if ((layer[i] & 7) === TILE.RAMP) explored[i] = 1;
    }
  }
}

// ---------------------------------------------------------------------------
// Снапшоты
// ---------------------------------------------------------------------------

function applySnapshot(msg) {
  state.focus.x = msg.focusX;
  state.focus.y = msg.focusY;

  // Ярус присылает сервер: он и решает, на каком этаже мы находимся.
  // Синхронизируем сразу — от этого зависит, какие события показывать.
  const mine = msg.players.find((p) => p.id === state.myId);
  if (mine) {
    const serverDeck = mine.deck ?? LEVEL_GROUND;
    if (serverDeck !== pred.deck) {
      pred.deck = serverDeck;
      resetRampState();
    }
    state.deck = pred.deck;
  }

  const players = new Map();
  for (const p of msg.players) {
    players.set(p.id, p);
  }
  const bullets = new Map();
  for (const b of msg.bullets) bullets.set(b.id, b);
  const pickups = new Map();
  for (const p of msg.pickups) pickups.set(p.id, p);
  const effects = new Map();
  for (const e of msg.effects ?? []) effects.set(e.id, e);

  state.snapshots.push({
    t: performance.now(),
    players,
    bullets,
    pickups,
    effects,
  });
  if (state.snapshots.length > SNAP_HISTORY) state.snapshots.shift();

  // Звуковой канал: выстрелы и взрывы слышны даже вне конуса обзора,
  // поэтому по ним игрок понимает, с какой стороны атакуют.
  for (const s of msg.sounds ?? []) {
    if (s.owner === state.myId) continue; // свой выстрел играем локально
    playSpatialSound(s.kind, s.x, s.y, s.deck ?? LEVEL_GROUND);
  }

  // Обновляем карту дельтами.
  if (msg.tiles.length && state.map) {
    for (const t of msg.tiles) {
      const deck = t.deck ?? 0;
      const layer = state.map.layers[deck];
      if (!layer) continue;
      if (t.x < state.map.width && t.y < state.map.height) {
        layer[t.y * state.map.width + t.x] = t.value;
        paintTerrainTile(t.x, t.y, deck);
      }
    }
  }

  // События мира.
  for (const ev of msg.events) {
    const evDeck = ev.deck ?? 0;
    if (evDeck !== state.deck) continue; // события другого этажа не показываем
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
    } else if (ev.kind === 'abilityUsed') {
      spawnAbilityMark(ev.x, ev.y, ev.ability);
      const dist = Math.hypot(ev.x - camera.x, ev.y - camera.y);
      if (dist < 900) playSound('explosion', 0.35 * (1 - dist / 900));
    } else if (ev.kind === 'levelUp') {
      if (ev.id === state.myId) playSound('powerup', 0.75);
    }
  }

  // Свой танк: сверяем предсказание с сервером.
  if (mine) {
    state.score = mine.score;
    state.kills = mine.kills;
    pred.level = mine.level;
    // Башня: направление считает клиент (по курсору мыши) — так наводка
    // не запаздывает на один круг по сети. Сервер получает его во вводе.
    state.mortarCd = mine.mortarCd ?? 0;
    state.airstrikeCd = mine.airstrikeCd ?? 0;
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
  // Правая кнопка мыши — применение выбранной способности.
  if (state.pendingUse) bits |= BIT.USE;
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
  // F — развернуть или свернуть карту на весь экран.
  if (e.code === 'KeyF') {
    e.preventDefault();
    setMinimapExpanded(!minimapExpanded);
    return;
  }
  // Esc — свернуть развёрнутую карту.
  if (e.code === 'Escape') {
    if (minimapExpanded) {
      e.preventDefault();
      setMinimapExpanded(false);
    }
    return;
  }
  // Выбор способности: 1 — миномёт, 2 — авиаподдержка.
  // Обычная пушка отдельной кнопкой не нужна: она на левой кнопке мыши.
  if (e.code === 'Digit1') {
    selectAbility(ABILITY.MORTAR);
    return;
  }
  if (e.code === 'Digit2') {
    selectAbility(ABILITY.AIR_STRIKE);
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

if (minimapToggle) {
  minimapToggle.addEventListener('click', (e) => {
    e.preventDefault();
    e.stopPropagation();
    setMinimapExpanded(!minimapExpanded);
  });
}

/**
 * Выбирает способность.
 * @param {number} ability
 */
function selectAbility(ability) {
  state.ability = ability;
  renderAbilityBar();
  updateAbilityHint();
}

canvas.addEventListener('mousedown', (e) => {
  if (e.button === 0) {
    mouseDown = true;
    return;
  }
  if (e.button === 2) {
    // Правая кнопка — применить выбранную способность в точке курсора.
    e.preventDefault();
    if (state.ability === ABILITY.NONE) {
      updateAbilityHint();
      return;
    }
    state.pendingUse = true;
    playAbilityLaunch();
  }
});
window.addEventListener('mouseup', () => {
  mouseDown = false;
});
canvas.addEventListener('contextmenu', (e) => e.preventDefault());

canvas.addEventListener('mousemove', (e) => {
  const rect = canvas.getBoundingClientRect();
  mouse.sx = e.clientX - rect.left;
  mouse.sy = e.clientY - rect.top;
  mouse.inside = true;
});
canvas.addEventListener('mouseleave', () => {
  mouse.inside = false;
});

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

function mapRectFree(x, y, size, deck = pred.deck) {
  const m = state.map;
  if (!m) return true;
  const layer = m.layers[deck];
  if (!layer) return true;
  const ts = m.tileSize;
  const x0 = Math.floor(x / ts);
  const y0 = Math.floor(y / ts);
  const x1 = Math.floor((x + size - 0.001) / ts);
  const y1 = Math.floor((y + size - 0.001) / ts);
  for (let ty = y0; ty <= y1; ty++) {
    for (let tx = x0; tx <= x1; tx++) {
      if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return false;
      const kind = layer[ty * m.width + tx] & 7;
      if (kind === TILE.BRICK || kind === TILE.STEEL || kind === TILE.WATER) return false;
    }
  }
  return true;
}

function isOnIce(x, y, deck = pred.deck) {
  const m = state.map;
  if (!m) return false;
  const layer = m.layers[deck];
  if (!layer) return false;
  const tx = Math.floor((x + TANK_SIZE / 2) / m.tileSize);
  const ty = Math.floor((y + TANK_SIZE / 2) / m.tileSize);
  if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return false;
  return (layer[ty * m.width + tx] & 7) === TILE.ICE;
}

/** Сбрасывает состояние пандуса (после смены яруса или возрождения). */
function resetRampState() {
  pred.rampDir = -1;
  pred.rampIndex = -1;
  pred.rampTarget = -1;
  pred.rampSwitched = false;
}

/**
 * Данные о полосе пандуса под точкой.
 *
 * В байте тайла пандуса закодированы: вид тайла (младшие 3 бита),
 * направление полосы (биты 3–4) и индекс тайла в полосе (биты 5–6).
 *
 * Направление полосы: 0 — вправо, 1 — вниз, 2 — влево, 3 — вверх.
 * Индекс: 0 — нижний край (уровень земли), 2 — вершина (верхний этаж).
 *
 * @returns {{dir:number, idx:number, tx:number, ty:number}|null}
 */
function rampInfoAt(x, y, deck = pred.deck) {
  const m = state.map;
  if (!m) return null;
  const layer = m.layers[deck];
  if (!layer) return null;
  const cx = x + TANK_SIZE / 2;
  const cy = y + TANK_SIZE / 2;
  const tx = Math.floor(cx / m.tileSize);
  const ty = Math.floor(cy / m.tileSize);
  if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return null;
  const v = layer[ty * m.width + tx];
  if ((v & 7) !== TILE.RAMP) return null;
  return { dir: (v >> 3) & 3, idx: (v >> 5) & 3, tx, ty };
}

/**
 * Высота подъёма пандуса под точкой (в мировых пикселях).
 *
 * Вне пандуса высота этажа постоянна: земля — ноль, верхняя площадка —
 * полная высота подъёма. На самом пандусе высота растёт линейно от
 * нижнего края полосы к верхнему, поэтому подъём выглядит непрерывным.
 */
function rampHeightAt(x, y, deck = pred.deck) {
  const info = rampInfoAt(x, y, deck);
  if (!info) return deck === LEVEL_UPPER ? RAMP_HEIGHT : 0;
  const m = state.map;
  const ts = m.tileSize;
  const cx = x + TANK_SIZE / 2;
  const cy = y + TANK_SIZE / 2;
  // Доля тайла вдоль направления подъёма: 0 — внизу полосы, 1 — вверху.
  let frac;
  if (info.dir === 0) frac = (cx - info.tx * ts) / ts;
  else if (info.dir === 1) frac = (cy - info.ty * ts) / ts;
  else if (info.dir === 2) frac = 1 - (cx - info.tx * ts) / ts;
  else frac = 1 - (cy - info.ty * ts) / ts;
  frac = Math.max(0, Math.min(1, frac));
  // Прогресс по полосе: 0 — нижний край, RAMP_LEN — верхний.
  const progress = info.idx + frac;
  return (progress / RAMP_LEN) * RAMP_HEIGHT;
}

/**
 * Непрерывный переход между ярусами по пандусу.
 *
 * Пандус — полоса из трёх тайлов: тайл 0 — нижний край (уровень земли),
 * тайл 1 — середина, тайл 2 — верхний край (верхняя площадка). Полоса —
 * это наклонная поверхность: на нижнем ярусе ею поднимаются наверх, на
 * верхнем — спускаются вниз.
 *
 * Цель перехода задаёт этаж, а не направление езда: снизу цель — верхний
 * край полосы, сверху — нижний. Поэтому заезд сбоку или с «не того» края
 * тоже отрабатывает правильно, и ярус не меняется случайно.
 *
 * Логика — зеркало серверной `Game::update_deck`.
 */
function updatePredictedDeck() {
  const info = rampInfoAt(pred.x, pred.y, pred.deck);
  if (!info) {
    // Сход с пандуса: следующий заезд начнётся заново.
    resetRampState();
    return;
  }
  if (pred.rampDir === -1) {
    // Только что въехали на пандус: запоминаем полосу и наш тайл в ней.
    pred.rampDir = info.dir;
    pred.rampIndex = info.idx;
    pred.rampSwitched = false;
  }
  // Снизу едем к вершине полосы, сверху — к её основанию.
  pred.rampTarget = pred.deck === LEVEL_GROUND ? RAMP_TOP : RAMP_BOTTOM;
  if (info.idx === pred.rampTarget && !pred.rampSwitched) {
    const next = pred.deck === LEVEL_GROUND ? LEVEL_UPPER : LEVEL_GROUND;
    // Переключаемся только если на новом ярусе тоже можно стоять.
    if (mapRectFree(pred.x, pred.y, TANK_SIZE, next)) {
      pred.deck = next;
      state.deck = next;
      pred.rampSwitched = true;
    }
  }
  pred.rampIndex = info.idx;
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

  // Трамплин меняет ярус: заехали на переход — оказались на другом этаже.
  updatePredictedDeck();

  // Перезарядка: по ней клиент понимает, когда прозвучит выстрел.
  pred.cooldown = Math.max(0, pred.cooldown - dt);
  if ((input & BIT.FIRE) !== 0 && pred.cooldown <= 0) {
    playOwnShot();
    pred.cooldown = fireCooldown(pred.level);
  }
}

// ---------------------------------------------------------------------------
// Туман войны
// ---------------------------------------------------------------------------

/**
 * Пересчитывает мировые координаты курсора.
 *
 * Камера следует за танком с задержкой, поэтому мировая точка курсора
 * меняется даже когда мышь неподвижна.
 */
function updateMouseWorld() {
  mouse.x = camera.x + (mouse.sx - canvas.clientWidth / 2) / camera.zoom;
  mouse.y = camera.y + (mouse.sy - canvas.clientHeight / 2) / camera.zoom;
}

/** Подготавливает массивы тумана под размер карты. */
function ensureFog() {
  const m = state.map;
  if (!m) return false;
  if (fog.width === m.width && fog.height === m.height && fog.explored.length === LEVEL_COUNT) {
    return true;
  }
  fog.width = m.width;
  fog.height = m.height;
  const total = m.width * m.height;
  fog.explored = [];
  fog.light = [];
  fog.target = [];
  fog.seen = [];
  for (let i = 0; i < LEVEL_COUNT; i++) {
    fog.explored.push(new Uint8Array(total));
    fog.light.push(new Float32Array(total));
    fog.target.push(new Float32Array(total));
    fog.seen.push(new Uint8Array(total));
  }
  fog.canvas = document.createElement('canvas');
  fog.canvas.width = m.width;
  fog.canvas.height = m.height;
  fog.ctx = fog.canvas.getContext('2d');
  fog.image = fog.ctx.createImageData(m.width, m.height);
  // «Старый» вид тайлов — снимок того, как карта выглядела изначально.
  for (let d = 0; d < LEVEL_COUNT; d++) {
    const layer = m.layers[d];
    if (layer) fog.seen[d].set(layer);
  }
  return true;
}

/** Тайл по мировым координатам. */
function tileIndexAt(x, y, deck = pred.deck) {
  const m = state.map;
  if (!m) return -1;
  const tx = Math.floor(x / m.tileSize);
  const ty = Math.floor(y / m.tileSize);
  if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return -1;
  return ty * m.width + tx;
}

/** Мешает ли тайл видеть дальше. */
function blocksSight(tx, ty, deck) {
  const m = state.map;
  if (!m) return false;
  if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return true;
  const layer = m.layers[deck];
  if (!layer) return false;
  const kind = layer[ty * m.width + tx] & 7;
  return kind === TILE.BRICK || kind === TILE.STEEL;
}

/**
 * Прямая видимость между двумя тайлами (Брезенхэм по сетке).
 *
 * Считается по каждому тайлу отдельно — это убирает «веер лучей», из-за
 * которого край конуса был рваным и тайлы на нём моргали. Дополнительно
 * проверяется «угол» при диагональном шаге: стена перекрывает обзор
 * симметрично с обеих сторон.
 */
function hasLineOfSight(x0, y0, x1, y1, deck) {
  const m = state.map;
  if (!m) return true;
  if (x0 === x1 && y0 === y1) return true;
  const dx = Math.abs(x1 - x0);
  const sx = x0 < x1 ? 1 : -1;
  const dy = -Math.abs(y1 - y0);
  const sy = y0 < y1 ? 1 : -1;
  let err = dx + dy;
  let x = x0;
  let y = y0;
  for (;;) {
    const px = x;
    const py = y;
    const e2 = 2 * err;
    const movedX = e2 >= dy;
    const movedY = e2 <= dx;
    if (movedX) {
      err += dy;
      x += sx;
    }
    if (movedY) {
      err += dx;
      y += sy;
    }
    if (x === x1 && y === y1) return true;
    if (x < 0 || y < 0 || x >= m.width || y >= m.height) return true;
    if (movedX && movedY) {
      if (blocksSight(px, y, deck) || blocksSight(x, py, deck)) return false;
    }
    if (blocksSight(x, y, deck)) return false;
  }
}

/**
 * Пересчитывает видимую область: конус взгляда из башни.
 *
 * Видимость определяется по каждому тайлу: попадает ли он в конус и есть
 * ли между ним и танком стена. Видимые тайлы запоминаются — их «старый»
 * вид сохраняется в `seen`.
 */
function updateVisibility(cx, cy, aim, deck) {
  const m = state.map;
  if (!m || !ensureFog()) return;
  const target = fog.target[deck];
  const explored = fog.explored[deck];
  const seen = fog.seen[deck];
  const layer = m.layers[deck];
  if (!layer) return;
  target.fill(0);

  const ts = m.tileSize;
  const ptx = Math.floor(cx / ts);
  const pty = Math.floor(cy / ts);
  const maxTiles = Math.ceil(VISION_RANGE / ts);
  const dir = aimDelta(aim);
  const cosLimit = Math.cos(VISION_HALF_ANGLE);

  for (let dy = -maxTiles; dy <= maxTiles; dy++) {
    for (let dx = -maxTiles; dx <= maxTiles; dx++) {
      const tx = ptx + dx;
      const ty = pty + dy;
      if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) continue;
      const vx = tx * ts + ts * 0.5 - cx;
      const vy = ty * ts + ts * 0.5 - cy;
      const dist = Math.hypot(vx, vy);
      if (dist > VISION_RANGE) continue;
      // Тайл должен попадать в конус обзора.
      if (dist > 1 && (vx * dir.x + vy * dir.y) / dist < cosLimit) continue;
      // И не должен быть отделён стеной.
      if (!hasLineOfSight(ptx, pty, tx, ty, deck)) continue;
      const idx = ty * m.width + tx;
      target[idx] = 1;
      explored[idx] = 1;
      seen[idx] = layer[idx];
    }
  }

  // Свой тайл всегда виден.
  const own = tileIndexAt(cx, cy, deck);
  if (own >= 0) {
    target[own] = 1;
    explored[own] = 1;
    seen[own] = layer[own];
  }
}

/**
 * Плавно гасит освещённость тайлов.
 *
 * Значение не прыгает сразу на 0 или 1, а подтягивается к цели — поэтому
 * граница конуса не «щёлкает», а растворяется. Именно это убирает моргание.
 */
function updateFogFade(dt, deck) {
  const m = state.map;
  if (!m || !ensureFog()) return;
  const light = fog.light[deck];
  const target = fog.target[deck];
  const explored = fog.explored[deck];
  const k = 1 - Math.pow(1 - FOG_FADE, dt * 60);
  for (let i = 0; i < light.length; i++) {
    const goal = target[i] > 0 ? 1 : explored[i] ? FOG_MEMORY : 0;
    light[i] += (goal - light[i]) * k;
  }
}

/**
 * Рисует слой тумана одним ImageData: неисследованное — чёрное,
 * исследованное — приглушённое, видимое — чистое.
 */
function paintFogLayer(deck) {
  const g = fog.ctx;
  if (!g || !state.map || !fog.image) return;
  const light = fog.light[deck];
  const data = fog.image.data;
  const total = state.map.width * state.map.height;
  for (let i = 0; i < total; i++) {
    const alpha = Math.round((1 - light[i]) * 255);
    const o = i * 4 + 3;
    // Пишем только изменившиеся пиксели — на кадр это заметно дешевле.
    if (data[o] === alpha) continue;
    data[o] = alpha;
  }
  g.putImageData(fog.image, 0, 0);
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
  // 11 спрайтов в ряд: 4 стадии кирпича, бетон, лес, лёд, 3 кадра воды, трамплин.
  c.width = TS * 11;
  c.height = TS;
  buildSpritesInto(c.getContext('2d'), TS);
  sprites = {
    steel: 4,
    forest: 5,
    ice: 6,
    water: [7, 8, 9],
    ramp: 10,
    ts: TS,
    canvas: c,
  };
}

function spriteCell(index) {
  const ts = sprites.ts;
  return { sx: index * ts, sy: 0 };
}

/** Отдельный холст с «рельефом» карты для мини-карты — по одному на ярус. */
function buildTerrainCanvases() {
  const m = state.map;
  if (!m) return;
  terrainCanvases = [];
  terrainCtxs = [];
  for (let d = 0; d < m.levels; d++) {
    const c = document.createElement('canvas');
    c.width = m.width;
    c.height = m.height;
    const g = c.getContext('2d');
    g.fillStyle = '#0a0e13';
    g.fillRect(0, 0, m.width, m.height);
    terrainCanvases.push(c);
    terrainCtxs.push(g);
  }
  for (let d = 0; d < m.levels; d++) {
    for (let ty = 0; ty < m.height; ty++) {
      for (let tx = 0; tx < m.width; tx++) {
        paintTerrainTileTo(terrainCtxs[d], tx, ty, d);
      }
    }
  }
}

function paintTerrainTile(tx, ty, deck = 0) {
  const g = terrainCtxs[deck];
  if (!g || !state.map) return;
  paintTerrainTileTo(g, tx, ty, deck);
}

function paintTerrainTileTo(g, tx, ty, deck) {
  const m = state.map;
  const layer = m.layers[deck];
  if (!layer) return;
  const v = layer[ty * m.width + tx];
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
    case TILE.RAMP:
      color = '#c9a227';
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

/** Кольцо на месте применения способности. */
function spawnAbilityMark(x, y, ability) {
  const color = ability === ABILITY.AIR_STRIKE ? '#ff9f1c' : '#8ecae6';
  for (let i = 0; i < 14; i++) {
    const angle = (i / 14) * Math.PI * 2;
    particles.push({
      x,
      y,
      vx: Math.cos(angle) * 110,
      vy: Math.sin(angle) * 110,
      life: 0.5,
      max: 0.5,
      size: 3,
      color,
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

/**
 * Рисует тайлы яруса.
 *
 * @param {number} now      время для анимации воды;
 * @param {number} deck     какой ярус рисуем;
 * @param {number} alpha    прозрачность слоя (для «чужого» этажа);
 * @param {boolean} withFog использовать ли туман войны и «старую» память.
 */
function drawTiles(now, deck, alpha, withFog) {
  const m = state.map;
  if (!m || !sprites) return;
  const layer = m.layers[deck];
  if (!layer) return;
  const ts = m.tileSize;
  const halfW = canvas.clientWidth / 2 / camera.zoom;
  const halfH = canvas.clientHeight / 2 / camera.zoom;

  const x0 = Math.max(0, Math.floor((camera.x - halfW) / ts));
  const x1 = Math.min(m.width - 1, Math.ceil((camera.x + halfW) / ts));
  const y0 = Math.max(0, Math.floor((camera.y - halfH) / ts));
  const y1 = Math.min(m.height - 1, Math.ceil((camera.y + halfH) / ts));

  const s = ts * camera.zoom;
  const waterFrame = Math.floor(now / 380) % 3;
  const light = withFog ? fog.light[deck] : null;
  const seen = withFog ? fog.seen[deck] : null;

  ctx.globalAlpha = alpha;
  for (let ty = y0; ty <= y1; ty++) {
    for (let tx = x0; tx <= x1; tx++) {
      const idx = ty * m.width + tx;
      // Видимые тайлы рисуем как есть, а исследованные — по «старому» виду,
      // как в Age of Empires 2: устаревшая карта, без живых обновлений.
      // Переключаемся на «старый» вид уже под затемнением, чтобы смена
      // не была заметна глазом.
      const lit = light ? light[idx] > 0.75 : true;
      const v = lit ? layer[idx] : seen ? seen[idx] : layer[idx];
      const kind = v & 7;
      if (kind === TILE.EMPTY) continue;
      const damage = v >> 3;

      let index;
      if (kind === TILE.BRICK) index = Math.min(3, damage);
      else if (kind === TILE.STEEL) index = sprites.steel;
      else if (kind === TILE.FOREST) index = sprites.forest;
      else if (kind === TILE.ICE) index = sprites.ice;
      else if (kind === TILE.WATER) index = sprites.water[waterFrame];
      else if (kind === TILE.RAMP) index = sprites.ramp;
      else continue;

      const { sx, sy } = spriteCell(index);
      ctx.drawImage(sprites.canvas, sx, sy, 32, 32, worldToScreenX(tx * ts), worldToScreenY(ty * ts), s, s);
    }
  }
  ctx.globalAlpha = 1;
}

function drawPickups(pickups) {
  for (const p of pickups.values()) {
    // Бонусы соседнего этажа не видны.
    if ((p.deck ?? LEVEL_GROUND) !== pred.deck) continue;
    // Бонус виден только там, где игрок уже был.
    const explored = fog.explored[pred.deck];
    if (explored) {
      const idx = tileIndexAt(p.x, p.y, pred.deck);
      if (idx < 0 || !explored[idx]) continue;
    }
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
    // Снаряды соседнего этажа не видны.
    if ((b.deck ?? LEVEL_GROUND) !== pred.deck) continue;
    const x = worldToScreenX(b.x);
    const y = worldToScreenY(b.y);
    const dir = aimDelta(b.dir);
    const dx = dir.x;
    const dy = dir.y;
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
function isInForest(x, y, deck = pred.deck) {
  const m = state.map;
  if (!m) return false;
  const layer = m.layers[deck];
  if (!layer) return false;
  const tx = Math.floor((x + TANK_SIZE / 2) / m.tileSize);
  const ty = Math.floor((y + TANK_SIZE / 2) / m.tileSize);
  if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return false;
  return (layer[ty * m.width + tx] & 7) === TILE.FOREST;
}

function drawTank(p, opts = {}) {
  const size = TANK_SIZE * camera.zoom;
  const x = worldToScreenX(p.x);
  const groundY = worldToScreenY(p.y);
  // Подъём на пандусе: корпус рисуется выше, а тень остаётся на земле —
  // так видно, что танк едет вверх, а не «перепрыгнул» этаж.
  const lift = (opts.lift ?? 0) * camera.zoom;
  const y = groundY - lift;
  const hue = opts.hue ?? 40;
  const level = p.level ?? 1;
  const aim = p.aim ?? 4;

  // Тень на земле.
  if (lift > 0.5) {
    ctx.save();
    ctx.globalAlpha = 0.38 * (1 - Math.min(1, lift / (RAMP_HEIGHT * camera.zoom)) * 0.45);
    ctx.fillStyle = '#000000';
    ctx.beginPath();
    ctx.ellipse(x + size / 2, groundY + size * 0.74, size * 0.42, size * 0.17, 0, 0, Math.PI * 2);
    ctx.fill();
    ctx.restore();
  }

  ctx.save();
  // В чаще танк частично маскируется — становится полупрозрачным.
  ctx.globalAlpha = isInForest(p.x, p.y, p.deck ?? pred.deck) ? (opts.self ? 0.62 : 0.45) : 1;
  ctx.translate(x + size / 2, y + size / 2);

  const half = size / 2;
  const trackW = size * 0.22;

  // --- Корпус: крутится по направлению движения ---
  ctx.save();
  ctx.rotate(ROT[p.dir] ?? 0);

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

  // Кузов.
  ctx.fillStyle = `hsl(${hue}, 58%, 40%)`;
  ctx.fillRect(-half + trackW, -half + size * 0.14, size - trackW * 2, size * 0.82);
  ctx.fillStyle = `hsl(${hue}, 62%, 52%)`;
  ctx.fillRect(-half + trackW, -half + size * 0.14, size - trackW * 2, size * 0.16);
  ctx.restore();

  // --- Башня: крутится по курсору мыши, независимо от корпуса ---
  ctx.save();
  const aimVec = aimDelta(aim);
  ctx.rotate(Math.atan2(aimVec.y, aimVec.x));

  // Стволы: на втором уровне появляется вторая пушка.
  ctx.fillStyle = `hsl(${hue}, 45%, 26%)`;
  if (level >= 2) {
    ctx.fillRect(0, -size * 0.19, size * 0.55, size * 0.12);
    ctx.fillRect(0, size * 0.07, size * 0.55, size * 0.12);
  } else {
    ctx.fillRect(0, -size * 0.06, size * 0.55, size * 0.12);
  }

  // Башня.
  ctx.fillStyle = `hsl(${hue}, 66%, 56%)`;
  ctx.fillRect(-size * 0.18, -size * 0.18, size * 0.36, size * 0.36);
  ctx.fillStyle = `hsl(${hue}, 70%, 66%)`;
  ctx.fillRect(-size * 0.1, -size * 0.1, size * 0.2, size * 0.2);
  ctx.restore();

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

  const deck = pred.deck;

  // Виден только свой этаж: соседний ярус не рисуется вовсе, чтобы не
  // путать планировку и не спорить с туманом.
  drawTiles(now, deck, 1, true);

  // Слой тумана: неисследованное закрыто, исследованное приглушено.
  // Растягиваем с билинейной фильтрацией — край тумана получается мягким.
  if (fog.canvas && state.map) {
    const ts = state.map.tileSize;
    ctx.imageSmoothingEnabled = true;
    ctx.drawImage(
      fog.canvas,
      0,
      0,
      fog.canvas.width,
      fog.canvas.height,
      worldToScreenX(0),
      worldToScreenY(0),
      fog.canvas.width * ts * camera.zoom,
      fog.canvas.height * ts * camera.zoom,
    );
    ctx.imageSmoothingEnabled = false;
  }

  const renderTime = performance.now() - INTERP_DELAY;
  const pair = interpolationPair(renderTime);

  if (pair) {
    const { a, b, f } = pair;

    // --- Бонусы ---
    drawPickups(b.pickups);

    // --- Снаряды ---
    drawBullets(b.bullets);

    // --- Способности в полёте ---
    drawEffects(b.effects);

    // --- Танки ---
    for (const [id, pb] of b.players) {
      if (id === state.myId) continue;
      // Чужие танки с другого этажа не видны — этаж перекрывает.
      if ((pb.deck ?? LEVEL_GROUND) !== deck) continue;
      const pa = a.players.get(id);
      const p = {
        ...pb,
        x: pa ? pa.x + (pb.x - pa.x) * f : pb.x,
        y: pa ? pa.y + (pb.y - pa.y) * f : pb.y,
      };
      drawTank(p, {
        hue: hueFor(id),
        trackPhase: (performance.now() / 60) % 6,
        lift: rampHeightAt(p.x, p.y, p.deck ?? deck),
      });
    }
  }

  // --- Свой танк (предсказание + мягкая коррекция) ---
  if (pred.active) {
    const selfX = pred.x + correction.x;
    const selfY = pred.y + correction.y;
    drawTank(
      {
        x: selfX,
        y: selfY,
        dir: pred.dir,
        aim: pred.aim,
        level: pred.level,
        hp: pred.hp,
        maxHp: pred.maxHp,
        shielded: pred.shielded,
        name: '',
      },
      {
        hue: hueFor(state.myId),
        self: true,
        trackPhase: (performance.now() / 60) % 6,
        lift: rampHeightAt(selfX, selfY, pred.deck),
      },
    );
  }

  drawParticles();
  drawCursor();
}

/** Прицел и подсветка точки применения способности. */
function drawCursor() {
  if (!mouse.inside) return;
  const x = mouse.sx;
  const y = mouse.sy;

  if (state.ability !== ABILITY.NONE) {
    // Круг показывает радиус поражения выбранной способности.
    const radius = (state.ability === ABILITY.MORTAR ? 48 : 66) * camera.zoom;
    ctx.strokeStyle = 'rgba(255,159,28,0.65)';
    ctx.setLineDash([6 * camera.zoom, 5 * camera.zoom]);
    ctx.lineWidth = 1.5 * camera.zoom;
    ctx.beginPath();
    ctx.arc(x, y, radius, 0, Math.PI * 2);
    ctx.stroke();
    ctx.setLineDash([]);
  }

  ctx.strokeStyle = 'rgba(240,162,2,0.9)';
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  ctx.arc(x, y, 9, 0, Math.PI * 2);
  ctx.stroke();
  ctx.beginPath();
  ctx.moveTo(x - 15, y);
  ctx.lineTo(x - 5, y);
  ctx.moveTo(x + 5, y);
  ctx.lineTo(x + 15, y);
  ctx.moveTo(x, y - 15);
  ctx.lineTo(x, y - 5);
  ctx.moveTo(x, y + 5);
  ctx.lineTo(x, y + 15);
  ctx.stroke();
}

/**
 * Рисует эффекты способностей: дуга миномёта и вызов авиаподдержки.
 */
function drawEffects(effects) {
  if (!effects) return;
  for (const e of effects.values()) {
    // Эффекты соседнего этажа не видны.
    if ((e.deck ?? LEVEL_GROUND) !== pred.deck) continue;
    const t = Math.max(0, Math.min(1, e.phase / 100));
    if (e.kind === EFFECT.MORTAR) {
      // Миномёт летит по дуге: горизонталь — по прямой, высота — по параболе.
      const px = e.x + (e.x2 - e.x) * t;
      const py = e.y + (e.y2 - e.y) * t;
      const arc = Math.sin(Math.PI * t) * 90;
      const sx = worldToScreenX(px);
      const sy = worldToScreenY(py - arc);
      const r = 5 * camera.zoom;

      // Тень на земле — видно, куда летит снаряд.
      ctx.strokeStyle = 'rgba(255,255,255,0.18)';
      ctx.setLineDash([4 * camera.zoom, 4 * camera.zoom]);
      ctx.lineWidth = 1.5 * camera.zoom;
      ctx.beginPath();
      ctx.moveTo(worldToScreenX(e.x), worldToScreenY(e.y));
      ctx.lineTo(worldToScreenX(e.x2), worldToScreenY(e.y2));
      ctx.stroke();
      ctx.setLineDash([]);

      // Цель удара.
      ctx.strokeStyle = 'rgba(142,202,230,0.75)';
      ctx.lineWidth = 2 * camera.zoom;
      ctx.beginPath();
      ctx.arc(worldToScreenX(e.x2), worldToScreenY(e.y2), 16 * camera.zoom, 0, Math.PI * 2);
      ctx.stroke();

      // Снаряд.
      ctx.fillStyle = '#e0fbfc';
      ctx.beginPath();
      ctx.arc(sx, sy, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillStyle = '#14212b';
      ctx.beginPath();
      ctx.arc(sx, sy, r * 0.45, 0, Math.PI * 2);
      ctx.fill();
    } else if (e.kind === EFFECT.AIR_STRIKE) {
      // Авиаподдержка: по точке сходится удар — рисуем сужающийся круг.
      const cx = worldToScreenX(e.x);
      const cy = worldToScreenY(e.y);
      const radius = (14 + (1 - t) * 70) * camera.zoom;
      ctx.strokeStyle = `rgba(255,159,28,${0.35 + t * 0.6})`;
      ctx.lineWidth = 2.5 * camera.zoom;
      ctx.beginPath();
      ctx.arc(cx, cy, radius, 0, Math.PI * 2);
      ctx.stroke();
      ctx.fillStyle = `rgba(255,159,28,${0.12 + t * 0.25})`;
      ctx.fill();
      // Пикирующий самолёт.
      if (t > 0.15) {
        const drop = (1 - t) * 420 * camera.zoom;
        ctx.fillStyle = '#ffd166';
        ctx.beginPath();
        ctx.moveTo(cx, cy - drop);
        ctx.lineTo(cx - 7 * camera.zoom, cy - drop + 11 * camera.zoom);
        ctx.lineTo(cx + 7 * camera.zoom, cy - drop + 11 * camera.zoom);
        ctx.closePath();
        ctx.fill();
      }
    }
  }
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
  if (statDeck) {
    const upper = pred.deck === LEVEL_UPPER;
    statDeck.textContent = upper ? tr('hud.deck.upper', 'ВЕРХНИЙ') : tr('hud.deck.lower', 'НИЖНИЙ');
    statDeck.classList.toggle('upper', upper);
  }
}

/**
 * Описания способностей для панели выбора.
 *
 * Обычная пушка сюда не входит: она и так на левой кнопке мыши, отдельная
 * кнопка для неё только занимает место. Названия и подписи берутся из
 * словаря i18n, поэтому переключение языка меняет и их.
 */
const ABILITY_INFO = [
  {
    id: ABILITY.MORTAR,
    key: '1',
    name: () => tr('ability.mortar.name', 'Миномёт'),
    desc: () => tr('ability.mortar.desc', 'через стены · урон 2'),
  },
  {
    id: ABILITY.AIR_STRIKE,
    key: '2',
    name: () => tr('ability.airstrike.name', 'Авиаподдержка'),
    desc: () => tr('ability.airstrike.desc', 'удар с воздуха · урон 3'),
  },
];

/** Кэш кнопок панели способностей: id -> { el, cd }. */
const abilityButtons = new Map();

/** Создаёт кнопки выбора способности. */
function buildAbilityBar() {
  if (!abilityBar) return;
  abilityBar.innerHTML = '';
  abilityButtons.clear();
  for (const info of ABILITY_INFO) {
    const btn = document.createElement('button');
    btn.className = 'ability';
    btn.dataset.ability = String(info.id);
    btn.type = 'button';

    const key = document.createElement('span');
    key.className = 'ability-key';
    key.textContent = info.key;

    const name = document.createElement('span');
    name.className = 'ability-name';
    name.textContent = info.name();

    const desc = document.createElement('span');
    desc.className = 'ability-desc';
    desc.textContent = info.desc();

    const cd = document.createElement('span');
    cd.className = 'ability-cd';

    btn.append(key, name, desc, cd);
    btn.addEventListener('click', (e) => {
      e.preventDefault();
      selectAbility(info.id);
    });
    abilityBar.append(btn);
    abilityButtons.set(info.id, { el: btn, cd });
  }
  renderAbilityBar();
}

/** Обновляет подсветку выбранной способности и остаток кулдауна. */
function renderAbilityBar() {
  if (abilityButtons.size === 0) return;
  for (const [id, { el, cd }] of abilityButtons) {
    el.classList.toggle('active', id === state.ability);
    el.classList.toggle('cooling', cooldownOf(id) > 0);
    // Кулдаун показывается ВСЕГДА: сколько секунд осталось до готовности.
    const left = cooldownOf(id);
    cd.textContent = left > 0 ? `${(left / 30).toFixed(1)} с` : tr('ability.ready', 'ГОТОВ');
    cd.classList.toggle('ready', left === 0);
  }
}

/** Остаток кулдауна способности в тиках. */
function cooldownOf(ability) {
  if (ability === ABILITY.MORTAR) return state.mortarCd;
  if (ability === ABILITY.AIR_STRIKE) return state.airstrikeCd;
  return 0;
}

/** Подсказка под панелью способностей. */
function updateAbilityHint() {
  if (!abilityHint) return;
  if (state.ability === ABILITY.MORTAR) {
    abilityHint.textContent = tr('ability.hint.mortar', 'Миномёт: ПКМ по точке — перелетает стены · 1 / 2 — выбор');
  } else {
    abilityHint.textContent = tr(
      'ability.hint.airstrike',
      'Авиаподдержка: ПКМ по точке — удар с воздуха · 1 / 2 — выбор',
    );
  }
}

/**
 * Перерисовывает все динамические части интерфейса после смены языка.
 *
 * Статические подписи переводятся модулем i18n по атрибутам `data-i18n`,
 * а всё, что клиент рисует сам (способности, кулдауны, ярус, кнопка
 * звука), обновляется здесь.
 */
function refreshLocalizedUi() {
  try {
    buildAbilityBar();
    updateAbilityHint();
    updateSoundButton();
    updateStatus();
    renderLeaderboard();
    if (minimapToggle) {
      const label = minimapExpanded
        ? tr('hud.collapse', 'СВЕРНУТЬ')
        : tr('hud.expand', 'РАЗВЕРНУТЬ');
      minimapToggle.title = label;
      minimapToggle.setAttribute('aria-label', label);
    }
  } catch (err) {
    console.warn('не удалось обновить интерфейс после смены языка:', err);
  }
}

/**
 * Следит за сменой языка.
 *
 * Модуль i18n выставляет `document.documentElement.lang`, поэтому
 * наблюдатель за этим атрибутом — самый надёжный способ узнать о смене
 * языка и перерисовать динамические подписи.
 */
function watchLanguage() {
  try {
    const observer = new MutationObserver(() => refreshLocalizedUi());
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['lang'] });
  } catch (err) {
    // Без наблюдателя язык переключится, но часть подписей останется
    // на старом языке — это не ломает игру.
  }
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

/**
 * Разворачивает мини-карту на весь экран и обратно.
 *
 * Переключение максимально устойчивое: любая ошибка внутри гасится,
 * чтобы кнопка не могла «сломать» игру.
 *
 * @param {boolean} expanded развернуть ли карту
 */
function setMinimapExpanded(expanded) {
  try {
    if (!minimapPanel) return;
    minimapExpanded = !!expanded;
    minimapPanel.classList.toggle('expanded', minimapExpanded);
    if (minimapToggle) {
      minimapToggle.textContent = minimapExpanded ? '⤡' : '⛶';
      const label = minimapExpanded
        ? tr('hud.collapse', 'СВЕРНУТЬ')
        : tr('hud.expand', 'РАЗВЕРНУТЬ');
      minimapToggle.title = label;
      minimapToggle.setAttribute('aria-label', label);
    }
    resizeMinimap();
    renderMinimap();
  } catch (err) {
    console.warn('не удалось переключить карту:', err);
  }
}

/** Текущий масштаб интерфейса из CSS-переменной `--ui`. */
function uiScale() {
  try {
    const raw = getComputedStyle(document.documentElement).getPropertyValue('--ui');
    const num = parseFloat(raw);
    return Number.isFinite(num) && num > 0 ? num : 1;
  } catch (err) {
    return 1;
  }
}

/**
 * Подгоняет размер холста мини-карты под текущий режим.
 *
 * В обычном режиме размер задаёт CSS, в развёрнутом — считаем сами,
 * чтобы карта занимала весь экран и оставалась резкой на HiDPI.
 */
function resizeMinimap() {
  try {
    if (!minimapCanvas || !state.map) return;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    let size;
    if (minimapExpanded) {
      const availW = Math.max(240, window.innerWidth - 80);
      const availH = Math.max(240, window.innerHeight - 170);
      size = Math.floor(Math.min(availW, availH));
      minimapCanvas.style.width = `${size}px`;
      minimapCanvas.style.height = `${size}px`;
    } else {
      minimapCanvas.style.width = '';
      minimapCanvas.style.height = '';
      size = Math.floor(320 * uiScale());
    }
    const px = Math.max(160, Math.min(1600, Math.floor(size * dpr)));
    if (minimapCanvas.width !== px) {
      minimapCanvas.width = px;
      minimapCanvas.height = px;
    }
  } catch (err) {
    console.warn('не удалось изменить размер карты:', err);
  }
}

function renderMinimap() {
  if (!state.map || !terrainCanvases.length) return;
  const m = state.map;
  const deck = pred.deck;
  const w = minimapCanvas.width;
  const h = minimapCanvas.height;
  if (!w || !h) return;
  mctx.clearRect(0, 0, w, h);
  const terrain = terrainCanvases[deck] ?? terrainCanvases[0];
  // Рельеф карты (160×160 тайлов) растягиваем на весь холст: так карта
  // одинаково чёткая и в маленьком, и в полноэкранном виде.
  mctx.imageSmoothingEnabled = false;
  mctx.drawImage(terrain, 0, 0, w, h);

  // Поверх рельефа — туман войны: неисследованное чёрное,
  // исследованное приглушено, видимое — как есть.
  if (fog.canvas) {
    mctx.drawImage(fog.canvas, 0, 0, w, h);
  }

  // Масштаб: сколько пикселей холста приходится на один тайл.
  const scale = w / m.width;

  // Бонусы.
  const snap = state.snapshots[state.snapshots.length - 1];
  if (snap) {
    mctx.fillStyle = '#58a6ff';
    for (const p of snap.pickups.values()) {
      if ((p.deck ?? LEVEL_GROUND) !== deck) continue;
      const cx = (p.x / m.tileSize) * scale;
      const cy = (p.y / m.tileSize) * scale;
      mctx.fillRect(cx - 1.5, cy - 1.5, 3, 3);
    }
    // Танки — только с текущего этажа.
    for (const p of snap.players.values()) {
      if ((p.deck ?? LEVEL_GROUND) !== deck) continue;
      const cx = (p.x / m.tileSize) * scale;
      const cy = (p.y / m.tileSize) * scale;
      if (p.id === state.myId) {
        mctx.fillStyle = '#58d68d';
        mctx.fillRect(cx - 2, cy - 2, 4, 4);
        mctx.strokeStyle = 'rgba(88,214,141,0.6)';
        mctx.lineWidth = 1;
        mctx.beginPath();
        mctx.arc(cx, cy, Math.max(4, 6 * scale), 0, Math.PI * 2);
        mctx.stroke();
      } else {
        mctx.fillStyle = p.isBot ? '#f0a202' : '#f85149';
        mctx.fillRect(cx - 1.5, cy - 1.5, 3, 3);
      }
    }
  }

  // Прямоугольник видимой области — в развёрнутой карте это главная
  // подсказка, где сейчас находится танк.
  const halfW = canvas.clientWidth / 2 / camera.zoom;
  const halfH = canvas.clientHeight / 2 / camera.zoom;
  mctx.strokeStyle = 'rgba(230,237,243,0.45)';
  mctx.lineWidth = 1;
  mctx.strokeRect(
    ((camera.x - halfW) / m.tileSize) * scale,
    ((camera.y - halfH) / m.tileSize) * scale,
    ((halfW * 2) / m.tileSize) * scale,
    ((halfH * 2) / m.tileSize) * scale,
  );
}

// ---------------------------------------------------------------------------
// Главный цикл
// ---------------------------------------------------------------------------

let lastFrame = performance.now();
let fpsAccum = 0;
let fpsFrames = 0;
/** Накопитель времени для пересчёта тумана войны. */
let fogAccumulator = 0;
/** Накопитель времени для обновления мини-карты. */
let minimapAccumulator = 0;

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

  // Курсор мыши задаёт направление башни.
  updateMouseWorld();
  if (pred.active && mouse.inside) {
    const cx = pred.x + correction.x + TANK_SIZE / 2;
    const cy = pred.y + correction.y + TANK_SIZE / 2;
    pred.aim = aimFromVector(mouse.x - cx, mouse.y - cy);
  }

  // Туман войны пересчитываем 30 раз в секунду — как тики сервера.
  fogAccumulator += dt;
  if (fogAccumulator >= TICK_DT) {
    fogAccumulator = 0;
    if (pred.active) {
      const cx = pred.x + correction.x + TANK_SIZE / 2;
      const cy = pred.y + correction.y + TANK_SIZE / 2;
      updateVisibility(cx, cy, pred.aim, pred.deck);
    } else if (state.focus.x || state.focus.y) {
      updateVisibility(state.focus.x, state.focus.y, pred.aim, pred.deck);
    }
  }
  // А затухание идёт каждый кадр: край конуса растворяется, а не щёлкает.
  updateFogFade(dt, pred.deck);
  paintFogLayer(pred.deck);

  // Звук мотора: громче и выше на высокой скорости.
  updateEngineSound(pred.active && !state.dead ? pred.vel : 0);
  // Слушатель стоит на месте игрока и смотрит туда же, куда башня.
  updateAudioListener();

  updateParticles(dt);
  renderWorld(now);
  renderAbilityBar();
  updateStatus();

  // Мини-карта обновляется реже основной сцены: в развёрнутом виде она
  // крупная, и перерисовывать её 60 раз в секунду незачем.
  minimapAccumulator += dt;
  if (minimapAccumulator >= (minimapExpanded ? 1 / 15 : 1 / 30)) {
    minimapAccumulator = 0;
    renderMinimap();
  }

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

  // Язык интерфейса: применяем сохранённый и следим за переключением.
  try {
    initI18n();
  } catch (err) {
    console.warn('не удалось инициализировать язык:', err);
  }
  watchLanguage();

  // Регуляторы громкости: восстанавливаем сохранённые значения.
  loadVolumes();
  bindVolumeControls();

  // Спрайты тайлов рисуем один раз в offscreen-холст.
  buildSprites();

  // Панель выбора способности.
  buildAbilityBar();
  updateAbilityHint();

  resizeCanvas();
  resizeMinimap();
  window.addEventListener('resize', () => {
    resizeCanvas();
    resizeMinimap();
  });

  showWelcome();
  connect();

  // Ввод отправляем с той же частотой, что и серверные тики.
  setInterval(() => {
    if (!state.connected) return;
    state.seq += 1;
    const flags = currentInput();
    send(
      encodeInput(
        state.seq,
        flags,
        pred.aim % AIM_COUNT,
        state.ability,
        mouse.x,
        mouse.y,
      ),
    );
    // Флаг применения способности одноразовый: отправлен — и сброшен.
    if ((flags & BIT.USE) !== 0) state.pendingUse = false;
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

  // Трамплин — переход между ярусами. Жёлтые шевроны показывают,
  // куда уезжает танк.
  {
    const x = 10 * TS;
    g.fillStyle = '#4a3c12';
    g.fillRect(x, 0, TS, TS);
    g.strokeStyle = '#e3b341';
    g.lineWidth = 3;
    g.lineCap = 'round';
    for (let i = 0; i < 3; i++) {
      const yy = 7 + i * 9;
      g.beginPath();
      g.moveTo(x + 7, yy + 5);
      g.lineTo(x + TS / 2, yy);
      g.lineTo(x + TS - 7, yy + 5);
      g.stroke();
    }
    g.strokeStyle = 'rgba(0,0,0,0.5)';
    g.lineWidth = 1;
    g.strokeRect(x + 0.5, 0.5, TS - 1, TS - 1);
  }
}

boot();
