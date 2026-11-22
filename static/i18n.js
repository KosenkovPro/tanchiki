/**
 * ТАНЧИКИ — переводы интерфейса (RU / EN).
 *
 * Модуль хранит словари, переключает язык по кнопке и применяет переводы
 * к DOM-элементам через атрибуты `data-i18n*`.
 *
 * Как это устроено:
 *  - разметка помечает элементы ключами словаря: `data-i18n="hud.score"`;
 *  - `applyI18n()` обходит такие элементы и подставляет перевод;
 *  - выбранный язык запоминается в localStorage под ключом `tanchiki.lang`.
 *
 * Никаких зависимостей и сетевых запросов — обычный ES-модуль.
 */

/** Поддерживаемые языки. */
export const LANGUAGES = ['ru', 'en'];

/** Язык по умолчанию, если в localStorage ничего нет или он неизвестен. */
export const DEFAULT_LANGUAGE = 'en';

/** Ключ в localStorage, где хранится выбранный язык. */
const STORAGE_KEY = 'tanchiki.lang';

// ---------------------------------------------------------------------------
// Словари
// ---------------------------------------------------------------------------

/** Русский интерфейс. */
const RU = Object.freeze({
  // --- HUD: мини-карта ---
  'hud.map': 'КАРТА',
  'hud.expand': 'РАЗВЕРНУТЬ',
  'hud.collapse': 'СВЕРНУТЬ',
  'hud.legend.you': 'вы',
  'hud.legend.enemy': 'враг',
  'hud.legend.bot': 'бот',
  'hud.legend.pickup': 'бонус',

  // --- HUD: таблица лидеров ---
  'hud.top': 'ТОП-10 · УБИЙСТВА',
  'hud.leaderLegend': '☠ убийств · ★ очки',

  // --- HUD: состояние игрока ---
  'hud.level': 'УРОВЕНЬ',
  'hud.armor': 'БРОНЯ',
  'hud.score': 'ОЧКИ',
  'hud.kills': 'УБИЙСТВ',
  'hud.ping': 'ПИНГ',
  'hud.fps': 'FPS',
  'hud.deck': 'ЯРУС',
  'hud.deck.lower': 'НИЖНИЙ',
  'hud.deck.upper': 'ВЕРХНИЙ',
  // --- Три уровня высот ---
  'hud.deck.pit': 'ЯМКА',
  'hud.deck.ground': 'ЗЕМЛЯ',

  // --- HUD: способности ---
  'hud.abilities': 'СПОСОБНОСТИ · ПКМ',
  'ability.mortar.name': 'Миномёт',
  'ability.mortar.desc': 'через стены · урон 2',
  'ability.airstrike.name': 'Авиаподдержка',
  'ability.airstrike.desc': 'удар с воздуха · урон 3',
  'ability.ready': 'ГОТОВ',
  'ability.hint.mortar': 'Миномёт: ПКМ по точке — перелетает стены · 1 / 2 — выбор',
  'ability.hint.airstrike': 'Авиаподдержка: ПКМ по точке — удар с воздуха · 1 / 2 — выбор',

  // --- HUD: звук и настройки ---
  'hud.soundOn': '🔊 ЗВУК',
  'hud.soundOff': '🔇 ЗВУК ВЫКЛ',
  'hud.soundToggle': 'Звук',
  'hud.mute': 'БЕЗ ЗВУКА',
  'hud.unmute': 'ВКЛЮЧИТЬ ЗВУК',
  'hud.mapSize': 'Размер карты',
  'settings.title': 'НАСТРОЙКИ',
  'settings.master': 'Общая громкость',
  'settings.shots': 'Выстрелы',
  'settings.explosions': 'Взрывы',
  'settings.engine': 'Мотор',
  'settings.hits': 'Попадания и интерфейс',
  'settings.language': 'Язык',
  'settings.hint': 'Настройки сохраняются в браузере',

  // --- HUD: подсказка по управлению ---
  'hud.help':
    '<b>WASD</b> / <b>стрелки</b> — корпус (в т.ч. по диагонали) · ' +
    '<b>мышь</b> — башня · <b>Пробел</b> / <b>ЛКМ</b> — огонь · ' +
    '<b>ПКМ</b> — способность · <b>1</b>–<b>2</b> — выбор · ' +
    '<b>F</b> — во весь экран · <b>K</b> — размер карты · <b>M</b> — звук',

  // --- Оверлей входа ---
  'welcome.title': 'ТАНЧИКИ',
  'welcome.tagline': 'Битва на открытой карте 5120×5120. Бесшовный вход — просто вбей ник и в бой.',
  'welcome.nickname': 'Никнейм',
  'welcome.button': 'В БОЙ',
  'welcome.hint':
    'Корпус танка едет на WASD или стрелках (в т.ч. по диагонали), а башня крутится за курсором ' +
    'мыши — можно ехать вбок и целиться в другую сторону. ЛКМ или Пробел — выстрел, ПКМ — ' +
    'выбранная способность (миномёт перелетает стены, авиаподдержка бьёт с воздуха), клавиши 1–2 ' +
    'переключают способность. Противники видны только там, куда смотрит башня: пройденная территория ' +
    'остаётся на карте, но в «старом» виде. Карта трёхуровневая: земля, ямки-окопы ниже и холмы с ' +
    'мостиками выше. Заезжаешь по жёлтому пандусу — меняешь высоту; из ямки настильный огонь не ' +
    'достаёт, попасть можно только миномётом или авиаударом, зато с холма видно землю внизу. ' +
    'Текущая высота показана в панели слева («ЯРУС»). Звезда повышает уровень танка: со второго уровня ' +
    'у него две пушки и выше скорострельность, щит даёт неуязвимость, ремкомплект чинит броню. ' +
    'Выстрелы врагов слышны даже когда их не видно — по звуку понятно, с какой стороны атакуют. ' +
    'Клавиша F — развернуть карту на весь экран, K — размер карты, M — звук.',

  // --- Оверлей смерти ---
  'death.title': 'ТАНК УНИСЧТОЖЕН',
  'death.score': 'Очки',
  'death.kills': 'Убийств',
  'death.deaths': 'Смертей',
  'death.time': 'Продержался',
  'death.button': 'В БОЙ',

  // --- Сообщения ---
  'msg.connectionLost': 'Соединение потеряно…',
});

