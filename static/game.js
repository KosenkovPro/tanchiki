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

import { initI18n, t, setLanguage, getLanguage } from './i18n.js';

// ---------------------------------------------------------------------------
// Константы (зеркалят src/map.rs)
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

/** Нижний слой — дно ямки (высота −1). */
const LEVEL_PIT = 0;
/** Средний слой — земля (высота 0). */
const LEVEL_GROUND = 1;
/** Верхний слой — возвышенности и мостики (высота +1). */
const LEVEL_UPPER = 2;
/** Сколько слоёв на карте. */
const LEVEL_COUNT = 3;

/**
 * Высота слоя: 0 → −1, 1 → 0, 2 → +1.
 * @param {number} layer индекс слоя
 * @returns {number} высота в «этажах»
 */
function layerElev(layer) {
  return layer - 1;
}

/**
 * Вид тайла «дно ямки».
 *
 * В `static/protocol.js` такого вида пока нет, поэтому константа объявлена
 * здесь. Если протокол её добавит — будет взят именно его вариант.
 */
const TILE_PIT = TILE.PIT ?? 7;

/** Бит «пандус ведёт вниз (земля ↔ ямка)» в байте тайла. */
const RAMP_FLAG_PIT = 0x80;

/**
 * Может ли снаряд слоя `bulletLayer` поразить танк слоя `targetLayer`.
 *
 * Зеркало серверного `flat_shot_hits`: настильный огонь летит по своему
 * слою, а снаряд с верхнего яруса (+1) дополнительно достаёт до земли (0).
 *
 * @param {number} bulletLayer слой снаряда
 * @param {number} targetLayer слой цели
 * @returns {boolean}
 */
function flatShotHits(bulletLayer, targetLayer) {
  return bulletLayer === targetLayer || (bulletLayer === LEVEL_UPPER && targetLayer === LEVEL_GROUND);
}

/**
 * Пустой тайл.
 *
 * На земле пустота — это трава (или дыра в яму), на слое ямок — сплошная
 * земля над головой, на верхнем ярусе — настил.
 */
const TILE_EMPTY = TILE.EMPTY;

/**
 * Длина полосы пандуса в тайлах.
 *
 * Полоса идёт от нижней стороны к верхней: тайл с индексом 0 — это
 * «низ» полосы (дно ямки или земля), тайл с индексом 2 — «верх»
 * (кромка земли или вершина холма).
 */
const RAMP_LEN = 3;
/** Индекс тайла в начале полосы (нижний край). */
const RAMP_BOTTOM = 0;
/** Индекс тайла в конце полосы (верхний край). */
const RAMP_TOP = RAMP_LEN - 1;
/** Индекс среднего тайла полосы — по нему видно, что танк проехал полосу. */
const RAMP_MIDDLE = RAMP_LEN - 2;
/**
 * Высота подъёма пандуса в мировых пикселях.
 *
 * Это чисто визуальная величина: коллизии остаются плоскими, но танк
 * на полосе плавно поднимается, а под ним идёт тень по земле.
 */
const RAMP_HEIGHT = 26;

/**
 * Высота боковой стенки уступа (мост, холм) в пикселях.
 *
 * Стенка показывает, что верхний ярус — это не плоская картинка, а
 * настоящий перепад высоты.
 */
const LEDGE_HEIGHT = 10;

/**
 * Ширина «нависающей кромки» ямки в пикселях.
 *
 * Кромка рисуется поверх ямы и создаёт ощущение углубления.
 */
const PIT_RIM = 7;

/**
 * Палитра рельефа.
 *
 * Карта светлая: сочная зелёная трава, тёплая кладка, насыщенная вода.
 * Цвета подобраны так, чтобы все виды тайлов различались с одного взгляда.
 */
const TERRAIN = Object.freeze({
  grass: ['#6fb04a', '#68a744', '#77b852', '#5f9e3f'],
  brick: '#b4572c',
  brickDark: '#7d3a1c',
  steel: '#b9c2cc',
  steelDark: '#8b949f',
  water: '#1f6fd0',
  waterDeep: '#174f9c',
  forest: '#3f9147',
  forestLight: '#57ab5f',
  ice: '#bfe6f5',
  pit: '#6d4a2e',
  pitDark: '#4c3320',
  pitWall: '#6f3520',
  deck: '#b58b52',
  deckDark: '#8a663a',
  deckTop: '#cda468',
  shadow: 'rgba(20,26,16,0.28)',
  fog: [26, 44, 24],
});

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
/** Старая кнопка звука в подсказках: её может уже не быть в DOM. */
const soundButton = document.getElementById('sound-toggle');
/** Компактная панель звука в правом нижнем углу. */
const soundHub = document.getElementById('sound-hub');
const soundHubBtn = document.getElementById('sound-hub-btn');
const soundPopover = document.getElementById('sound-popover');
const muteButton = document.getElementById('btn-mute');
/** Выбор размера мини-карты. */
const mapSizeSelect = document.getElementById('minimap-size');
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

/**
 * Размеры мини-карты в пикселях — по кругу переключаются клавишей K.
 */
const MINIMAP_SIZES = [150, 200, 260, 320];

/** Размер мини-карты по умолчанию. */
const MINIMAP_SIZE_DEFAULT = 200;

/** Ключ в localStorage, где хранится размер мини-карты. */
const MINIMAP_SIZE_KEY = 'tanchiki.mapSize';


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
/** Текущий размер мини-карты в пикселях (150 / 200 / 260 / 320). */
let minimapSize = MINIMAP_SIZE_DEFAULT;

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
 * Четыре слоя складываются в характерный «бум» пушки:
 *  1) глубокий сабовый удар — 90 → 38 Гц за 0.12 с: тяга и вес выстрела;
 *  2) тело выстрела — синус 150 → 45 Гц;
 *  3) короткий пороховой щелчок — шум через узкий фильтр;
 *  4) металлический резонанс ствола — короткий отзвук.
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

  // 1) Сабовый удар: резкий спад частоты — самый «низкий» слой звука.
  const sub = ctx.createOscillator();
  sub.type = 'sine';
  sub.frequency.setValueAtTime(90 * pitch, now);
  sub.frequency.exponentialRampToValueAtTime(38 * pitch, now + 0.12);
  const subGain = ctx.createGain();
  subGain.gain.setValueAtTime(0.0001, now);
  subGain.gain.exponentialRampToValueAtTime(0.85 * volume, now + 0.008);
  subGain.gain.exponentialRampToValueAtTime(0.0001, now + 0.32);

  // 2) Тело выстрела.
  const osc = ctx.createOscillator();
  osc.type = 'sine';
  osc.frequency.setValueAtTime(150 * pitch, now);
  osc.frequency.exponentialRampToValueAtTime(45 * pitch, now + 0.3);
  const og = ctx.createGain();
  og.gain.setValueAtTime(0.0001, now);
  og.gain.exponentialRampToValueAtTime(0.55 * volume, now + 0.014);
  og.gain.exponentialRampToValueAtTime(0.0001, now + 0.45);

  // 3) Пороховой щелчок: короткий шум через узкий фильтр.
  const click = ctx.createBufferSource();
  click.buffer = noiseBuffer();
  const bp = ctx.createBiquadFilter();
  bp.type = 'bandpass';
  bp.frequency.setValueAtTime(2600 * pitch, now);
  bp.Q.value = 1.1;
  const clickGain = ctx.createGain();
  clickGain.gain.setValueAtTime(0.0001, now);
  clickGain.gain.exponentialRampToValueAtTime(0.35 * volume, now + 0.004);
  clickGain.gain.exponentialRampToValueAtTime(0.0001, now + 0.07);

  // 4) Металлический резонанс ствола: шум в узкой полосе + треугольник.
  const metal = ctx.createBufferSource();
  metal.buffer = noiseBuffer();
  const mb = ctx.createBiquadFilter();
  mb.type = 'bandpass';
  mb.frequency.setValueAtTime(1500 * pitch, now);
  mb.frequency.exponentialRampToValueAtTime(700 * pitch, now + 0.12);
  mb.Q.value = 6;
  const mg = ctx.createGain();
  mg.gain.setValueAtTime(0.0001, now);
  mg.gain.exponentialRampToValueAtTime(0.25 * volume, now + 0.005);
  mg.gain.exponentialRampToValueAtTime(0.0001, now + 0.16);

  const ring = ctx.createOscillator();
  ring.type = 'triangle';
  ring.frequency.setValueAtTime(430 * pitch, now);
  const rg = ctx.createGain();
  rg.gain.setValueAtTime(0.0001, now);
  rg.gain.exponentialRampToValueAtTime(0.15 * volume, now + 0.006);
  rg.gain.exponentialRampToValueAtTime(0.0001, now + 0.16);

  sub.connect(subGain);
  osc.connect(og);
  click.connect(bp);
  bp.connect(clickGain);
  metal.connect(mb);
  mb.connect(mg);
  ring.connect(rg);
  subGain.connect(dest);
  og.connect(dest);
  clickGain.connect(dest);
  mg.connect(dest);
  rg.connect(dest);

  sub.start(now);
  sub.stop(now + 0.36);
  osc.start(now);
  osc.stop(now + 0.5);
  click.start(now);
  click.stop(now + 0.09);
  metal.start(now);
  metal.stop(now + 0.18);
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

/**
 * Открыта ли всплывающая панель звука.
 * @returns {boolean}
 */
function isSoundPopoverOpen() {
  if (!soundPopover) return false;
  return soundPopover.classList.contains('open') || soundPopover.classList.contains('is-open');
}

/**
 * Открывает или закрывает панель звука.
 *
 * Класс `open`/`is-open` — для стилей, `hidden` — для старого соглашения
 * проекта: так панель открывается при любом оформлении.
 *
 * @param {boolean} open
 */
function setSoundPopoverOpen(open) {
  if (!soundPopover) return;
  soundPopover.classList.toggle('open', open);
  soundPopover.classList.toggle('is-open', open);
  soundPopover.classList.toggle('hidden', !open);
  if (soundHubBtn) soundHubBtn.setAttribute('aria-expanded', open ? 'true' : 'false');
}

/**
 * Обновляет иконки звука и подпись кнопки mute.
 */
function updateSoundButton() {
  const icon = audio.enabled ? '🔊' : '🔇';
  const label = tr('hud.soundToggle', 'Sound');
  if (soundHubBtn) {
    soundHubBtn.textContent = icon;
    soundHubBtn.classList.toggle('off', !audio.enabled);
    soundHubBtn.classList.toggle('muted', !audio.enabled);
    soundHubBtn.setAttribute('aria-label', label);
    soundHubBtn.setAttribute('title', label);
  }
  // Старая кнопка в подсказках может остаться в DOM — работаем с ней,
  // если она есть.
  if (soundButton) {
    soundButton.textContent = audio.enabled ? tr('hud.soundOn', '🔊 ЗВУК') : tr('hud.soundOff', '🔇 ЗВУК ВЫКЛ');
    soundButton.classList.toggle('off', !audio.enabled);
  }
  if (muteButton) {
    muteButton.textContent = audio.enabled
      ? tr('hud.mute', 'MUTE')
      : tr('hud.unmute', 'UNMUTE');
    muteButton.classList.toggle('muted', !audio.enabled);
  }
}

/**
 * Подсвечивает кнопки выбора языка.
 */
function updateLanguageButtons() {
  const current = getLanguage();
  for (const btn of document.querySelectorAll('[data-lang-set]')) {
    btn.classList.toggle('is-active', btn.getAttribute('data-lang-set') === current);
  }
}

/**
 * Включает язык по кнопке `[data-lang-set="en|ru"]`.
 *
 * Слушатель делегирован на документ: кнопки есть и на стартовом экране,
 * и в панели звука, а появятся они могут когда угодно.
 */