/** Английский интерфейс. */
const EN = Object.freeze({
  // --- HUD: minimap ---
  'hud.map': 'MAP',
  'hud.expand': 'EXPAND',
  'hud.collapse': 'COLLAPSE',
  'hud.legend.you': 'you',
  'hud.legend.enemy': 'enemy',
  'hud.legend.bot': 'bot',
  'hud.legend.pickup': 'pickup',

  // --- HUD: leaderboard ---
  'hud.top': 'TOP-10 · KILLS',
  'hud.leaderLegend': '☠ kills · ★ points',

  // --- HUD: player state ---
  'hud.level': 'LEVEL',
  'hud.armor': 'ARMOR',
  'hud.score': 'SCORE',
  'hud.kills': 'KILLS',
  'hud.ping': 'PING',
  'hud.fps': 'FPS',
  'hud.deck': 'DECK',
  'hud.deck.lower': 'LOWER',
  'hud.deck.upper': 'UPPER',
  // --- Three elevation levels ---
  'hud.deck.pit': 'PIT',
  'hud.deck.ground': 'GROUND',

  // --- HUD: abilities ---
  'hud.abilities': 'ABILITIES · RMB',
  'ability.mortar.name': 'Mortar',
  'ability.mortar.desc': 'over walls · dmg 2',
  'ability.airstrike.name': 'Air strike',
  'ability.airstrike.desc': 'strike from air · dmg 3',
  'ability.ready': 'READY',
  'ability.hint.mortar': 'Mortar: RMB on a point — flies over walls · 1 / 2 to switch',
  'ability.hint.airstrike': 'Air strike: RMB on a point — strike from above · 1 / 2 to switch',

  // --- HUD: sound and settings ---
  'hud.soundOn': '🔊 SOUND',
  'hud.soundOff': '🔇 SOUND OFF',
  'hud.soundToggle': 'Sound',
  'hud.mute': 'MUTE',
  'hud.unmute': 'UNMUTE',
  'hud.mapSize': 'Map size',
  'settings.title': 'SETTINGS',
  'settings.master': 'Master volume',
  'settings.shots': 'Shots',
  'settings.explosions': 'Explosions',
  'settings.engine': 'Engine',
  'settings.hits': 'Hits and interface',
  'settings.language': 'Language',
  'settings.hint': 'Settings are saved in your browser',

  // --- HUD: controls hint ---
  'hud.help':
    '<b>WASD</b> / <b>arrows</b> — body (incl. diagonals) · ' +
    '<b>mouse</b> — turret · <b>Space</b> / <b>LMB</b> — fire · ' +
    '<b>RMB</b> — ability · <b>1</b>–<b>2</b> — select · ' +
    '<b>F</b> — fullscreen map · <b>K</b> — map size · <b>M</b> — sound',

  // --- Welcome overlay ---
  'welcome.title': 'TANCHEKI',
  'welcome.tagline': 'Battle on a 5120×5120 open map. Drop in — just type a nickname and fight.',
  'welcome.nickname': 'Nickname',
  'welcome.button': 'PLAY',
  'welcome.hint':
    'The tank body drives on WASD or the arrow keys (including diagonals), while the turret follows ' +
    'your mouse cursor — you can strafe sideways and aim in another direction. LMB or Space fires, ' +
    'RMB uses the selected ability (the mortar flies over walls, the air strike hits from above), ' +
    'keys 1–2 switch the ability. Enemies are visible only where your turret is looking: the ground ' +
    'you have passed stays on the map, but in its “old” state. The map has three levels: the ground, ' +
    'trenches below it and hills with bridges above it. Drive onto a yellow ramp to change your ' +
    'elevation; from a trench flat fire cannot reach you — only the mortar or the air strike can, ' +
    'but from a hill you can shoot down at the ground. Your current elevation is shown in the panel ' +
    'on the left (“DECK”). A star raises the tank level: from the second level it has two guns and ' +
    'a higher rate of fire, a shield grants invulnerability, and a repair kit fixes the armor. ' +
    'Enemy shots are audible even when the enemy is not visible — the sound tells you which side ' +
    'the attack comes from. Press F for the fullscreen map, K for the map size, M for the sound.',

  // --- Death overlay ---
  'death.title': 'TANK DESTROYED',
  'death.score': 'Score',
  'death.kills': 'Kills',
  'death.deaths': 'Deaths',
  'death.time': 'Survived',
  'death.button': 'PLAY',

  // --- Messages ---
  'msg.connectionLost': 'Connection lost…',
});