function bindLanguageButtons() {
  document.addEventListener('click', (e) => {
    const target = e.target;
    if (!target || typeof target.closest !== 'function') return;
    const btn = target.closest('[data-lang-set]');
    if (!btn) return;
    e.preventDefault();
    const lang = btn.getAttribute('data-lang-set');
    if (lang !== 'en' && lang !== 'ru') return;
    setLanguage(lang);
    updateLanguageButtons();
  });
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
  // Маска настила верхнего яруса: мостики и холмы, а не пустота.
  // Считаем её до рельефа мини-карты — от неё зависит цвет настила.
  refreshUpperDeckMask();
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
      const deck = t.deck ?? LEVEL_GROUND;
      const layer = state.map.layers[deck];
      if (!layer) continue;
      if (t.x < state.map.width && t.y < state.map.height) {
        layer[t.y * state.map.width + t.x] = t.value;
        // Настил верхнего яруса мог измениться — пересчитываем маску.
        refreshUpperDeckMask();
        paintTerrainTile(t.x, t.y, deck);
      }
    }
  }

  // События мира.
  for (const ev of msg.events) {
    const evDeck = ev.deck ?? LEVEL_GROUND;
    if (evDeck !== state.deck) continue; // события другого слоя не показываем
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
  // K — размер мини-карты: переключение по кругу.
  if (e.code === 'KeyK') {
    e.preventDefault();
    cycleMapSize();
    return;
  }
  // F — развернуть или свернуть карту на весь экран.
  if (e.code === 'KeyF') {
    e.preventDefault();
    setMinimapExpanded(!minimapExpanded);
    return;
  }
  // Esc — свернуть развёрнутую карту или закрыть панель звука.
  if (e.code === 'Escape') {
    if (minimapExpanded) {
      e.preventDefault();
      setMinimapExpanded(false);
      return;
    }
    if (isSoundPopoverOpen()) {
      e.preventDefault();
      setSoundPopoverOpen(false);
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

// Иконка динамика в правом нижнем углу: только открывает панель.
// Mute переключается отдельной кнопкой и клавишей M.
if (soundHubBtn) {
  soundHubBtn.addEventListener('click', (e) => {
    e.preventDefault();
    e.stopPropagation();
    initAudio();
    setSoundPopoverOpen(!isSoundPopoverOpen());
  });
}

if (muteButton) {
  muteButton.addEventListener('click', (e) => {
    e.preventDefault();
    e.stopPropagation();
    initAudio();
    toggleSound();
  });
}

// Клик вне панели звука закрывает её.
document.addEventListener('click', (e) => {
  if (!isSoundPopoverOpen()) return;
  const target = e.target;
  if (!target) return;
  if (soundHub && typeof soundHub.contains === 'function' && soundHub.contains(target)) return;
  if (soundPopover && typeof soundPopover.contains === 'function' && soundPopover.contains(target)) return;
  setSoundPopoverOpen(false);
});

// Выбор размера мини-карты.
if (mapSizeSelect) {
  mapSizeSelect.addEventListener('change', () => {
    const num = Number(mapSizeSelect.value);
    if (MINIMAP_SIZES.includes(num)) setMapSize(num);
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

/**
 * Свободно ли место для танка в прямоугольнике на заданном слое.
 *
 * Зеркало серверного `GameMap::rect_is_free`:
 *  - на слое ямок проходимы только дно (`Pit`) и пандус (`Ramp`); пустота
 *    здесь означает «сверху сплошная земля», а не «пусто»;
 *  - на земле тайл непроходим, если сам он блокирует (кирпич/бетон/вода)
 *    либо если под ним ямка (на слое ямок `Pit` или `Brick`) — это дыра,
 *    в которую можно попасть только по спускному пандусу;
 *  - на верхнем ярусе блокируют кирпич, бетон и вода.
 *
 * @param {number} x     левый край в мировых пикселях
 * @param {number} y     верхний край в мировых пикселях
 * @param {number} size  размер танка (сторона квадрата)
 * @param {number} layer слой (0 — ямка, 1 — земля, 2 — верх)
 * @returns {boolean}
 */
function mapRectFree(x, y, size, layer = pred.deck) {
  const m = state.map;
  if (!m) return true;
  const tiles = m.layers[layer];
  if (!tiles) return true;
  const pitLayer = m.layers[LEVEL_PIT];
  const ts = m.tileSize;
  const x0 = Math.floor(x / ts);
  const y0 = Math.floor(y / ts);
  const x1 = Math.floor((x + size - 0.001) / ts);
  const y1 = Math.floor((y + size - 0.001) / ts);
  for (let ty = y0; ty <= y1; ty++) {
    for (let tx = x0; tx <= x1; tx++) {
      if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return false;
      const kind = tiles[ty * m.width + tx] & 7;
      if (layer === LEVEL_PIT) {
        // Дно ямки и пандус — единственные проходимые тайлы.
        if (kind !== TILE_PIT && kind !== TILE.RAMP) return false;
        continue;
      }
      if (kind === TILE.BRICK || kind === TILE.STEEL || kind === TILE.WATER) return false;
      if (layer === LEVEL_UPPER) {
        // На верхнем ярусе пустота — пропасть: ездить можно только по
        // настилам мостов и холмов.
        if (kind === TILE_EMPTY && (tiles[ty * m.width + tx] & DECK_FLAG) === 0) return false;
      }
      if (layer === LEVEL_GROUND && pitLayer) {
        // Дыра в земле: под нами либо дно ямки, либо её стенка.
        const below = pitLayer[ty * m.width + tx] & 7;
        if (below !== TILE_EMPTY && below !== TILE.RAMP) return false;
      }
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
 * направление полосы (биты 3–4), индекс тайла в полосе (биты 5–6) и бит 7 —
 * «пандус ведёт вниз, в ямку».
 *
 * Направление полосы: 0 — вправо, 1 — вниз, 2 — влево, 3 — вверх.
 * Индекс: 0 — низкая сторона полосы, 2 — высокая.
 *
 * @param {number} x     мировая координата (левый край танка)
 * @param {number} y     мировая координата (верхний край танка)
 * @param {number} layer слой
 * @returns {{dir:number, idx:number, toPit:boolean, tx:number, ty:number}|null}
 */
function rampInfoAt(x, y, layer = pred.deck) {
  const m = state.map;
  if (!m) return null;
  const tiles = m.layers[layer];
  if (!tiles) return null;
  const cx = x + TANK_SIZE / 2;
  const cy = y + TANK_SIZE / 2;
  const tx = Math.floor(cx / m.tileSize);
  const ty = Math.floor(cy / m.tileSize);
  if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return null;
  const v = tiles[ty * m.width + tx];
  if ((v & 7) !== TILE.RAMP) return null;
  return {
    dir: (v >> 3) & 3,
    idx: (v >> 5) & 3,
    toPit: (v & RAMP_FLAG_PIT) !== 0,
    tx,
    ty,
  };
}

/**
 * Высота подъёма пандуса под точкой (в мировых пикселях).
 *
 * Вне пандуса высота слоя постоянна: земля — ноль, верхний ярус — полная
 * высота подъёма, дно ямки — та же высота вниз.
 *
 * На самом пандусе высота меняется линейно вдоль полосы: `progress` — это
 * положение точки на полосе (0 — низкая сторона, RAMP_LEN — высокая).
 * Подъёмный пандус ведёт вверх (земля → верх), спускной — вниз
 * (дно ямки → земля), поэтому у спускного знак отрицательный.
 *
 * @param {number} x     мировая координата
 * @param {number} y     мировая координата
 * @param {number} layer слой
 * @returns {number} высота в пикселях
 */
function rampHeightAt(x, y, layer = pred.deck) {
  const info = rampInfoAt(x, y, layer);
  if (!info) {
    if (layer === LEVEL_UPPER) return RAMP_HEIGHT;
    if (layer === LEVEL_PIT) return -RAMP_HEIGHT;
    return 0;
  }
  const m = state.map;
  const ts = m.tileSize;
  const cx = x + TANK_SIZE / 2;
  const cy = y + TANK_SIZE / 2;
  // Доля тайла вдоль направления полосы: 0 — низкая сторона, 1 — высокая.
  let frac;
  if (info.dir === 0) frac = (cx - info.tx * ts) / ts;
  else if (info.dir === 1) frac = (cy - info.ty * ts) / ts;
  else if (info.dir === 2) frac = 1 - (cx - info.tx * ts) / ts;
  else frac = 1 - (cy - info.ty * ts) / ts;
  frac = Math.max(0, Math.min(1, frac));
  // Прогресс по полосе: 0 — низкий край, RAMP_LEN — высокий.
  const progress = info.idx + frac;
  if (info.toPit) {
    // Спускной пандус: низ — дно ямки (−RAMP_HEIGHT), верх — земля (0).
    return -((RAMP_LEN - progress) / RAMP_LEN) * RAMP_HEIGHT;
  }
  return (progress / RAMP_LEN) * RAMP_HEIGHT;
}

/**
 * Непрерывный переход между слоями по пандусу.
 *
 * Пандус — полоса из трёх тайлов: тайл 0 — низкая сторона, тайл 2 — высокая.
 * Подъёмный пандус связывает землю и верх, спускной — ямку и землю.
 *
 * Правило перехода (зеркало серверного `Game::update_deck`):
 *  - на низкой стороне полосы цель перехода — край высокой стороны (idx 2);
 *  - на высокой стороне цель — край низкой стороны (idx 0);
 *  - слой меняется, только если на новом слое танк помещается.
 *
 * Сторона определяется не сравнением слоёв, а самим пандусом
 * (`ramp_low_layer`/`ramp_high_layer`), поэтому подъёмный и спускной
 * пандусы обрабатываются одинаково.
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
  // Низкая и высокая стороны полосы — по типу пандуса.
  const lowLayer = info.toPit ? LEVEL_PIT : LEVEL_GROUND;
  const highLayer = info.toPit ? LEVEL_GROUND : LEVEL_UPPER;
  const onLowSide = pred.deck === lowLayer;
  pred.rampTarget = onLowSide ? RAMP_TOP : RAMP_BOTTOM;
  // Смена тайла в полосе — это новый заезд: счётчик переключений сбрасываем
  // (зеркало серверного `update_deck`).
  if (info.idx !== pred.rampIndex) {
    pred.rampSwitched = false;
  }
  // Переход — только если танк реально проехал полосу: перед крайним тайлом
  // должен быть средний. Наехать сбоку на край пандуса — не переход.
  const prevIndex = pred.rampIndex;
  if (info.idx === pred.rampTarget && prevIndex === RAMP_MIDDLE && !pred.rampSwitched) {
    const next = onLowSide ? highLayer : lowLayer;
    // Переключаемся только если на новом слое тоже можно стоять.
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
  // Туман — не чёрный, а тёмно-зелёный: карта выглядит светлее, но
  // неисследованное всё равно не читается.
  const pixels = fog.image.data;
  for (let i = 0; i < pixels.length; i += 4) {
    pixels[i] = TERRAIN.fog[0];
    pixels[i + 1] = TERRAIN.fog[1];
    pixels[i + 2] = TERRAIN.fog[2];
    pixels[i + 3] = 0;
  }
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
 * Плавно гасит освещённость тайлов всех слоёв.
 *
 * Значение не прыгает сразу на 0 или 1, а подтягивается к цели — поэтому
 * граница конуса не «щёлкает», а растворяется. Именно это убирает моргание.
 *
 * Гасим сразу все слои: соседние слои тоже видны сквозь дыры и мосты,
 * и их освещённость должна вести себя так же.
 *
 * @param {number} dt время кадра в секундах
 */
function updateFogFade(dt) {
  const m = state.map;
  if (!m || !ensureFog()) return;
  const k = 1 - Math.pow(1 - FOG_FADE, dt * 60);
  for (let d = 0; d < LEVEL_COUNT; d++) {
    const light = fog.light[d];
    const target = fog.target[d];
    const explored = fog.explored[d];
    for (let i = 0; i < light.length; i++) {
      const goal = target[i] > 0 ? 1 : explored[i] ? FOG_MEMORY : 0;
      light[i] += (goal - light[i]) * k;
    }
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
  // Спрайты в ряд: 4 стадии кирпича, бетон, лес, лёд, 3 кадра воды,
  // пандус вверх, пандус в ямку, дно ямки, настил моста, 4 варианта травы.
  c.width = TS * 18;
  c.height = TS;
  buildSpritesInto(c.getContext('2d'), TS);
  sprites = {
    steel: 4,
    forest: 5,
    ice: 6,
    water: [7, 8, 9],
    ramp: 10,
    pit: 11,
    pitRamp: 12,
    deck: 13,
    grass: [14, 15, 16, 17],
    ts: TS,
    canvas: c,
  };
}

function spriteCell(index) {
  const ts = sprites.ts;
  return { sx: index * ts, sy: 0 };
}

/**
 * Детерминированный хеш координат тайла.
 *
 * Нужен, чтобы трава выглядела слегка неровной, но одинаково при каждом
 * запуске: вариант оттенка выбирается по хешу, а не случайно.
 *
 * @param {number} tx
 * @param {number} ty
 * @returns {number}
 */
function tileHash(tx, ty) {
  let h = (tx * 73856093) ^ (ty * 19349663);
  h = (h ^ (h >>> 13)) >>> 0;
  return h;
}

/**
 * Есть ли под этим тайлом земли ямка (дыра в земле).
 *
 * На слое земли тайл ямки пустой, а на слое ямок — дно (`Pit`) или стенка
 * (`Brick`). Такой тайл земли не закрашивается травой: в нём видно яму.
 *
 * @param {number} tx
 * @param {number} ty
 * @returns {boolean}
 */
function isHoleAt(tx, ty) {
  const m = state.map;
  if (!m) return false;
  const ground = m.layers[LEVEL_GROUND];
  const pit = m.layers[LEVEL_PIT];
  if (!ground || !pit) return false;
  if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return false;
  const idx = ty * m.width + tx;
  if ((ground[idx] & 7) !== TILE_EMPTY) return false;
  const below = pit[idx] & 7;
  return below === TILE_PIT || below === TILE.BRICK;
}

/**
 * Маска настила верхнего яруса.
 *
 * На верхнем слое пустые тайлы — это настил: мостики над ямками, подходы
 * к холмам и площадки. Клиент рисует настил там, где он действительно
 * есть: над ямкой, рядом с холмом и в коридоре, который ведёт от пандуса.
 */
let upperDeckMask = null;

/** Бит 3 у пустого тайла верхнего яруса: это настил — по нему ездят. */
const DECK_FLAG = 0x08;

/** Бит 4 у настила верхнего яруса: это мостик, а не холм. */
const BRIDGE_FLAG = 0x10;

/** Смещение тайла по направлению полосы пандуса. */
const RAMP_DX = [1, 0, -1, 0];
const RAMP_DY = [0, 1, 0, -1];

/**
 * Пересчитывает маску настила верхнего яруса.
 *
 * Сервер помечает настил прямо в байте тайла (бит 3), поэтому маска — это
 * точная копия того, что можно проехать наверху: мостики и холмы.
 */
function refreshUpperDeckMask() {
  const m = state.map;
  if (!m) return;
  const upper = m.layers[LEVEL_UPPER];
  if (!upper) {
    upperDeckMask = null;
    return;
  }
  const mask = new Uint8Array(m.width * m.height);
  for (let i = 0; i < upper.length; i++) {
    const v = upper[i];
    // Настил — пустой тайл с флагом: по нему ездят.
    if ((v & 7) === TILE_EMPTY && (v & DECK_FLAG) !== 0) {
      mask[i] = 1;
    }
  }
  upperDeckMask = mask;
}

/** Есть ли настил верхнего яруса в этом тайле. */
function isUpperDeck(tx, ty) {
  const m = state.map;
  if (!m || !upperDeckMask) return false;
  if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return false;
  return upperDeckMask[ty * m.width + tx] === 1;
}

/** Отдельный холст с «рельефом» карты для мини-карты — по одному на слой. */
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
    // Фон слоя: земля — трава, ямка — грунт. Верхний ярус остаётся
    // прозрачным: там пустота (пропасть), сквозь неё видно землю внизу.
    if (d === LEVEL_PIT) {
      g.fillStyle = TERRAIN.pitDark;
      g.fillRect(0, 0, m.width, m.height);
    } else if (d === LEVEL_GROUND) {
      g.fillStyle = TERRAIN.grass[1];
      g.fillRect(0, 0, m.width, m.height);
    }
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

function paintTerrainTile(tx, ty, deck = LEVEL_GROUND) {
  const g = terrainCtxs[deck];
  if (!g || !state.map) return;
  paintTerrainTileTo(g, tx, ty, deck);
}

/**
 * Рисует один тайл в «рельеф» мини-карты.
 *
 * Цвет выбирается по виду тайла; на земле пустой тайл — это трава,
 * а если под ним ямка — дыра, и она подсвечивается отдельным цветом.
 */
function paintTerrainTileTo(g, tx, ty, deck) {
  const m = state.map;
  const layer = m.layers[deck];
  if (!layer) return;
  const v = layer[ty * m.width + tx];
  const kind = v & 7;
  let color = null;
  switch (kind) {
    case TILE.BRICK:
      color = deck === LEVEL_PIT ? TERRAIN.pitWall : TERRAIN.brick;
      break;
    case TILE.STEEL:
      color = TERRAIN.steel;
      break;
    case TILE.WATER:
      color = TERRAIN.water;
      break;
    case TILE.FOREST:
      color = TERRAIN.forest;
      break;
    case TILE.ICE:
      color = TERRAIN.ice;
      break;
    case TILE.RAMP:
      color = deck === LEVEL_PIT ? '#d9a05b' : '#f2c94c';
      break;
    case TILE_PIT:
      color = TERRAIN.pit;
      break;
    default:
      if (deck === LEVEL_GROUND) {
        // Пустой тайл земли: трава, а если под ним ямка — дыра.
        color = isHoleAt(tx, ty) ? '#2a1c10' : TERRAIN.grass[tileHash(tx, ty) % TERRAIN.grass.length];
      } else if (deck === LEVEL_UPPER) {
        // Верхний ярус — настил: мостики (дерево) и холмы (камень).
        // Пустота — пропасть: её не красим, сквозь неё видно землю внизу.
        if (isUpperDeck(tx, ty)) {
          color = (layer[ty * m.width + tx] & BRIDGE_FLAG) !== 0 ? TERRAIN.deck : TERRAIN.deckTop;
        } else {
          return;
        }
      } else {
        // Слой ямок: пустота означает сплошную землю над головой.
        color = '#2f2a20';
      }
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
 * Угол поворота спрайта пандуса под направление полосы.
 *
 * Спрайт нарисован шевронами вверх, поэтому для направления полосы
 * его нужно повернуть на угол dir.
 */
const RAMP_ROTATION = [Math.PI / 2, Math.PI, -Math.PI / 2, 0];

/**
 * Границы видимой области карты в тайлах.
 * @returns {{x0:number, x1:number, y0:number, y1:number}}
 */
function visibleTileBounds() {
  const m = state.map;
  const ts = m.tileSize;
  const halfW = canvas.clientWidth / 2 / camera.zoom;
  const halfH = canvas.clientHeight / 2 / camera.zoom;
  return {
    x0: Math.max(0, Math.floor((camera.x - halfW) / ts)),
    x1: Math.min(m.width - 1, Math.ceil((camera.x + halfW) / ts)),
    y0: Math.max(0, Math.floor((camera.y - halfH) / ts)),
    y1: Math.min(m.height - 1, Math.ceil((camera.y + halfH) / ts)),
  };
}

/**
 * Рисует спрайт тайла в мировых координатах.
 * @param {number} index индекс спрайта
 * @param {number} tx
 * @param {number} ty
 * @param {number} ts  размер тайла
 */
function blitTile(index, tx, ty, ts) {
  const { sx, sy } = spriteCell(index);
  const s = ts * camera.zoom;
  ctx.drawImage(sprites.canvas, sx, sy, 32, 32, worldToScreenX(tx * ts), worldToScreenY(ty * ts), s, s);
}

/**
 * Рисует спрайт пандуса, повёрнутый по направлению полосы.
 *
 * Шевроны на пандусе показывают, в какую сторону ведёт полоса.
 *
 * @param {number} index индекс спрайта
 * @param {number} tx
 * @param {number} ty
 * @param {number} ts  размер тайла
 * @param {number} dir  направление полосы (0..3)
 */
function blitRamp(index, tx, ty, ts, dir) {
  const { sx, sy } = spriteCell(index);
  const s = ts * camera.zoom;
  const cx = worldToScreenX(tx * ts) + s / 2;
  const cy = worldToScreenY(ty * ts) + s / 2;
  ctx.save();
  ctx.translate(cx, cy);
  ctx.rotate(RAMP_ROTATION[dir] ?? 0);
  ctx.drawImage(sprites.canvas, sx, sy, 32, 32, -s / 2, -s / 2, s, s);
  ctx.restore();
}

/**
 * Прозрачность слоя ямок.
 *
 * Дно ямки видно сквозь дыру в земле, поэтому считаем освещение и земли
 * над ямой, и того слоя, на котором стоит игрок (с моста тоже видно в яму).
 *
 * @param {number} idx
 * @returns {number}
 */
function pitAlpha(idx) {
  const pit = fog.light[LEVEL_PIT];
  const ground = fog.light[LEVEL_GROUND];
  const active = fog.light[pred.deck];
  return Math.max(
    pit ? pit[idx] : 0,
    ground ? ground[idx] : 0,
    active ? active[idx] : 0,
  );
}

/**
 * Рисует слой ямок: дно, стенки и спускные пандусы.
 *
 * Слой ямок находится ниже земли, поэтому рисуется первым — сквозь дыру
 * в земле видно грунт и внутренние стенки.
 */
function drawPitLayer() {
  const m = state.map;
  if (!m || !sprites) return;
  const pit = m.layers[LEVEL_PIT];
  if (!pit) return;
  const ts = m.tileSize;
  const b = visibleTileBounds();
  for (let ty = b.y0; ty <= b.y1; ty++) {
    for (let tx = b.x0; tx <= b.x1; tx++) {
      const idx = ty * m.width + tx;
      const v = pit[idx];
      const kind = v & 7;
      if (kind === TILE_EMPTY) continue;
      const alpha = pitAlpha(idx);
      if (alpha <= 0.03) continue;
      ctx.globalAlpha = Math.min(1, alpha);
      if (kind === TILE_PIT) {
        blitTile(sprites.pit, tx, ty, ts);
      } else if (kind === TILE.BRICK) {
        // Стенки окопа — тёмный кирпич.
        blitTile(0, tx, ty, ts);
        ctx.fillStyle = 'rgba(24,12,6,0.45)';
        ctx.fillRect(worldToScreenX(tx * ts), worldToScreenY(ty * ts), ts * camera.zoom, ts * camera.zoom);
      } else if (kind === TILE.RAMP) {
        blitRamp(sprites.pitRamp, tx, ty, ts, (v >> 3) & 3);
      }
    }
  }
  ctx.globalAlpha = 1;
}

/**
 * Рисует «нависающие кромки» ямок.
 *
 * По краю дыры земля нависает над ямой, поэтому кромка рисуется поверх
 * дна с тенью внутрь — яма читается как углубление, а не как дыра в фото.
 */
function drawPitRim() {
  const m = state.map;
  if (!m) return;
  const ts = m.tileSize;
  const zoom = camera.zoom;
  const b = visibleTileBounds();
  const band = PIT_RIM * zoom;
  for (let ty = b.y0; ty <= b.y1; ty++) {
    for (let tx = b.x0; tx <= b.x1; tx++) {
      if (!isHoleAt(tx, ty)) continue;
      const idx = ty * m.width + tx;
      const alpha = pitAlpha(idx);
      if (alpha <= 0.03) continue;
      const sx = worldToScreenX(tx * ts);
      const sy = worldToScreenY(ty * ts);
      const s = ts * zoom;
      ctx.globalAlpha = Math.min(1, alpha);
      // Верхняя кромка (сосед сверху — сплошная земля).
      if (!isHoleAt(tx, ty - 1)) {
        const grad = ctx.createLinearGradient(0, sy, 0, sy + band);
        grad.addColorStop(0, 'rgba(26,16,8,0.85)');
        grad.addColorStop(1, 'rgba(26,16,8,0)');
        ctx.fillStyle = grad;
        ctx.fillRect(sx, sy, s, band);
        ctx.fillStyle = 'rgba(18,10,4,0.9)';
        ctx.fillRect(sx, sy, s, Math.max(1, 2 * zoom));
      }
      // Нижняя кромка.
      if (!isHoleAt(tx, ty + 1)) {
        const grad = ctx.createLinearGradient(0, sy + s - band, 0, sy + s);
        grad.addColorStop(0, 'rgba(26,16,8,0)');
        grad.addColorStop(1, 'rgba(26,16,8,0.8)');
        ctx.fillStyle = grad;
        ctx.fillRect(sx, sy + s - band, s, band);
      }
      // Левая и правая кромки.
      if (!isHoleAt(tx - 1, ty)) {
        const grad = ctx.createLinearGradient(sx, 0, sx + band, 0);
        grad.addColorStop(0, 'rgba(26,16,8,0.8)');
        grad.addColorStop(1, 'rgba(26,16,8,0)');
        ctx.fillStyle = grad;
        ctx.fillRect(sx, sy, band, s);
      }
      if (!isHoleAt(tx + 1, ty)) {
        const grad = ctx.createLinearGradient(sx + s - band, 0, sx + s, 0);
        grad.addColorStop(0, 'rgba(26,16,8,0)');
        grad.addColorStop(1, 'rgba(26,16,8,0.8)');
        ctx.fillStyle = grad;
        ctx.fillRect(sx + s - band, sy, band, s);
      }
    }
  }
  ctx.globalAlpha = 1;
}

/**
 * Рисует слой земли: траву, препятствия и пандусы.
 *
 * В местах ям ничего не рисуется — там сквозь дыру видно слой ямок.
 */
function drawGroundLayer(now) {
  const m = state.map;
  if (!m || !sprites) return;
  const ground = m.layers[LEVEL_GROUND];
  if (!ground) return;
  const ts = m.tileSize;
  const b = visibleTileBounds();
  const waterFrame = Math.floor(now / 380) % 3;
  const light = fog.light[LEVEL_GROUND];
  const seen = fog.seen[LEVEL_GROUND];
  for (let ty = b.y0; ty <= b.y1; ty++) {
    for (let tx = b.x0; tx <= b.x1; tx++) {
      const idx = ty * m.width + tx;
      // Видимые тайлы рисуем как есть, а исследованные — по «старому» виду,
      // как в Age of Empires 2: устаревшая карта, без живых обновлений.
      const lit = light ? light[idx] > 0.75 : true;
      const v = lit ? ground[idx] : seen ? seen[idx] : ground[idx];
      const kind = v & 7;
      const alpha = light ? light[idx] : 1;
      if (alpha <= 0.03) continue;
      ctx.globalAlpha = Math.min(1, alpha);
      if (kind === TILE.EMPTY) {
        // Пустой тайл земли — трава, но в яме сквозь дыру видно грунт.
        if (isHoleAt(tx, ty)) continue;
        blitTile(sprites.grass[tileHash(tx, ty) % sprites.grass.length], tx, ty, ts);
        continue;
      }
      const damage = v >> 3;
      let index;
      if (kind === TILE.BRICK) index = Math.min(3, damage);
      else if (kind === TILE.STEEL) index = sprites.steel;
      else if (kind === TILE.FOREST) index = sprites.forest;
      else if (kind === TILE.ICE) index = sprites.ice;
      else if (kind === TILE.WATER) index = sprites.water[waterFrame];
      else if (kind === TILE.RAMP) {
        blitRamp(sprites.ramp, tx, ty, ts, (v >> 3) & 3);
        continue;
      } else continue;
      blitTile(index, tx, ty, ts);
    }
  }
  ctx.globalAlpha = 1;
}

/**
 * Прозрачность верхнего яруса.
 *
 * Сверху верхний ярус виден как обычно. Снизу (с земли) — полупрозрачным,
 * так читается, что мосты и холмы нависают над землёй. Но только там, где
 * видна сама земля: за пределами конуса обзора верхнего яруса нет.
 *
 * @param {number} idx
 * @returns {number}
 */
function upperAlpha(idx) {
  const upper = fog.light[LEVEL_UPPER];
  const upperValue = upper ? upper[idx] : 1;
  if (pred.deck === LEVEL_UPPER) return Math.min(1, Math.max(0.25, upperValue));
  const active = fog.light[pred.deck];
  const gate = active ? active[idx] : 1;
  return Math.min(1, Math.max(0.35, upperValue) * gate);
}

/**
 * Есть ли на верхнем ярусе сплошная поверхность в этом тайле.
 *
 * Стоя на верхнем ярусе, игрок видит весь настил целиком; снизу — только
 * мосты и холмы, которые нависают над землёй.
 *
 * @param {number} tx
 * @param {number} ty
 * @returns {boolean}
 */
function isUpperSolid(tx, ty) {
  const m = state.map;
  const upper = m.layers[LEVEL_UPPER];
  if (!upper) return false;
  if (tx < 0 || ty < 0 || tx >= m.width || ty >= m.height) return false;
  const kind = upper[ty * m.width + tx] & 7;
  if (kind === TILE.BRICK || kind === TILE.STEEL) return true;
  if (kind === TILE.RAMP) return true;
  if (kind !== TILE_EMPTY) return false;
  // Пустота верхнего яруса — пропасть, проезжает только настил.
  return isUpperDeck(tx, ty);
}

/**
 * Тень, которую мосты и холмы отбрасывают на землю.
 *
 * Рисуется между землёй и верхним ярусом: тень лежит на земле, а сам
 * мост — выше.
 */
function drawUpperShadows() {
  const m = state.map;
  if (!m) return;
  // В ямке верхнего яруса не видно — мы под землёй.
  if (pred.deck === LEVEL_PIT) return;
  const upper = m.layers[LEVEL_UPPER];
  if (!upper) return;
  const ts = m.tileSize;
  const zoom = camera.zoom;
  const b = visibleTileBounds();
  for (let ty = b.y0; ty <= b.y1; ty++) {
    for (let tx = b.x0; tx <= b.x1; tx++) {
      if (!isUpperSolid(tx, ty)) continue;
      // Тень падает вниз-вправо от конструкции.
      const idx = ty * m.width + tx;
      ctx.globalAlpha = upperAlpha(idx) * 0.45;
      ctx.fillStyle = '#101a0c';
      ctx.fillRect(
        worldToScreenX(tx * ts) + 3 * zoom,
        worldToScreenY((ty + 1) * ts) - 6 * zoom,
        ts * zoom,
        ts * zoom * 0.55,
      );
    }
  }
  ctx.globalAlpha = 1;
}

/**
 * Рисует верхний ярус: настил мостов, холмы и их боковые стенки.
 *
 * Уступ виден по нижней кромке — так перепад высоты читается сразу.
 */
function drawUpperLayer() {
  const m = state.map;
  if (!m || !sprites) return;
  // В ямке верхнего яруса не видно — мы под землёй.
  if (pred.deck === LEVEL_PIT) return;
  const upper = m.layers[LEVEL_UPPER];
  if (!upper) return;
  const ts = m.tileSize;
  const zoom = camera.zoom;
  const b = visibleTileBounds();
  const wall = LEDGE_HEIGHT * zoom;
  for (let ty = b.y0; ty <= b.y1; ty++) {
    for (let tx = b.x0; tx <= b.x1; tx++) {
      const idx = ty * m.width + tx;
      const v = upper[idx];
      const kind = v & 7;
      // Стоя на верхнем ярусе, видно весь настил; снизу — только мосты и холмы.
      const deck = kind === TILE_EMPTY ? isUpperSolid(tx, ty) : false;
      if (kind === TILE_EMPTY && !deck) continue;
      const alpha = upperAlpha(idx);
      if (alpha <= 0.03) continue;
      ctx.globalAlpha = Math.min(1, alpha);
      if (kind === TILE.BRICK) blitTile(Math.min(3, v >> 3), tx, ty, ts);
      else if (kind === TILE.STEEL) blitTile(sprites.steel, tx, ty, ts);
      else if (kind === TILE.RAMP) blitRamp(sprites.ramp, tx, ty, ts, (v >> 3) & 3);
      else if (deck) blitTile(sprites.deck, tx, ty, ts);
      // Боковая стенка уступа: там, где под конструкцией пусто.
      if (!isUpperSolid(tx, ty + 1)) {
        const sx = worldToScreenX(tx * ts);
        const sy = worldToScreenY((ty + 1) * ts);
        ctx.fillStyle = TERRAIN.deckDark;
        ctx.fillRect(sx, sy, ts * zoom, wall);
        ctx.fillStyle = 'rgba(30,20,8,0.35)';
        ctx.fillRect(sx, sy + wall - 2 * zoom, ts * zoom, 2 * zoom);
      }
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

/**
 * Радиус, в котором снаряд считается «своим».
 *
 * В протоколе у снаряда нет владельца, поэтому свои выстрелы узнаются по
 * близости к своему танку: они рождаются у башни и летят дальше.
 */
const OWN_BULLET_RADIUS = 420;

/**
 * Свой ли снаряд.
 *
 * Если сервер когда-нибудь пришлёт владельца — берём его, иначе смотрим
 * на расстояние до танка игрока.
 *
 * @param {object} b снаряд
 * @returns {boolean}
 */
function isOwnBullet(b) {
  if (b.owner !== undefined && b.owner !== null) return b.owner === state.myId;
  const dx = b.x - (pred.x + TANK_SIZE / 2);
  const dy = b.y - (pred.y + TANK_SIZE / 2);
  return Math.hypot(dx, dy) <= OWN_BULLET_RADIUS;
}

/**
 * Рисует снаряды.
 *
 * Свои выстрелы рисуются всегда: их не отсекает ни туман войны, ни
 * соседний слой — иначе игрок теряет из виду свой же снаряд.
 */
function drawBullets(bullets) {
  for (const b of bullets.values()) {
    const own = isOwnBullet(b);
    // Снаряды соседнего этажа не видны, кроме собственных.
    if (!own && (b.deck ?? LEVEL_GROUND) !== pred.deck) continue;
    const x = worldToScreenX(b.x);
    const y = worldToScreenY(b.y);
    const dir = aimDelta(b.dir);
    const dx = dir.x;
    const dy = dir.y;
    const size = (own ? 7 : 6) * camera.zoom;

    // След снаряда.
    ctx.strokeStyle = own ? 'rgba(255,235,150,0.6)' : 'rgba(255,214,102,0.35)';
    ctx.lineWidth = size * 0.55;
    ctx.beginPath();
    ctx.moveTo(x - dx * size * 2.2, y - dy * size * 2.2);
    ctx.lineTo(x, y);
    ctx.stroke();

    if (own) {
      // Свечение — свой снаряд не должен теряться на фоне.
      ctx.fillStyle = 'rgba(255,214,102,0.35)';
      ctx.beginPath();
      ctx.arc(x, y, size * 1.5, 0, Math.PI * 2);
      ctx.fill();
    }

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

/**
 * Скруглённый прямоугольник.
 *
 * Отдельная функция вместо `ctx.roundRect`, чтобы рисование одинаково
 * работало во всех браузерах.
 *
 * @param {number} x
 * @param {number} y
 * @param {number} w
 * @param {number} h
 * @param {number} r радиус скругления
 */
function roundRect(x, y, w, h, r) {
  const rr = Math.max(0, Math.min(r, Math.abs(w) / 2, Math.abs(h) / 2));
  ctx.beginPath();
  ctx.moveTo(x + rr, y);
  ctx.lineTo(x + w - rr, y);
  ctx.quadraticCurveTo(x + w, y, x + w, y + rr);
  ctx.lineTo(x + w, y + h - rr);
  ctx.quadraticCurveTo(x + w, y + h, x + w - rr, y + h);
  ctx.lineTo(x + rr, y + h);
  ctx.quadraticCurveTo(x, y + h, x, y + h - rr);
  ctx.lineTo(x, y + rr);
  ctx.quadraticCurveTo(x, y, x + rr, y);
  ctx.closePath();
}

/**
 * Цвет детали танка с учётом высоты слоя.
 *
 * На верхнем ярусе танок светлее (ближе к солнцу), в ямке — темнее на 20%.
 *
 * @param {number} hue
 * @param {number} sat
 * @param {number} light
 * @param {number} elev высота слоя (−1, 0, +1)
 * @returns {string}
 */
function tankColor(hue, sat, light, elev) {
  const k = elev > 0 ? 1.08 : elev < 0 ? 0.8 : 1;
  return `hsl(${hue}, ${sat}%, ${Math.max(16, Math.min(74, light * k))}%)`;
}

/**
 * Рисует танк.
 *
 * Модель построена честно: корпус с двумя симметричными гусеницами по
 * бокам (одинаковой ширины, с одинаковой проступью) и башня, которая
 * поворачивается по направлению прицела.
 *
 * Высота слоя читается на самом танке: на верхнем ярусе (+1) он крупнее
 * и светлее, в ямке (−1) — мельче и темнее.
 *
 * @param {object} p    состояние танка
 * @param {object} opts {hue, self, trackPhase, lift}
 */
function drawTank(p, opts = {}) {
  const deck = p.deck ?? pred.deck;
  const elev = layerElev(deck);
  const zoom = camera.zoom;
  // На верхнем ярусе танк крупнее, в ямке — мельче.
  const scale = elev > 0 ? 1.07 : elev < 0 ? 0.94 : 1;
  const size = TANK_SIZE * zoom * scale;
  const x = worldToScreenX(p.x);
  const groundY = worldToScreenY(p.y);
  // Подъём на пандусе: корпус рисуется выше, а тень остаётся на земле —
  // так видно, что танк едет вверх, а не «перепрыгнул» этаж.
  const lift = (opts.lift ?? 0) * zoom;
  const y = groundY - lift;
  const hue = opts.hue ?? 40;
  const level = p.level ?? 1;
  const aim = p.aim ?? 4;

  // Тень на земле: с верхнего яруса она длинная и смещённая.
  const shadowOffX = elev > 0 ? 6 * zoom : 0;
  const shadowOffY = elev > 0 ? 5 * zoom : 0;
  ctx.save();
  ctx.globalAlpha =
    (elev > 0 ? 0.3 : elev < 0 ? 0.16 : 0.24) *
    (lift > 0.5 ? 1 - Math.min(1, lift / (RAMP_HEIGHT * zoom)) * 0.45 : 1);
  ctx.fillStyle = '#000000';
  ctx.beginPath();
  ctx.ellipse(
    x + size / 2 + shadowOffX,
    groundY + size * 0.74 + shadowOffY,
    size * (elev > 0 ? 0.5 : 0.42),
    size * (elev > 0 ? 0.16 : 0.17),
    0,
    0,
    Math.PI * 2,
  );
  ctx.fill();
  ctx.restore();

  ctx.save();
  // В чаще танк частично маскируется — становится полупрозрачным.
  ctx.globalAlpha = isInForest(p.x, p.y, deck) ? (opts.self ? 0.62 : 0.45) : 1;
  ctx.translate(x + size / 2, y + size / 2);

  const half = size / 2;
  const trackW = size * 0.2;
  const trackTop = -half + size * 0.05;
  const trackH = size * 0.9;

  // --- Корпус: крутится по направлению движения ---
  ctx.save();
  ctx.rotate(ROT[p.dir] ?? 0);

  // Гусеницы: две одинаковые, симметричные относительно корпуса.
  ctx.fillStyle = '#1b1f25';
  roundRect(-half, trackTop, trackW, trackH, trackW * 0.35);
  ctx.fill();
  roundRect(half - trackW, trackTop, trackW, trackH, trackW * 0.35);
  ctx.fill();

  // Проступь: одинаковый рисунок на обеих гусеницах, без смещения.
  ctx.fillStyle = '#49525e';
  const tread = ((opts.trackPhase ?? 0) * 4) % 6;
  for (let i = trackTop + tread; i < trackTop + trackH - 1; i += 6 * zoom) {
    ctx.fillRect(-half, i, trackW, 2 * zoom);
    ctx.fillRect(half - trackW, i, trackW, 2 * zoom);
  }

  // Кузов.
  const bodyX = -half + trackW;
  const bodyW = size - trackW * 2;
  const bodyY = -half + size * 0.1;
  const bodyH = size * 0.8;
  ctx.fillStyle = tankColor(hue, 58, 40, elev);
  roundRect(bodyX, bodyY, bodyW, bodyH, size * 0.07);
  ctx.fill();
  // Верхняя кромка корпуса — ближе к свету.
  ctx.fillStyle = tankColor(hue, 62, 54, elev);
  roundRect(bodyX + size * 0.03, bodyY + size * 0.03, bodyW - size * 0.06, bodyH * 0.2, size * 0.04);
  ctx.fill();
  // Кормовые листы.
  ctx.fillStyle = tankColor(hue, 40, 30, elev);
  ctx.fillRect(bodyX, bodyY + bodyH * 0.42, bodyW, size * 0.05);
  ctx.restore();

  // --- Башня: крутится по курсору мыши, независимо от корпуса ---
  ctx.save();
  const aimVec = aimDelta(aim);
  ctx.rotate(Math.atan2(aimVec.y, aimVec.x));

  // Стволы: на втором уровне появляется вторая пушка.
  ctx.fillStyle = tankColor(hue, 45, 26, elev);
  if (level >= 2) {
    ctx.fillRect(0, -size * 0.19, size * 0.55, size * 0.12);
    ctx.fillRect(0, size * 0.07, size * 0.55, size * 0.12);
  } else {
    ctx.fillRect(0, -size * 0.06, size * 0.55, size * 0.12);
  }

  // Башня.
  ctx.fillStyle = tankColor(hue, 66, 56, elev);
  roundRect(-size * 0.18, -size * 0.18, size * 0.36, size * 0.36, size * 0.07);
  ctx.fill();
  ctx.fillStyle = tankColor(hue, 70, 68, elev);
  roundRect(-size * 0.1, -size * 0.1, size * 0.2, size * 0.2, size * 0.04);
  ctx.fill();
  ctx.restore();

  ctx.restore();
  ctx.globalAlpha = 1;

  // Уровень прокачки — звёздочки на башне.
  if (p.level > 1) {
    for (let i = 0; i < p.level - 1; i++) {
      const px = x + size / 2 - (p.level - 2) * 4 * zoom + i * 8 * zoom;
      const py = y - size * 0.12;
      drawStar(px, py, 3.4 * zoom, '#ffd166', '#8a5b00');
    }
  }

  // Щит.
  if (p.shielded) {
    ctx.beginPath();
    ctx.arc(x + size / 2, y + size / 2, size * 0.72, 0, Math.PI * 2);
    ctx.strokeStyle = 'rgba(88,166,255,0.85)';
    ctx.lineWidth = 2.5 * zoom;
    ctx.stroke();
    ctx.fillStyle = 'rgba(88,166,255,0.14)';
    ctx.fill();
  }

  // Полоска здоровья.
  if (!opts.self && p.hp < p.maxHp) {
    // Цель на недостижимой высоте: полоску приглушаем — это подсказка,
    // что навести настильный огонь не выйдет и нужно менять позицию.
    const reachable = flatShotHits(pred.deck, deck);
    const bw = size;
    const bx = x;
    const by = y - 8 * zoom;
    ctx.save();
    ctx.globalAlpha = reachable ? 1 : 0.35;
    ctx.fillStyle = 'rgba(0,0,0,0.6)';
    ctx.fillRect(bx, by, bw, 3.5 * zoom);
    ctx.fillStyle = p.hp > p.maxHp / 2 ? '#3fb950' : '#f85149';
    ctx.fillRect(bx, by, (bw * p.hp) / p.maxHp, 3.5 * zoom);
    ctx.restore();
  }

  // Ник.
  if (!opts.self && p.name) {
    ctx.font = `${Math.max(9, 11 * zoom)}px Consolas, monospace`;
    ctx.textAlign = 'center';
    ctx.fillStyle = 'rgba(0,0,0,0.75)';
    ctx.fillText(p.name, x + size / 2 + 1, y - 11 * zoom + 1);
    ctx.fillStyle = p.isBot ? '#f0a202' : '#e6edf3';
    ctx.fillText(p.name, x + size / 2, y - 11 * zoom);
  }

  // Значок высоты: ▲ — верхний ярус, ▼ — ямка.
  if (!opts.self && elev !== 0) {
    const mark = elev > 0 ? '▲' : '▼';
    const mx = x + size / 2 + size * 0.42;
    const my = y - 9 * zoom;
    ctx.font = `${Math.max(9, 11 * zoom)}px Consolas, monospace`;
    ctx.textAlign = 'center';
    ctx.fillStyle = 'rgba(0,0,0,0.8)';
    ctx.fillText(mark, mx + 1, my + 1);
    ctx.fillStyle = elev > 0 ? '#7ee787' : '#79c0ff';
    ctx.fillText(mark, mx, my);
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

/**
 * Рисует слой тумана войны поверх рельефа.
 *
 * Неисследованное закрыто, исследованное приглушено. Растягиваем с
 * билинейной фильтрацией — край тумана получается мягким.
 */
function drawFogOverlay() {
  if (!fog.canvas || !state.map) return;
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

/**
 * Рисует танки слоя с интерполяцией.
 *
 * @param {{a:object, b:object, f:number}} pair пара снапшотов
 * @param {number} deck слой
 */
function drawDeckTanks(pair, deck) {
  const { a, b, f } = pair;
  for (const [id, pb] of b.players) {
    if (id === state.myId) continue;
    // Чужие танки с другого слоя не видны — слой перекрывает.
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

/**
 * Главный кадр сцены.
 *
 * Порядок отрисовки задаёт высоту: сначала дно ям, потом земля, потом
 * верхний ярус. Благодаря этому мосты нависают над землёй, а ямы читаются
 * как углубления.
 */
function renderWorld(now) {
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;

  // Фон мира: тёмно-зелёная земля, а не чёрный.
  ctx.fillStyle = '#2c4526';
  ctx.fillRect(0, 0, w, h);

  const deck = pred.deck;
  const renderTime = performance.now() - INTERP_DELAY;
  const pair = interpolationPair(renderTime);

  // --- 1. Дно ям и танки в них (−1) ---
  drawPitLayer();
  if (pair && deck === LEVEL_PIT) drawDeckTanks(pair, deck);

  // --- 2. Нависающие кромки ям с внутренней тенью ---
  drawPitRim();

  // --- 3. Основная земля (0) ---
  drawGroundLayer(now);

  // Слой тумана: неисследованное закрыто, исследованное приглушено.
  drawFogOverlay();

  // Танки земли — под мостами и холмами.
  if (pair && deck === LEVEL_GROUND) drawDeckTanks(pair, deck);

  // --- 4. Тень, которую отбрасывают мосты и холмы ---
  drawUpperShadows();

  // --- 5. Настил мостов, холмы и танки на них (+1) ---
  drawUpperLayer();
  if (pair && deck === LEVEL_UPPER) drawDeckTanks(pair, deck);

  if (pair) {
    // --- Бонусы ---
    drawPickups(pair.b.pickups);
  }

  // --- Свой танк (предсказание + мягкая коррекция) ---
  // Рисуется поверх рельефа и чужих танков: игрок не должен теряться
  // ни под мостом, ни за кромкой ямы.
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
        deck: pred.deck,
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

  if (pair) {
    // --- Снаряды: свои всегда поверх рельефа и тумана ---
    drawBullets(pair.b.bullets);

    // --- Способности в полёте ---
    drawEffects(pair.b.effects);
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
 * Свой ли эффект способности.
 *
 * Владельца у эффекта в протоколе нет, поэтому «свой» определяется по
 * дальности: миномёт и авиаудар применяются игроком в пределах
 * `ABILITY_RANGE`.
 *
 * @param {object} e эффект
 * @returns {boolean}
 */
function isOwnEffect(e) {
  if (e.owner !== undefined && e.owner !== null) return e.owner === state.myId;
  const dx = e.x - (pred.x + TANK_SIZE / 2);
  const dy = e.y - (pred.y + TANK_SIZE / 2);
  return Math.hypot(dx, dy) <= ABILITY_RANGE;
}

/**
 * Рисует эффекты способностей: дуга миномёта и вызов авиаподдержки.
 *
 * Свои эффекты рисуются всегда — их не должен съедать туман войны.
 */
function drawEffects(effects) {
  if (!effects) return;
  for (const e of effects.values()) {
    const own = isOwnEffect(e);
    // Эффекты соседнего этажа не видны, кроме собственных.
    if (!own && (e.deck ?? LEVEL_GROUND) !== pred.deck) continue;
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
    // Слой игрока: ямка, земля или верхний ярус.
    const pit = pred.deck === LEVEL_PIT;
    const upper = pred.deck === LEVEL_UPPER;
    statDeck.textContent = pit
      ? tr('hud.deck.pit', 'PIT')
      : upper
        ? tr('hud.deck.upper', 'UPPER')
        : tr('hud.deck.ground', 'GROUND');
    statDeck.classList.toggle('upper', upper);
    statDeck.classList.toggle('pit', pit);
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
    updateLanguageButtons();
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

/**
 * Текущий масштаб интерфейса из CSS-переменной `--ui`.
 *
 * Нужен, чтобы холст мини-карты оставался резким: CSS увеличивает карту
 * в `--ui` раз, значит и буфер рисунка должен быть в столько же больше.
 *
 * @returns {number}
 */
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
 * В обычном режиме размер задаёт CSS-переменная `--map-size` (её меняет
 * клиент: select и клавиша K), в развёрнутом — считаем сами, чтобы карта
 * занимала весь экран и оставалась резкой на HiDPI.
 */
function resizeMinimap() {
  try {
    if (!minimapCanvas) return;
    // Размер мини-карты в CSS: этим же свойством оперирует style.css.
    document.documentElement.style.setProperty('--map-size', `${minimapSize}px`);
    if (!state.map) return;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    // В обычном режиме размер задаёт CSS, а в развёрнутом — холст сам.
    minimapCanvas.style.width = '';
    minimapCanvas.style.height = '';
    let px;
    if (minimapExpanded) {
      const availW = Math.max(240, window.innerWidth - 80);
      const availH = Math.max(240, window.innerHeight - 170);
      px = Math.floor(Math.min(availW, availH) * dpr);
    } else {
      px = Math.floor(minimapSize * uiScale() * dpr);
    }
    px = Math.max(160, Math.min(1600, px));
    if (minimapCanvas.width !== px) {
      minimapCanvas.width = px;
      minimapCanvas.height = px;
    }
  } catch (err) {
    console.warn('не удалось изменить размер карты:', err);
  }
}

/**
 * Читает сохранённый размер мини-карты.
 */
function loadMapSize() {
  try {
    const raw = localStorage.getItem(MINIMAP_SIZE_KEY);
    const num = Number(raw);
    if (Number.isFinite(num) && MINIMAP_SIZES.includes(num)) minimapSize = num;
  } catch (err) {
    // Приватный режим браузера — остаётся размер по умолчанию.
  }
}

/**
 * Сохраняет размер мини-карты в localStorage.
 */
function saveMapSize() {
  try {
    localStorage.setItem(MINIMAP_SIZE_KEY, String(minimapSize));
  } catch (err) {
    // Сохранение недоступно — на игру это не влияет.
  }
}

/**
 * Задаёт размер мини-карты и перерисовывает её.
 * @param {number} size размер в пикселях
 */
function setMapSize(size) {
  if (!MINIMAP_SIZES.includes(size)) return;
  minimapSize = size;
  saveMapSize();
  if (mapSizeSelect && mapSizeSelect.value !== String(size)) {
    mapSizeSelect.value = String(size);
  }
  resizeMinimap();
  renderMinimap();
}

/**
 * Переключает размер мини-карты по кругу: 150 → 200 → 260 → 320 → 150.
 */
function cycleMapSize() {
  const idx = MINIMAP_SIZES.indexOf(minimapSize);
  const next = MINIMAP_SIZES[(idx + 1) % MINIMAP_SIZES.length];
  setMapSize(next);
}

/**
 * Рисует иконку бонуса на мини-карте.
 *
 * Разные бонусы — разные цвета: звезда жёлтая, щит голубой,
 * ремкомплект зелёный.
 *
 * @param {number} kind вид бонуса
 * @param {number} cx
 * @param {number} cy
 * @param {number} scale пикселей на тайл
 */
function drawMinimapPickup(kind, cx, cy, scale) {
  const r = Math.max(3, 4 * scale);
  if (kind === PICKUP.STAR) {
    mctx.fillStyle = '#ffd166';
    mctx.beginPath();
    for (let i = 0; i < 10; i++) {
      const rad = i % 2 === 0 ? r : r * 0.45;
      const a = (Math.PI / 5) * i - Math.PI / 2;
      const px = cx + Math.cos(a) * rad;
      const py = cy + Math.sin(a) * rad;
      if (i === 0) mctx.moveTo(px, py);
      else mctx.lineTo(px, py);
    }
    mctx.closePath();
    mctx.fill();
  } else if (kind === PICKUP.SHIELD) {
    mctx.fillStyle = '#58a6ff';
    mctx.beginPath();
    mctx.arc(cx, cy, r, 0, Math.PI * 2);
    mctx.fill();
    mctx.strokeStyle = '#d6ecff';
    mctx.lineWidth = 1;
    mctx.stroke();
  } else {
    mctx.fillStyle = '#3fb950';
    mctx.fillRect(cx - r * 0.3, cy - r, r * 0.6, r * 2);
    mctx.fillRect(cx - r, cy - r * 0.3, r * 2, r * 0.6);
  }
}

/**
 * Рисует мини-карту.
 *
 * Показывается только активный слой игрока: на земле — земля и дыры
 * ямок, в ямке — дно ямок, наверху — холмы и мостики.
 */
function renderMinimap() {
  if (!state.map || !terrainCanvases.length) return;
  const m = state.map;
  const deck = pred.deck;
  const w = minimapCanvas.width;
  const h = minimapCanvas.height;
  if (!w || !h) return;
  mctx.clearRect(0, 0, w, h);
  const terrain = terrainCanvases[deck] ?? terrainCanvases[0];
  // Фон слоя: на верхнем ярусе пустота — пропасть, её видно как тёмное поле.
  mctx.fillStyle = deck === LEVEL_UPPER ? '#141c12' : '#05070a';
  mctx.fillRect(0, 0, w, h);
  // Рельеф карты (160×160 тайлов) растягиваем на весь холст: так карта
  // одинаково чёткая и в маленьком, и в полноэкранном виде.
  mctx.imageSmoothingEnabled = false;
  mctx.drawImage(terrain, 0, 0, w, h);

  // Поверх рельефа — туман войны: неисследованное закрыто,
  // исследованное приглушено, видимое — как есть.
  if (fog.canvas) {
    mctx.drawImage(fog.canvas, 0, 0, w, h);
  }

  // Масштаб: сколько пикселей холста приходится на один тайл.
  const scale = w / m.width;

  // Бонусы.
  const snap = state.snapshots[state.snapshots.length - 1];
  if (snap) {
    for (const p of snap.pickups.values()) {
      if ((p.deck ?? LEVEL_GROUND) !== deck) continue;
      const cx = (p.x / m.tileSize) * scale;
      const cy = (p.y / m.tileSize) * scale;
      drawMinimapPickup(p.kind, cx, cy, scale);
    }
    // Танки — только с текущего слоя.
    for (const p of snap.players.values()) {
      if ((p.deck ?? LEVEL_GROUND) !== deck) continue;
      const cx = (p.x / m.tileSize) * scale;
      const cy = (p.y / m.tileSize) * scale;
      if (p.id === state.myId) {
        // Свой танк: яркая метка со стрелкой направления корпуса.
        const r = Math.max(4, 6 * scale);
        mctx.strokeStyle = 'rgba(126,242,154,0.75)';
        mctx.lineWidth = 1;
        mctx.beginPath();
        mctx.arc(cx, cy, r, 0, Math.PI * 2);
        mctx.stroke();
        const dx = DIR_DX[p.dir] ?? 0;
        const dy = DIR_DY[p.dir] ?? 0;
        mctx.fillStyle = '#7ef29a';
        mctx.beginPath();
        mctx.moveTo(cx + dx * (r + 4), cy + dy * (r + 4));
        mctx.lineTo(cx - dy * 3.5, cy + dx * 3.5);
        mctx.lineTo(cx + dy * 3.5, cy - dx * 3.5);
        mctx.closePath();
        mctx.fill();
        mctx.fillStyle = '#f2fff4';
        mctx.fillRect(cx - 2, cy - 2, 4, 4);
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
  updateFogFade(dt);
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
  bindLanguageButtons();
  updateLanguageButtons();

  // Регуляторы громкости: восстанавливаем сохранённые значения.
  loadVolumes();
  bindVolumeControls();

  // Размер мини-карты: восстанавливаем сохранённый.
  loadMapSize();
  if (mapSizeSelect) mapSizeSelect.value = String(minimapSize);

  // Панель звука закрыта по умолчанию, иконка показывает состояние.
  setSoundPopoverOpen(false);
  updateSoundButton();

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
  // Кирпич: 4 стадии. Тёплая красная кладка.
  for (let d = 0; d < 4; d++) {
    const x = d * TS;
    g.fillStyle = TERRAIN.brick;
    g.fillRect(x, 0, TS, TS);
    g.fillStyle = '#c9693c';
    g.fillRect(x + 1, 1, TS - 2, TS - 2);
    g.strokeStyle = TERRAIN.brickDark;
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
      g.strokeStyle = 'rgba(60,26,12,0.85)';
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
      g.fillStyle = 'rgba(50,24,10,0.55)';
      g.fillRect(x + 4, 6, 9, 7);
      g.fillRect(x + 19, 18, 8, 9);
    }
    g.strokeStyle = 'rgba(60,26,12,0.45)';
    g.lineWidth = 1;
    g.strokeRect(x + 0.5, 0.5, TS - 1, TS - 1);
  }

  // Бетон: светло-серый.
  {
    const x = 4 * TS;
    g.fillStyle = TERRAIN.steel;
    g.fillRect(x, 0, TS, TS);
    g.fillStyle = '#d7dee6';
    g.fillRect(x + 1, 1, TS - 2, 7);
    g.fillStyle = TERRAIN.steelDark;
    g.fillRect(x + 1, TS - 8, TS - 2, 7);
    g.fillStyle = '#c6cfd9';
    g.fillRect(x + 2, 9, TS - 4, TS - 18);
    g.fillStyle = '#eef3f8';
    g.fillRect(x + 4, 4, 5, 4);
    g.fillRect(x + TS - 9, 4, 5, 4);
    g.strokeStyle = '#79828e';
    g.lineWidth = 1;
    g.strokeRect(x + 0.5, 0.5, TS - 1, TS - 1);
  }

  // Лес: зелёный, темнее травы, но читаемый.
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
      g.fillStyle = TERRAIN.forest;
      g.beginPath();
      g.arc(x + bx, by, r, 0, Math.PI * 2);
      g.fill();
    }
    for (const [bx, by, r] of blobs) {
      g.fillStyle = TERRAIN.forestLight;
      g.beginPath();
      g.arc(x + bx - 1.5, by - 2, r * 0.62, 0, Math.PI * 2);
      g.fill();
    }
  }

  // Лёд: светло-голубой.
  {
    const x = 6 * TS;
    g.fillStyle = TERRAIN.ice;
    g.fillRect(x, 0, TS, TS);
    g.fillStyle = '#e2f6ff';
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
    g.strokeStyle = 'rgba(110,170,205,0.7)';
    g.lineWidth = 1;
    g.strokeRect(x + 0.5, 0.5, TS - 1, TS - 1);
  }

  // Вода: 3 кадра.
  for (let f = 0; f < 3; f++) {
    const x = (7 + f) * TS;
    g.fillStyle = TERRAIN.waterDeep;
    g.fillRect(x, 0, TS, TS);
    g.fillStyle = TERRAIN.water;
    g.fillRect(x, 0, TS, 16);
    g.strokeStyle = '#8fd2ff';
    g.lineWidth = 2;
    for (let w = 0; w < 3; w++) {
      const yy = 6 + w * 10 + f * 2;
      g.beginPath();
      g.moveTo(x, yy);
      g.bezierCurveTo(x + 8, yy - 4, x + 18, yy + 4, x + 32, yy);
      g.stroke();
    }
  }

  // Пандус вверх: жёлтые шевроны показывают направление полосы.
  {
    const x = 10 * TS;
    g.fillStyle = '#7d6a2c';
    g.fillRect(x, 0, TS, TS);
    g.fillStyle = '#a89040';
    g.fillRect(x + 1, 1, TS - 2, TS - 2);
    g.strokeStyle = '#f2c94c';
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
    g.strokeStyle = 'rgba(50,40,10,0.5)';
    g.lineWidth = 1;
    g.strokeRect(x + 0.5, 0.5, TS - 1, TS - 1);
  }

  // Дно ямки: земля, грунт.
  {
    const x = 11 * TS;
    g.fillStyle = TERRAIN.pit;
    g.fillRect(x, 0, TS, TS);
    g.fillStyle = TERRAIN.pitDark;
    for (let i = 0; i < 12; i++) {
      const px = x + ((i * 13) % TS);
      const py = (i * 7) % TS;
      g.fillRect(px, py, 3, 2);
    }
    g.fillStyle = 'rgba(255,225,180,0.12)';
    g.fillRect(x, 0, TS, 4);
  }

  // Спускной пандус в ямку: земляные шевроны.
  {
    const x = 12 * TS;
    g.fillStyle = '#5b4028';
    g.fillRect(x, 0, TS, TS);
    g.strokeStyle = '#d9a05b';
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
    g.strokeStyle = 'rgba(30,18,8,0.5)';
    g.lineWidth = 1;
    g.strokeRect(x + 0.5, 0.5, TS - 1, TS - 1);
  }

  // Настил моста: доски.
  {
    const x = 13 * TS;
    g.fillStyle = TERRAIN.deck;
    g.fillRect(x, 0, TS, TS);
    g.fillStyle = TERRAIN.deckTop;
    g.fillRect(x + 1, 1, TS - 2, 4);
    g.strokeStyle = TERRAIN.deckDark;
    g.lineWidth = 1;
    for (let i = 0; i < 4; i++) {
      const y = 8 * i + 7.5;
      g.beginPath();
      g.moveTo(x, y);
      g.lineTo(x + TS, y);
      g.stroke();
    }
    g.strokeStyle = 'rgba(60,40,18,0.55)';
    g.strokeRect(x + 0.5, 0.5, TS - 1, TS - 1);
  }

  // Трава: четыре оттенка, чтобы поле не выглядело однотонным.
  for (let v = 0; v < 4; v++) {
    const x = (14 + v) * TS;
    g.fillStyle = TERRAIN.grass[v];
    g.fillRect(x, 0, TS, TS);
    // Редкие пучки травы — лёгкая фактура.
    g.strokeStyle = 'rgba(40,74,32,0.35)';
    g.lineWidth = 1;
    for (let i = 0; i < 5; i++) {
      const px = 4 + ((v * 7 + i * 11) % (TS - 8));
      const py = 5 + ((v * 5 + i * 9) % (TS - 10));
      g.beginPath();
      g.moveTo(x + px, py);
      g.lineTo(x + px + 1, py - 3);
      g.stroke();
    }
    // Пятнышки света.
    g.fillStyle = 'rgba(190,225,150,0.16)';
    g.fillRect(x + ((v * 3) % 20) + 4, 10 + ((v * 7) % 14), 5, 3);
  }
}

boot();