/** Словари по языкам: `DICTIONARIES[lang][key]`. */
const DICTIONARIES = { ru: RU, en: EN };

// ---------------------------------------------------------------------------
// Состояние
// ---------------------------------------------------------------------------

/** Текущий язык: 'ru' или 'en'. */
let currentLanguage = DEFAULT_LANGUAGE;

/** Кнопки переключения, к которым уже привязан обработчик. */
const boundToggles = new WeakSet();

// ---------------------------------------------------------------------------
// Вспомогательные функции
// ---------------------------------------------------------------------------

/**
 * localStorage может быть недоступен (приватный режим, запрет cookies),
 * поэтому обращаемся к нему осторожно.
 * @returns {Storage|null}
 */
function getStorage() {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/**
 * Читает сохранённый язык; неизвестное значение заменяется языком по умолчанию.
 * @returns {string}
 */
function readStoredLanguage() {
  try {
    const raw = getStorage()?.getItem(STORAGE_KEY);
    return LANGUAGES.includes(raw) ? raw : DEFAULT_LANGUAGE;
  } catch {
    return DEFAULT_LANGUAGE;
  }
}

/**
 * Сохраняет язык в localStorage. Ошибки хранилища игнорируются:
 * язык всё равно применяется, просто не запоминается.
 * @param {string} lang
 */
function writeStoredLanguage(lang) {
  try {
    getStorage()?.setItem(STORAGE_KEY, lang);
  } catch {
    // Хранилище недоступно — молча продолжаем.
  }
}

/**
 * Проставляет язык документа (`<html lang="ru">`).
 */
function updateDocumentLang() {
  if (typeof document === 'undefined') return;
  const root = document.documentElement;
  if (root) root.lang = currentLanguage;
}

/**
 * Нормализует язык: неизвестное значение заменяется языком по умолчанию.
 * @param {string} lang
 * @returns {string}
 */
function normalizeLanguage(lang) {
  return LANGUAGES.includes(lang) ? lang : DEFAULT_LANGUAGE;
}

// ---------------------------------------------------------------------------
// Публичный API
// ---------------------------------------------------------------------------

/**
 * Текущий язык: 'ru' | 'en'.
 * @returns {string}
 */
export function getLanguage() {
  return currentLanguage;
}

/**
 * Переключает язык, применяет переводы к DOM и сохраняет выбор в localStorage.
 * @param {string} lang язык: 'ru' | 'en'
 * @returns {string} применённый язык
 */
export function setLanguage(lang) {
  currentLanguage = normalizeLanguage(lang);
  writeStoredLanguage(currentLanguage);
  updateDocumentLang();
  if (typeof document !== 'undefined') applyI18n(document);
  return currentLanguage;
}

/**
 * Переключает ru <-> en и возвращает новый язык.
 * @returns {string}
 */
export function toggleLanguage() {
  const next = currentLanguage === 'ru' ? 'en' : 'ru';
  setLanguage(next);
  return next;
}

/**
 * Перевод по ключу. Если ключа нет — возвращается сам ключ,
 * чтобы не ломать интерфейс.
 * @param {string} key
 * @param {Record<string, string|number>} [vars] подстановка {имя}
 * @returns {string}
 */
export function t(key, vars) {
  const name = typeof key === 'string' ? key : String(key ?? '');
  const dict = DICTIONARIES[currentLanguage] ?? RU;
  let text = Object.prototype.hasOwnProperty.call(dict, name) ? dict[name] : name;
  if (vars) {
    for (const [varName, value] of Object.entries(vars)) {
      text = text.split(`{${varName}}`).join(String(value));
    }
  }
  return text;
}

/**
 * Применяет переводы к элементам внутри root (по умолчанию document).
 *
 * Ищет:
 *   [data-i18n]            -> element.textContent = t(value)
 *   [data-i18n-html]       -> element.innerHTML = t(value)   (для текста с <b> и т.п.)
 *   [data-i18n-title]      -> element.title = t(value)
 *   [data-i18n-placeholder] -> element.placeholder = t(value)
 *   [data-i18n-aria]       -> element.setAttribute('aria-label', t(value))
 *
 * @param {ParentNode} [root]
 */
export function applyI18n(root) {
  const scope = root ?? (typeof document !== 'undefined' ? document : null);
  if (!scope || typeof scope.querySelectorAll !== 'function') return;

  for (const el of scope.querySelectorAll('[data-i18n]')) {
    const key = el.getAttribute('data-i18n');
    if (!key) continue;
    el.textContent = t(key);
  }

  for (const el of scope.querySelectorAll('[data-i18n-html]')) {
    const key = el.getAttribute('data-i18n-html');
    if (!key) continue;
    el.innerHTML = t(key);
  }

  for (const el of scope.querySelectorAll('[data-i18n-title]')) {
    const key = el.getAttribute('data-i18n-title');
    if (!key) continue;
    el.title = t(key);
  }

  for (const el of scope.querySelectorAll('[data-i18n-placeholder]')) {
    const key = el.getAttribute('data-i18n-placeholder');
    if (!key) continue;
    el.placeholder = t(key);
  }

  for (const el of scope.querySelectorAll('[data-i18n-aria]')) {
    const key = el.getAttribute('data-i18n-aria');
    if (!key) continue;
    el.setAttribute('aria-label', t(key));
  }

  // Кнопка переключения языка показывает название ДРУГОГО языка,
  // поэтому её текст ставим последним — перевод его не должен затирать.
  for (const el of scope.querySelectorAll('[data-lang-toggle]')) {
    el.textContent = currentLanguage === 'ru' ? 'EN' : 'RU';
  }

  // Переключатель EN / RU: активный язык подсвечивается.
  for (const el of scope.querySelectorAll('[data-lang-set]')) {
    const lang = el.getAttribute('data-lang-set');
    const active = lang === currentLanguage;
    el.classList.toggle('is-active', active);
    el.setAttribute('aria-pressed', active ? 'true' : 'false');
  }
}

/**
 * Вешает обработчик клика на кнопки выбора языка `[data-lang-set="en|ru"]`.
 * @param {ParentNode} root
 */
function bindLanguageSetButtons(root) {
  if (!root || typeof root.querySelectorAll !== 'function') return;
  for (const el of root.querySelectorAll('[data-lang-set]')) {
    if (boundToggles.has(el)) continue;
    boundToggles.add(el);
    el.addEventListener('click', (event) => {
      event.preventDefault();
      const lang = el.getAttribute('data-lang-set');
      if (lang) setLanguage(lang);
    });
  }
}

/**
 * Вешает обработчик клика на кнопки переключения языка.
 * @param {ParentNode} root
 */
function bindLanguageToggles(root) {
  if (!root || typeof root.querySelectorAll !== 'function') return;
  for (const el of root.querySelectorAll('[data-lang-toggle]')) {
    if (boundToggles.has(el)) continue;
    boundToggles.add(el);
    el.addEventListener('click', (event) => {
      event.preventDefault();
      toggleLanguage();
    });
  }
}

/**
 * Инициализация: читает язык из localStorage (fallback — DEFAULT_LANGUAGE),
 * применяет переводы и вешает обработчик на элементы с [data-lang-toggle].
 * @returns {string} применённый язык
 */
export function initI18n() {
  currentLanguage = readStoredLanguage();
  updateDocumentLang();
  if (typeof document !== 'undefined') {
    applyI18n(document);
    bindLanguageToggles(document);
    bindLanguageSetButtons(document);
  }
  return currentLanguage;
}
