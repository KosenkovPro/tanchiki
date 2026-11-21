# 🎮 TANCHIKI

> Multiplayer browser 2D tank battle in the spirit of **Battle City** — but with a huge
> open map and seamless *drop-in* play, like `.io` games.

[![Rust](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/KosenkovPro/tanchiki.svg)](https://github.com/KosenkovPro/tanchiki/releases)
[![Play](https://img.shields.io/badge/play-localhost%3A3000-2ea44f.svg)](http://localhost:3000)

| | |
|---|---|
| **Server** | Rust 2024 · `tokio` · `axum` + WebSockets · authoritative simulation at **30 TPS** |
| **Client** | Vanilla JS (ES modules) + Canvas 2D · **no bundler, no runtime dependencies** |
| **Network** | custom compact **binary protocol** (little-endian) · no JSON, no serializer crates |
| **Assets** | graphics drawn procedurally on `<canvas>` · sounds are **CC0** (see “🔊 Sounds”) |

```
┌────────────────┐   WebSocket (binary protocol)   ┌─────────────────────┐
│  Client        │◄───────────────────────────────►│  Server (axum)      │
│  Canvas 2D     │   INPUT ↑  /  SNAPSHOT ↓         │  game loop 30 Hz    │
│  prediction    │   PING   ↑  /  PONG   ↓          │  interest mgmt      │
└────────────────┘                                 └─────────────────────┘
```

---

<details name="lang">
<summary><b>🇷🇺 Русский</b> — <i>нажмите, чтобы переключить язык на русский</i></summary>

# 🎮 ТАНЧИКИ

> Многопользовательская браузерная 2D-игра с танками в духе **Battle City** — но с большой
> открытой картой и бесшовным входом в стиле `.io`-игр.

| | |
|---|---|
| **Сервер** | Rust 2024 · `tokio` · `axum` + WebSockets · авторитарная симуляция **30 TPS** |
| **Клиент** | Vanilla JS (ES-модули) + Canvas 2D · **без сборщика и внешних зависимостей** |
| **Сеть** | собственный компактный **бинарный протокол** (little-endian) · без JSON и без сериализаторов |
| **Ассеты** | графика рисуется процедурно на `<canvas>` · звуки — **CC0** (см. «🔊 Звуки») |

---

## 🚀 Быстрый старт

Нужен только Rust. Склонируйте репозиторий и запустите сервер:

```bash
git clone https://github.com/KosenkovPro/tanchiki.git
cd tanchiki
cargo run
```

Откройте <http://localhost:3000> и нажмите **«В БОЙ»**. Регистрация не нужна: придумайте
ник — и вы сразу в общем мире.

Статика вшивается в бинарник через `rust-embed`, поэтому папку `static/` держать рядом с
исполняемым файлом **не нужно**: одна и та же команда работает и в разработке, и в релизе.

> **Важно про редактирование клиента.** Файлы в `static/` вшиваются на этапе сборки.
> После правки `static/*.js` или `static/*.css` нужно пересобрать сервер
> (`cargo build`) и перезапустить его — иначе браузер получит старую версию из бинаря.

### Требования

| Компонент | Версия |
|-----------|--------|
| Rust | 1.85+ (edition 2024) |
| Node.js | не требуется |
| Docker | не требуется (но есть `Dockerfile` и `docker-compose.yml`) |

---

## 📦 Готовые бинарники

В разделе [Releases](https://github.com/KosenkovPro/tanchiki/releases) опубликованы сборки
под Windows и Linux (x86_64) — скачать, распаковать и запустить, без Rust и без сборки.

| Файл | Платформа | Запуск |
|------|-----------|--------|
| `tanchiki-windows-x86_64.exe` | Windows 10/11 x86_64 | `.\tanchiki-windows-x86_64.exe` |
| `tanchiki-linux-x86_64` | Linux x86_64 (glibc) | `chmod +x tanchiki-linux-x86_64 && ./tanchiki-linux-x86_64` |
| `SHA256SUMS.txt` | контрольные суммы | `sha256sum -c SHA256SUMS.txt` |

Проверить целостность на Windows:

```powershell
Get-FileHash .\tanchiki-windows-x86_64.exe -Algorithm SHA256
```

Сервер слушает `0.0.0.0:3000`, поэтому в браузере откройте <http://localhost:3000>.
Чтобы принимать игроков из локальной сети, узнайте свой адрес (`ipconfig` / `ip addr`)
и откройте `http://<ваш-ip>:3000` на другом устройстве.

---

## 🐳 Запуск в Docker

```bash
docker compose up -d --build
```

Откройте <http://localhost:3000>. Остановить и удалить контейнер:

```bash
docker compose down
```

Или вручную:

```bash
docker build -t tanchiki .
docker run -d --name tanchiki -p 3000:3000 tanchiki
```

Образ — `debian:bookworm-slim` с одним бинарником внутри; `HEALTHCHECK` опрашивает
`/healthz`, поэтому `docker ps` покажет состояние контейнера.

---

## 🎮 Управление

| Клавиша | Действие |
|---------|----------|
| `W` `A` `S` `D` / стрелки | **корпус** танка — в том числе по диагонали (8 направлений) |
| Мышь | **башня** — поворачивается за курсором, 64 направления (шаг 5.625°) |
| `Пробел` / ЛКМ | выстрел (по направлению башни) |
| **ПКМ** | применить выбранную способность в точке курсора |
| `1` `2` | выбор способности: миномёт / авиаподдержка |
| `Enter` | начать бой / возродиться |
| `M` | включить / выключить звук |
| Колесо мыши | зум камеры (0.55×…1.6×) |

**Корпус и башня независимы.** Корпус едет по WASD, а башня крутится за курсором мыши:
можно ехать вбок и целиться в другую сторону, а можно стоять на месте и вести огонь
вокруг себя. Снаряды летят **по башне**, а не по корпусу.

Башня смотрит в **64 направления** (шаг 5.625°) — настолько мелкий шаг нужен,
чтобы край конуса обзора не «моргал» на блоках в тумане.

Диагональ получается одновременным нажатием двух клавиш (`W`+`D`, `S`+`A` и т. д.):
сервер выводит направление из знаков осей, поэтому корпус по-прежнему имеет 8 направлений.

При повороте танк **автоматически выравнивается по сетке тайлов** — как в классическом
Battle City, поэтому он не застревает в узких проходах. Диагональный ход по сетке
не выравнивается — он свободный.

Камера **не трясётся**: ни при попаданиях, ни при взрывах.

---

## 💣 Способности

Две способности на панели «СПОСОБНОСТИ · ПКМ» (клавиши `1`–`2` или клик по кнопке).
Выбранная способность применяется **правой кнопкой мыши** в точке курсора.

| Способность | Урон | Радиус | Дальность | Кулдаун | Особенность |
|-------------|------|--------|-----------|---------|-------------|
| Миномёт (`1`) | 2 | 48 px | 1400 px | 6 с (180 тиков) | летит **вдвое медленнее пули** и перелетает стены |
| Авиаподдержка (`2`) | 3 | 66 px | 1400 px | 15 с (450 тиков) | удар с воздуха **с задержкой 2.5 с** |

Кнопки «Пушка» на панели больше нет: обычная стрельба — это **ЛКМ**, отдельная
способность для неё не нужна.

Кулдаун показывается **всегда** — сколько секунд осталось до готовности; когда
способность готова, на кнопке написано «ГОТОВ». У миномёта и авиаподдержки
кулдауны **раздельные**: они не мешают друг другу.

Скорость миномётного снаряда — **ровно вдвое ниже** скорости прямого выстрела
из пушки (коэффициент `0.5` от скорости снаряда уровня). На первом уровне пуля
летит 300 px/с, миномёт — 150 px/с. Время полёта считается **от дистанции**, а не
задано константой: чем дальше цель, тем дольше летит миномёт.

---

## 🌫 Туман войны (как в Age of Empires 2)

Мир такой же, как в AoE2: **противники видны только там, куда «смотрит» башня.**

* **Конус обзора** — 120° (полуугол 60°) вокруг направления башни, дальность **900 px**.
* **Стены перекрывают обзор**: кирпич и бетон останавливают взгляд, вода и лес — нет.
* Сервер — источник истины: танк вне конуса **вообще не попадает в снимок мира**,
  поэтому врага нельзя увидеть даже с помощью модификации клиента.
* Пройденная территория **остаётся на карте**, но в «старом» виде: стена, которую вы
  разрушили минуту назад, изображается такой, какой была, когда вы в последний раз
  смотрели в ту сторону. Неисследованные области закрыты полностью.
* На мини-карте видно ровно то же: исследованное — приглушено, видимое — как есть.
* Лес работает как в AoE2: танк внутри массива не виден врагам снаружи, пока не выстрелит.

### Почему блоки в тумане больше не «моргают»

Раньше башня смотрела в 16 направлений, край конуса прыгал на 22.5°, и блоки
на границе тумана то появлялись, то исчезали. Сейчас это устранено:

* **64 направления башни** вместо 16 — шаг 5.625°, край конуса почти гладкий.
* Видимость считается **по каждому тайлу отдельно**, а не «веером из 56 лучей»:
  тайл попадает в конус 120° и между ним и танком нет стены. Прямая видимость
  проверяется по Брезенхэму, причём на диагональных шагах дополнительно
  проверяется «угол» — стена перекрывает обзор симметрично с обеих сторон.
* Прозрачность тайлов **плавно интерполируется** (lerp) между состояниями, а не
  переключается скачком: граница конуса растворяется, а не щёлкает.
* Слой тумана рисуется **одним `ImageData`** (один пиксель на тайл) и
  растягивается с билинейной фильтрацией — край тумана мягкий.

Состояния тайла — как в Age of Empires 2:

| Состояние | Что значит | Как выглядит |
|-----------|-----------|--------------|
| `Hidden` | не исследовано | полностью чёрный |
| `Explored` | исследовано, но сейчас не в конусе | приглушено примерно на 55 %, «старый» вид тайла |
| `Visible` | сейчас в конусе обзора | чисто, без затемнения |

Сервер присылает сущности **только из конуса обзора**, а сам слой тумана
клиент рисует у себя локально — по карте мира, без лишнего трафика.

---

## 📖 Правила игры

### Типы тайлов

| Тайл | Проходим для танка | Для снаряда | Особенности |
|------|-------------------|-------------|-------------|
| `Empty` (0) | да | да | — |
| `Brick` (1) | нет | нет | 4 стадии повреждения, разрушается выстрелами |
| `Steel` (2) | нет | нет | пробивается только снарядом **3-го уровня** |
| `Water` (3) | нет | да | танк не проезжает, снаряды летят насквозь |
| `Forest` (4) | да | да | танк внутри леса **невидим** для врагов снаружи массива (пока не выстрелит) и **полупрозрачен** — сквозь него видно кусты |
| `Ice` (5) | да | да | скользко: слабое трение, танк уезжает по инерции |
| `Ramp` (6) | да | да | **трамплин** (жёлтые шевроны) — переход между ярусами, заезд меняет этаж |

### Двухуровневая карта

Карта состоит из **двух ярусов** (этажей по высоте), но вид остаётся сверху, 2D:

| Ярус | Что на нём |
|------|-------------|
| `НИЖНИЙ` (0) | земля — основная поверхность |
| `ВЕРХНИЙ` (1) | площадки и перекрытия |

* Переход между этажами — **тайл-трамплин** (`Ramp`). Он есть сразу на обоих
  ярусах, а ярус меняется **при заезде** на трамплин: пока танк стоит на нём,
  он остаётся на своём этаже.
* Стены, снаряды, бонусы, взрывы и обзор работают **в пределах своего яруса**:
  танк на другом этаже не виден и не слышен как цель.
* В панели состояния есть индикатор **«ЯРУС»** — `НИЖНИЙ` или `ВЕРХНИЙ`.
  Соседний этаж виден на карте приглушённым.
* Карта генерируется на сервере **детерминированно** (сид `0x7A4C1E55D1CE2026`):
  `160 × 160` тайлов по `32` px — это `5120 × 5120` px.

### Прокачка

| Уровень | Броня | Урон снаряда | Скорость | Стволов | Перезарядка | Снарядов одновременно |
|---------|-------|--------------|----------|---------|------------|------------------------|
| 1 | 3 | 1 | 140 | 1 | **1.65 с** | 3 |
| 2 | 4 | 2 | 151 | **2** | 1.35 с | 6 |
| 3 | 5 | 3 | 162 | 2 | 1.05 с | 6 |

Со **второго уровня** у танка появляется **вторая пушка**: стволы разведены
симметрично относительно продольной оси, поэтому сам спрайт танка остаётся
симметричным и в любом из 8 направлений.

Перезарядка базового танка **втрое медленнее** прежней (0.55 с → 1.65 с), и с каждой
собранной звездой танк стреляет всё быстрее: 1.65 с → 1.35 с → 1.05 с.

### Бонусы

| Бонус | Эффект |
|-------|--------|
| ★ Звезда | повышение уровня танка на 1 (максимум 3), полное восстановление брони |
| ● Щит | 8 секунд неуязвимости |
| ✚ Ремкомплект | мгновенное восстановление брони |

Бонусы появляются случайно на свободных тайлах и живут 30 секунд. Из уничтоженного танка
с 35 % шансом выпадает звезда.

### Очки

| Событие | Очки |
|---------|------|
| Уничтожение танка | +100 |
| Подобранный бонус | +50 |
| Разрушенный кирпич | +1 |
| Разрушенный бетон (снарядом 3-го уровня) | +2 |

### Таблица лидеров

Панель «ТОП-10 · УБИЙСТВА» в правом верхнем углу показывает **число уничтоженных танков**
главным числом, а очки — вторичным. Сортировка — по убийствам, при равенстве по очкам.

### 🔊 Звуки

Игра озвучена семью короткими эффектами: выстрел, выстрел врага, взрыв, попадание,
подбор бонуса, повышение уровня и урон. Все файлы — **CC0 1.0 Universal**
(общественное достояние): их можно свободно использовать в коммерческих проектах
без оплаты и без указания авторства.

Источник — набор
[The Essential Retro Video Game Sound Effects Collection](https://opengameart.org/content/512-sound-effects-8-bit-style)
(Juhani Junkala / SubspaceAudio) с OpenGameArt; подробности и точные имена файлов —
в [`static/sfx/CREDITS.txt`](static/sfx/CREDITS.txt).

Звук включается при первом клике (браузеры разрешают аудио только после действия
пользователя) и переключается клавишей `M` или кнопкой «🔊 ЗВУК» в углу экрана.

#### Звук мотора

Отдельный звук мотора **процедурный** — генерируется через Web Audio, без
единого звукового файла: пила и меандр проходят через низкочастотный фильтр.
Громкость и тон зависят от скорости танка, поэтому на холостых ходах танк тихо
гудит, а на разгоне мотор «ревет».

#### Противники слышны

Звук — единственный канал, который намеренно **не скрывается туманом войны**.
Сервер присылает звуковые события отдельной секцией снимка даже для источников
вне конуса обзора (слышимость — до 1800 px), поэтому можно услышать, **с какой
стороны атакуют**. Клиент проигрывает их через `PannerNode` с моделью `HRTF`:
«слушатель» стоит в точке игрока и направлен туда же, куда смотрит башня.
Звук с другого этажа слышен глуше — этаж перекрывает.

### Боты

Если на сервере меньше 20 активных игроков, сервер сам поддерживает население ИИ-танками
(до 24 ботов): поиск пути в обход препятствий, стрельба по линии видимости и **бой друг
с другом** — целью бота может быть и человек, и другой бот, поэтому на карте идут
настоящие бои. Если цели нет, бот блуждает по карте. Когда приходят люди,
лишние боты уходят плавно: исчезают со взрывом и удаляются через 1.5 секунды.

#### Уровни сложности

| Уровень | Скорость | Ошибка наводки | Перезарядка | Реакция | Дальность обзора |
|---------|----------|----------------|-------------|---------|-----------------|
| Лёгкий | **0.55×** | ±3 шага башни | ×1.8 | 1.2 с | 520 px |
| Средний | **0.72×** | ±2 шага башни | ×1.4 | 0.67 с | 700 px |
| Сложный | **0.88×** | ±1 шаг башни | ×1.15 | 0.33 с | 860 px |

Главное правило: **ни один бот не быстрее игрока** — множитель скорости всегда меньше
единицы, поэтому в скорости танка игрок всегда выигрывает. Сложность влияет ещё и на
ошибку наводки, задержку реакции, длительность перезарядки и дальность обзора: «сложный»
бот первым замечает противника, но всё равно не догоняет человека.

По умолчанию состав смешанный: 45 % лёгких, 40 % средних, 15 % сложных. Весь сервер
можно перевести на один уровень переменной окружения `TANCHIKI_BOT_SKILL`
(`easy` / `normal` / `hard` / `mixed`).

---

## ⚙️ Настройки сервера

Параметры задаются переменными окружения (см. `src/game.rs`, `GameConfig::from_env`):

| Переменная | По умолчанию | Описание |
|------------|--------------|----------|
| `TANCHIKI_BIND` | `0.0.0.0:3000` | адрес прослушивания |
| `TANCHIKI_TICK_RATE` | `30` | тиков в секунду (5…120) |
| `TANCHIKI_MAP_W` | `160` | ширина карты в тайлах (32…512) |
| `TANCHIKI_MAP_H` | `160` | высота карты в тайлах (32…512) |
| `TANCHIKI_MIN_PLAYERS` | `20` | минимальный размер населения (люди + боты) |
| `TANCHIKI_MAX_BOTS` | `24` | максимум ботов |
| `TANCHIKI_BOT_SKILL` | `mixed` | сложность ботов: `easy` / `normal` / `hard` / `mixed` |
| `TANCHIKI_SEED` | фиксированный | зерно генерации карты |
| `TANCHIKI_PROFILE` | выкл. | `1` — подробный профайлинг тиков в лог |

Размер тайла — константа `TILE_SIZE = 32` px (`src/map.rs`): геометрия карты, коллизии
и протокол опираются на неё, поэтому она не выносится в настройки.

Уровень логов задаётся переменной `RUST_LOG` (по умолчанию `info`).

Пример запуска:

```bash
TANCHIKI_BIND=0.0.0.0:8080 TANCHIKI_MAP_W=256 TANCHIKI_MAP_H=256 cargo run --release
```

В PowerShell:

```powershell
$env:TANCHIKI_BIND = '0.0.0.0:8080'
$env:TANCHIKI_MAP_W = '256'
cargo run --release
```

> **Про точность тикрейта.** По умолчанию Windows округляет любое ожидание до границы
> ~15.6 мс, из-за чего таймер на 33 мс срабатывал раз в ~46 мс — и игра теряла треть
> тиков. Поэтому сервер (1) просит у системы точность таймера 1 мс и (2) считает тики
> по накопленному времени, догоняя пропущенные (максимум 5 тиков за раз). Фактическая
> частота — ровно 30 тиков/с независимо от точности планировщика ОС.

---

## 🏗 Архитектура

```
src/
├── main.rs      HTTP + WebSocket сервер, раздача встроенной статики
├── protocol.rs  бинарный протокол: структуры, кодирование, декодирование
├── map.rs       типы тайлов, повреждаемость, генерация карты, RLE, чанки
├── game.rs      игровой цикл: физика, коллизии, стрельба, боты, лут
└── network.rs   WebSocket-подключения, рассылка снимков по зоне видимости
static/
├── index.html   разметка канваса и HUD
├── style.css    оформление
├── protocol.js  клиентская копия бинарного протокола
├── game.js      рендер, предсказание, интерполяция, HUD
└── sfx/         звуковые эффекты (CC0, mp3)
```

### Тикрейт и авторитетность

Вся физика, проверка попаданий и разрушение стен считаются **исключительно на сервере**
с фиксированным тикрейтом **30 TPS** (`tokio::time::interval`). Клиент присылает только
состояние управления и получает готовый снимок мира.

### Interest management и туман войны

Карта `160 × 160` тайлов по `32 × 32` px — это `5120 × 5120` px. Она делится на чанки
`16 × 16` тайлов (всего `10 × 10` чанков).

Снимок мира фильтруется **по конусу взгляда**, а не только по чанкам: в снимок попадают
только тела, снаряды и эффекты, что попадают в 120°-конус башни наблюдателя на расстоянии
до 900 px и не закрыты стеной. Пространственная сетка чанков используется как быстрый
отбор кандидатов (радиус 3 чанка), а окончательное решение принимает проверка конуса.
Танк в чаще дополнительно скрывается от наблюдателей, находящихся вне того же лесного
массива.

Фильтрация **по ярусам** идёт отдельным шагом: сущности с чужим `deck` в снимок
не попадают вообще — этаж перекрывает обзор. Исключение — **звуковой канал**:
звуковые события присылаются всем, кто в пределах слышимости, независимо от
конуса обзора, поэтому игрок слышит, с какой стороны атакуют.

Слой тумана войны целиком рисует **клиент**: сервер не пересылает «чёрные» тайлы,
а присылает только то, что игрок видит. Расчёт видимости по тайлам, плавное
затухание прозрачности и мягкий край тумана описаны в разделе «🌫 Туман войны».

### Синхронизация карты

* При входе игрок получает **полное состояние карты по ярусам**, упакованное **RLE**:
  для каждого яруса — своя последовательность троек `value:u8, len:u16`.
* Во время игры приходят только **дельты** `TileUpdate { x, y, value, deck }` для изменившихся
  тайлов в зоне видимости; свой ярус указывает и у дельты.

### Безопасность сети

* В сетевом коде **нет `unwrap()`/`expect()`**: битый пакет возвращается клиенту как
  `ERROR`, оборванное соединение корректно удаляет игрока, сбой одного соединения не
  влияет на игровой цикл.
* Декодирование протокола проверяет границы буфера на каждом чтении и никогда не паникует.
* Исходящая очередь ограничена: медленный клиент просто пропускает кадр, а не тормозит
  сервер.

### Сглаживание на клиенте

* **Client-side prediction** для собственного танка: клиент повторяет серверную физику
  (ускорение, трение, выравнивание по сетке) фиксированным шагом 30 Гц.
* **Мягкая коррекция:** расхождение меньше 64 px гасится плавным смещением, больше —
  жёсткий снимок позиции.
* **Линейная интерполяция** чужих танков и снарядов по буферу последних снапшотов
  с задержкой 100 мс.

---

## 📦 Бинарный протокол

Транспорт — WebSocket, сообщения — бинарные фреймы. Все целые числа **little-endian**.
Строки кодируются как `u16 len + utf8 bytes` (никнейм обрезается до 16 байт).
Каждое сообщение начинается с одного байта-типа.

### Клиент → сервер

| Тип | Код | Тело |
|-----|-----|------|
| `JOIN` | `0x01` | `nickname: string` |
| `INPUT` | `0x02` | `seq: u32`, `flags: u8`, `aim: u8`, `ability: u8`, `target_x: f32`, `target_y: f32` |
| `RESPAWN` | `0x03` | — |
| `PING` | `0x04` | `client_time: u64` |

`flags` — битовая маска: `1` вверх, `2` вниз, `4` влево, `8` вправо, `16` огонь,
`32` применить способность (одноразовый флаг).

`aim` — направление башни, `0…63` (шаг 5.625°): `0` вправо, дальше по часовой стрелке.
`ability` — `0` пушка, `1` миномёт, `2` авиаподдержка. `target_x/target_y` — мировая
точка, куда применяется способность (по ней рисуется прицел).

### Сервер → клиент

| Тип | Код | Тело |
|-----|-----|------|
| `WELCOME` | `0x11` | `version: u16`, `player_id: u32`, `tick: u64`, карта |
| `SNAPSHOT` | `0x12` | снимок мира (см. ниже) |
| `LEADERBOARD` | `0x13` | `count: u16`, записи `{ id: u32, name: string, score: u32, kills: u32, is_bot: u8 }` |
| `DEATH` | `0x14` | `score: u32`, `kills: u32`, `deaths: u32`, `survived_ms: u64` |
| `RESPAWNED` | `0x15` | `tick: u64`, `x: f32`, `y: f32` |
| `PONG` | `0x16` | `client_time: u64`, `server_time: u64` |
| `ERROR` | `0x17` | `code: u16`, `message: string` |

**Карта** (внутри `WELCOME`): `width: u32`, `height: u32`, `tile_size: u32`,
`layers: u8`, затем для каждого яруса — `rle_len: u32` и байты RLE-потока.

**`SNAPSHOT`:**

```
tick: u64
focus_x: f32, focus_y: f32          // точка, вокруг которой центрировать камеру
players: u16 × { id: u32, name: string, flags: u8, x: f32, y: f32,
                  dir: u8, aim: u8, deck: u8, level: u8, hp: u8, max_hp: u8,
                  score: u32, kills: u32, ability: u8,
                  mortar_cd: u16, airstrike_cd: u16 }
bullets: u16 × { id: u32, x: f32, y: f32, dir: u8, deck: u8, level: u8 }
pickups: u16 × { id: u32, x: f32, y: f32, kind: u8, ttl: u32, deck: u8 }
tiles:   u16 × { x: u16, y: u16, value: u8, deck: u8 }
effects: u16 × { id: u32, kind: u8, x: f32, y: f32, x2: f32, y2: f32, phase: u8, deck: u8 }
sounds:  u16 × { id: u32, x: f32, y: f32, kind: u8, owner: u32, deck: u8 }
events:  u16 × { kind: u8, ... }     // 0 взрыв, 1 разрушен тайл, 2 подобран бонус,
                                     // 3 убийство, 4 повышение уровня, 5 способность
```

Значение тайла упаковано в один байт: биты `0..=2` — базовый тип, биты `3..=5` — стадия
повреждения (0–3 для кирпича).

`dir`: `0` вверх, `1` вниз, `2` влево, `3` вправо, `4` вверх-влево, `5` вверх-вправо,
`6` вниз-влево, `7` вниз-вправо. Это направление **корпуса**.

`aim` — направление **башни**, 64 значения с шагом 5.625°; по нему летят снаряды.
`deck` — номер яруса: `0` — нижний, `1` — верхний. Он есть у всех сущностей:
танков, снарядов, бонусов, эффектов способностей, дельт тайлов и звуков.
`mortar_cd` и `airstrike_cd` — остатки кулдаунов миномёта и авиаподдержки в тиках;
присылаются **всегда**, поэтому панель показывает готовность обеих способностей.

`sounds` — звуковые события: `kind` `0` — выстрел, `1` — взрыв, `2` — попадание;
`owner` — чей это звук (`0` — бота, иначе id игрока). Это единственный канал, который
намеренно **не скрывается туманом войны**: события приходят и для источников вне
конуса обзора, поэтому игрок слышит, с какой стороны атакуют.

`effects` — визуальные эффекты: `kind = 1` миномёт в полёте (`x,y` — точка запуска,
`x2,y2` — точка падения, `phase` — ход полёта в процентах), `kind = 2` вызов
авиаподдержки (`x,y` — точка удара, `phase` — ход вызова).

События `Explosion` и `TileBroken` тоже несут `deck` — ярус, на котором они произошли.

Версия протокола — **4**: в первой было 4 направления, во второй добавились 8 направлений
и счётчик убийств, в третьей — независимая башня (16 направлений), способности и туман
войны, в четвёртой — 64 направления башни (шаг 5.625°), многоуровневая карта с ярусами
(`deck` у всех сущностей), раздельные кулдауны миномёта и авиаподдержки и звуковой
канал вне конуса обзора.

---

## 🛡 Устойчивость к ошибкам

* В сетевом коде **нет `unwrap()`/`expect()`**: битый пакет возвращается клиенту как
  `ERROR`, оборванное соединение корректно удаляет игрока, сбой одного соединения не
  влияет на игровой цикл.
* Декодирование протокола проверяет границы буфера на каждом чтении и никогда не паникует.
* Исходящая очередь ограничена: медленный клиент просто пропускает кадр, а не тормозит
  сервер.
* `panic = "abort"` намеренно **не включён** в релизном профиле — паника в одной задаче
  не должна ронять весь игровой сервер.

---

## 🧪 Разработка и тесты

```bash
cargo check --all-targets   # 0 ошибок, 0 предупреждений
cargo test                  # 41 тест
cargo build --release
```

Покрыты: RLE round-trip, связность карты, разрушаемость кирпича и бетона, разрешение
направлений, движение по диагонали, темп стрельбы по перезарядке, застревание танка
в стенах, сбор бонусов, поведение population при входе людей, корректность протокола
на битых данных, независимость башни от корпуса, скорость ботов относительно игрока,
сокрытие противников вне конуса обзора, полёт миномёта и удар авиаподдержки с задержкой,
кулдауны и дальность способностей, 64 направления башни без разрывов, трамплины на
обоих ярусах и слышимость выстрелов вне конуса обзора.

Полезные команды во время разработки:

```bash
# профайлинг фаз каждого 60-го тика
TANCHIKI_PROFILE=1 cargo run --release

# лог в файл
RUST_LOG=debug cargo run --release 2> server.log
```

---

## 🩹 Если что-то пошло не так

| Симптом | Причина и решение |
|---------|-------------------|
| Порт 3000 занят | `TANCHIKI_BIND=0.0.0.0:3001 cargo run` |
| `cargo: command not found` | Rust не установлен или не в `PATH`: см. <https://rustup.rs> |
| Игра не обновляется после правки `static/` | статика вшита в бинарник — пересоберите: `cargo build` и перезапустите |
| Соединение сразу обрывается | проверьте, что открываете `ws://<хост>:3000/ws`, а не `https://` |
| Низкий FPS в браузере | отключите аппаратное ускорение / уменьшите окно; зум колесом меняет масштаб мира |
| «too many open files» на Linux | поднимите лимит: `ulimit -n 4096` |

Полная русская версия этой документации лежит в [`README.ru.md`](README.ru.md).

---

## 📄 Лицензия

MIT — см. [LICENSE](LICENSE).

Проект открытый: Issue и Pull Request приветствуются.

</details>

<details name="lang" open>
<summary><b>🇬🇧 English</b> — <i>click to switch the language to English</i></summary>

## 🚀 Quick start

Only Rust is required. Clone the repository and start the server:

```bash
git clone https://github.com/KosenkovPro/tanchiki.git
cd tanchiki
cargo run
```

Open <http://localhost:3000> and press **“BATTLE”**. There is no registration: type a
nickname and you are instantly in the shared world.

The client assets are embedded into the binary with `rust-embed`, so you do **not** need to
keep the `static/` folder next to the executable — the very same command works in
development and in a release build.

> **Editing the client.** Files in `static/` are baked in at build time. After changing
> `static/*.js` or `static/*.css` you must rebuild the server (`cargo build`) and restart
> it — otherwise the browser keeps receiving the old version embedded in the binary.

### Requirements

| Component | Version |
|-----------|---------|
| Rust | 1.85+ (edition 2024) |
| Node.js | not required |
| Docker | not required (but a `Dockerfile` and `docker-compose.yml` are included) |

---

## 📦 Prebuilt binaries

The [Releases](https://github.com/KosenkovPro/tanchiki/releases) page publishes Windows and
Linux (x86_64) builds — download, unpack, run. No Rust, no build step.

| File | Platform | Run |
|------|----------|-----|
| `tanchiki-windows-x86_64.exe` | Windows 10/11 x86_64 | `.\tanchiki-windows-x86_64.exe` |
| `tanchiki-linux-x86_64` | Linux x86_64 (glibc) | `chmod +x tanchiki-linux-x86_64 && ./tanchiki-linux-x86_64` |
| `SHA256SUMS.txt` | checksums | `sha256sum -c SHA256SUMS.txt` |

Verify the download on Windows:

```powershell
Get-FileHash .\tanchiki-windows-x86_64.exe -Algorithm SHA256
```

The server binds `0.0.0.0:3000`, so open <http://localhost:3000> in your browser. To let
players from the same local network join, find your address (`ipconfig` / `ip addr`) and
open `http://<your-ip>:3000` on the other device.

---

## 🐳 Running with Docker

```bash
docker compose up -d --build
```

Open <http://localhost:3000>. Stop and remove the container:

```bash
docker compose down
```

Or manually:

```bash
docker build -t tanchiki .
docker run -d --name tanchiki -p 3000:3000 tanchiki
```

The runtime image is `debian:bookworm-slim` with a single binary inside; `HEALTHCHECK`
polls `/healthz`, so `docker ps` reflects the real state of the container.

---

## 🎮 Controls

| Key | Action |
|-----|--------|
| `W` `A` `S` `D` / arrow keys | **hull** movement — **including diagonals** (8 directions) |
| Mouse | **turret** — follows the cursor, 64 directions (5.625° per step) |
| `Space` / left mouse button | fire (along the turret) |
| **Right mouse button** | apply the selected ability at the cursor |
| `1` `2` | select ability: mortar / air strike |
| `Enter` | join the battle / respawn |
| `M` | toggle sound on / off |
| Mouse wheel | camera zoom (0.55×…1.6×) |

**Hull and turret are independent.** The hull drives on WASD while the turret tracks the
mouse cursor: you can strafe and aim somewhere else, or stand still and fire in any
direction. Bullets fly **along the turret**, not along the hull.

The turret looks in **64 directions** (5.625° per step) — such a fine step is
needed so that the edge of the vision cone does not make blocks in the fog
flicker.

A diagonal move is just two keys at once (`W`+`D`, `S`+`A`, …): the server derives the
hull direction from the axis signs, so the hull still has 8 directions.

When the tank turns, it is **automatically aligned to the tile grid** — exactly like in
classic Battle City — so it never gets stuck in narrow passages. A diagonal step is not
snapped to the grid; it stays free.

The camera **never shakes**: not on hits, not on explosions.

---

## 💣 Abilities

Two modes on the “СПОСОБНОСТИ · ПКМ / ABILITIES · RMB” panel (keys `1`–`2` or a click
on the button). The selected ability is applied with the **right mouse button** at the
cursor position.

| Ability | Damage | Radius | Range | Cooldown | Notes |
|---------|--------|--------|-------|----------|-------|
| Mortar (`1`) | 2 | 48 px | 1400 px | 6 s (180 ticks) | flies **twice slower than a bullet** and clears walls |
| Air strike (`2`) | 3 | 66 px | 1400 px | 15 s (450 ticks) | hit from the sky **after a 2.5 s delay** |

The “Gun” button is gone from the panel: plain shooting is the **left mouse
button**, it never needed a separate ability.

The cooldown is **always shown** — how many seconds are left before the ability
is ready; when it is ready, the button reads “ГОТОВ”. Mortar and air strike have
**separate** cooldowns: they never block each other.

The mortar shell flies **exactly twice slower** than a direct gun shot (a factor
of `0.5` of the level bullet speed). On level 1 a bullet travels at 300 px/s and
the mortar at 150 px/s. The flight time is computed **from the distance**, not
from a constant: the farther the target, the longer the mortar flies.

---

## 🌫 Fog of war (Age of Empires 2 style)

The world works like AoE2: **enemies are only visible where the turret looks.**

* **Vision cone** — 120° (60° half-angle) around the turret direction, range **900 px**.
* **Walls block sight**: brick and steel stop the gaze, water and forest do not.
* The server is the authority: a tank outside the cone **never enters the world
  snapshot**, so an enemy cannot be seen even by tampering with the client.
* Explored territory **stays on the map**, but in its “old” look: a wall you destroyed a
  minute ago is drawn as it was when you last looked that way. Unexplored areas are
  covered completely.
* The minimap shows exactly the same: explored is dimmed, visible is bright.
* Forest works like in AoE2: a tank inside a forest block is hidden from enemies outside
  it until it fires.

### Why blocks in the fog no longer flicker

The turret used to look in 16 directions, so the edge of the cone jumped by 22.5°
and blocks on the fog boundary appeared and vanished. That is fixed now:

* **64 turret directions** instead of 16 — a 5.625° step, so the edge of the cone
  is almost smooth.
* Visibility is computed **per tile** instead of a “fan of 56 rays”: a tile has
  to fall into the 120° cone and no wall may stand between it and the tank. The
  line of sight is traced with Bresenham, and on diagonal steps the “corner” is
  checked too — a wall blocks the view symmetrically on both sides.
* Tile transparency is **interpolated smoothly** (lerp) between states instead of
  being switched in one step, so the cone edge dissolves rather than snaps.
* The fog layer is drawn as **a single `ImageData`** (one pixel per tile) and
  stretched with bilinear filtering — the fog edge is soft.

Tile states are the ones from Age of Empires 2:

| State | Meaning | How it looks |
|-------|---------|--------------|
| `Hidden` | never explored | completely black |
| `Explored` | explored, but not in the cone right now | dimmed by roughly 55 %, the “old” look of the tile |
| `Visible` | inside the vision cone right now | clean, no dimming |

The server sends entities **only from the vision cone**, while the fog layer
itself is drawn locally by the client — from the map data, with no extra traffic.

---

## 📖 Game rules

### Tile types

| Tile | Driveable by tanks | By bullets | Notes |
|------|--------------------|------------|-------|
| `Empty` (0) | yes | yes | — |
| `Brick` (1) | no | no | 4 damage stages, destroyed by fire |
| `Steel` (2) | no | no | only a **level 3** bullet can punch through |
| `Water` (3) | no | yes | tanks cannot enter, bullets fly over |
| `Forest` (4) | yes | yes | a tank inside the forest is **invisible** to enemies outside that forest block (until it fires) and **semi-transparent** — the bushes show through it |
| `Ice` (5) | yes | yes | slippery: low friction, the tank coasts by inertia |
| `Ramp` (6) | yes | yes | **ramp** (yellow chevrons) — a transition between decks, driving onto it switches the floor |

### Two-level map

The map has **two decks** (floors by height), but the view is still top-down 2D:

| Deck | What is on it |
|------|---------------|
| `LOWER` (0) | the ground — the main surface |
| `UPPER` (1) | platforms and overhangs |

* The transition between floors is a **ramp tile** (`Ramp`). It exists on both decks
  at once, and the deck changes **when you drive onto it**: while standing on a ramp
  the tank stays on its floor.
* Walls, bullets, pickups, explosions and sight all work **within their own deck**:
  a tank on another floor is neither visible nor audible as a target.
* The status panel has a **«ЯРУС» (deck)** indicator — «НИЖНИЙ» (lower) or
  «ВЕРХНИЙ» (upper). The neighbouring floor is visible on the map dimmed.
* The map is generated on the server **deterministically** (seed `0x7A4C1E55D1CE2026`):
  `160 × 160` tiles of `32` px — that is `5120 × 5120` px.

### Tank levels

| Level | Armor | Bullet damage | Speed | Guns | Reload | Bullets at once |
|-------|-------|--------------|-------|------|--------|-----------------|
| 1 | 3 | 1 | 140 | 1 | **1.65 s** | 3 |
| 2 | 4 | 2 | 151 | **2** | 1.35 s | 6 |
| 3 | 5 | 3 | 162 | 2 | 1.05 s | 6 |

From **level 2** the tank gets a **second gun**: the barrels are spread symmetrically
around the longitudinal axis, so the tank sprite itself stays symmetric in all 8
directions.

The base tank reloads **three times slower** than before (0.55 s → 1.65 s), and every
collected star makes it shoot faster: 1.65 s → 1.35 s → 1.05 s.

### Pickups

| Pickup | Effect |
|--------|--------|
| ★ Star | tank level +1 (max 3), armor fully restored |
| ● Shield | 8 seconds of invulnerability |
| ✚ Repair kit | instantly restores armor |

Pickups spawn randomly on free tiles and live for 30 seconds. A destroyed tank drops a star
with a 35 % chance.

### Scoring

| Event | Points |
|-------|--------|
| Tank destroyed | +100 |
| Pickup collected | +50 |
| Brick destroyed | +1 |
| Steel destroyed (by a level 3 bullet) | +2 |

### Leaderboard

The “TOP-10 · KILLS” panel in the top-right corner shows the **number of destroyed tanks**
as the main number, with points as the secondary value. The order is by kills, then by points.

### 🔊 Sounds

The game is voiced with seven short effects: shot, enemy shot, explosion, hit, pickup,
level up and damage. Every file is **CC0 1.0 Universal** (public domain): they can be used
in commercial projects for free — no payment and no attribution required.

Source — [The Essential Retro Video Game Sound Effects Collection](https://opengameart.org/content/512-sound-effects-8-bit-style)
(Juhani Junkala / SubspaceAudio) on OpenGameArt; details and the exact file names are in
[`static/sfx/CREDITS.txt`](static/sfx/CREDITS.txt).

Audio starts on the first click (browsers only allow sound after a user gesture) and can be
toggled with the `M` key or the “🔊 SOUND” button in the corner of the screen.

#### Engine sound

The engine sound is **procedural** — generated with Web Audio, without a single sound
file: a sawtooth and a square wave go through a lowpass filter. Volume and pitch depend
on the tank speed, so an idling tank hums quietly and the engine roars on a run.

#### Enemies are audible

Sound is the only channel that is deliberately **not hidden by the fog of war**. The
server sends sound events in a separate snapshot section even for sources outside the
vision cone (hearable up to 1800 px), so you can hear **which side the attack comes
from**. The client plays them through a `PannerNode` with the `HRTF` model: the
“listener” stands at the player position and is oriented the same way the turret
looks. A sound from another floor is duller — the floor muffles it.

### Bots

If the server has fewer than 20 active players, it maintains the population with AI tanks
(up to 24 bots): pathfinding around obstacles, shooting along the line of sight and
**fighting each other** — a bot may target a human or another bot, so real battles happen
all over the map. With no target around, a bot wanders around the map. When real players
join, the extra bots leave gracefully: they explode and are removed 1.5 seconds later.

#### Difficulty levels

| Level | Speed | Aim error | Reload | Reaction | Sight range |
|-------|-------|-----------|--------|----------|-------------|
| Easy | **0.55×** | ±3 turret steps | ×1.8 | 1.2 s | 520 px |
| Normal | **0.72×** | ±2 turret steps | ×1.4 | 0.67 s | 700 px |
| Hard | **0.88×** | ±1 turret step | ×1.15 | 0.33 s | 860 px |

The main rule: **no bot is ever faster than the player** — the speed multiplier is always
below 1, so the player always wins the race. Difficulty also changes aim error, reaction
delay, reload time and sight range: a “hard” bot spots you first, but still cannot
outrun you.

By default the mix is 45 % easy, 40 % normal, 15 % hard. The whole server can be switched
to a single level with the `TANCHIKI_BOT_SKILL` environment variable
(`easy` / `normal` / `hard` / `mixed`).

---

## ⚙️ Server configuration

Everything is configured through environment variables (see `src/game.rs`,
`GameConfig::from_env`):

| Variable | Default | Description |
|----------|---------|-------------|
| `TANCHIKI_BIND` | `0.0.0.0:3000` | listening address |
| `TANCHIKI_TICK_RATE` | `30` | ticks per second (5…120) |
| `TANCHIKI_MAP_W` | `160` | map width in tiles (32…512) |
| `TANCHIKI_MAP_H` | `160` | map height in tiles (32…512) |
| `TANCHIKI_MIN_PLAYERS` | `20` | minimum population (humans + bots) |
| `TANCHIKI_MAX_BOTS` | `24` | maximum number of bots |
| `TANCHIKI_BOT_SKILL` | `mixed` | bot difficulty: `easy` / `normal` / `hard` / `mixed` |
| `TANCHIKI_SEED` | fixed | map generation seed |
| `TANCHIKI_PROFILE` | off | `1` — detailed per-tick profiling in the log |

The tile size is a compile-time constant `TILE_SIZE = 32` px (`src/map.rs`): map geometry,
collisions and the protocol are all built on top of it, so it is deliberately not
configurable.

Log verbosity is controlled with `RUST_LOG` (`info` by default).

Example:

```bash
TANCHIKI_BIND=0.0.0.0:8080 TANCHIKI_MAP_W=256 TANCHIKI_MAP_H=256 cargo run --release
```

PowerShell:

```powershell
$env:TANCHIKI_BIND = '0.0.0.0:8080'
$env:TANCHIKI_MAP_W = '256'
cargo run --release
```

> **About tick-rate accuracy.** By default Windows rounds every wait to a ~15.6 ms
> boundary, so a 33 ms timer fired roughly every 46 ms and the game lost a third of its
> ticks. The server therefore (1) asks the OS for a 1 ms timer resolution and (2) counts
> ticks from accumulated elapsed time, catching up on missed ticks (up to 5 per wake-up).
> The effective rate is a steady 30 ticks/second regardless of the OS scheduler.

---

## 🏗 Architecture

```
src/
├── main.rs      HTTP + WebSocket server, serves the embedded client assets
├── protocol.rs  binary protocol: structures, encoding, decoding
├── map.rs       tile types, damageability, map generation, RLE, chunks
├── game.rs      game loop: physics, collisions, shooting, bots, loot
└── network.rs   WebSocket connections, per-player snapshot broadcast
static/
├── index.html   canvas and HUD markup
├── style.css    styling
├── protocol.js  client-side copy of the binary protocol
├── game.js      rendering, prediction, interpolation, HUD
└── sfx/         sound effects (CC0, mp3)
```

### Tick rate and authority

All physics, hit detection and wall destruction are computed **exclusively on the server**
at a fixed tick rate of **30 TPS** (`tokio::time::interval`). The client only sends input
state and receives finished world snapshots.

### Interest management and fog of war

The map is `160 × 160` tiles of `32 × 32` px — that is `5120 × 5120` px. It is split into
chunks of `16 × 16` tiles (`10 × 10` chunks in total).

The world snapshot is filtered **by the vision cone**, not by chunks alone: only bodies,
bullets and effects that fall inside the viewer's 120° cone within 900 px and are not
blocked by a wall are included. The chunk grid is used as a fast candidate filter (radius
3 chunks), and the cone check makes the final decision. A tank hidden in the forest
additionally disappears from observers that are outside the same forest block.

Filtering **by deck** is a separate step: entities with a foreign `deck` never enter
the snapshot at all — the floor blocks the view. The exception is the **sound
channel**: sound events are sent to everyone within hearing range, regardless of the
vision cone, so the player hears which side the attack comes from.

The fog layer itself is drawn entirely by the **client**: the server does not send
“black” tiles, it sends only what the player can see. The per-tile visibility
computation, the smooth fade of transparency and the soft fog edge are described in
the “🌫 Fog of war” section.

### Map synchronization

* On join, a player receives the **full map state per deck**, packed with **RLE**:
  for every deck its own sequence of `value:u8, len:u16` triples.
* During the game only **deltas** arrive: `TileUpdate { x, y, value, deck }` for changed
  tiles inside the visibility area; a delta carries its deck too.

### Network safety

* There is **no `unwrap()`/`expect()` in the network code**: a malformed packet is returned
  to the client as an `ERROR`, a dropped connection removes the player cleanly, and one
  broken connection never affects the game loop.
* Protocol decoding checks buffer bounds on every read and never panics.
* The outgoing queue is bounded: a slow client simply skips a frame instead of stalling
  the server.

### Client-side smoothing

* **Client-side prediction** for your own tank: the client replays the server physics
  (acceleration, friction, grid alignment) at a fixed 30 Hz step.
* **Soft correction:** a divergence smaller than 64 px is smoothed out, a larger one is
  snapped to the authoritative position.
* **Linear interpolation** of other tanks and bullets from a buffer of the latest snapshots
  with a 100 ms delay.

---

## 📦 Binary protocol

Transport is WebSocket, messages are binary frames. All integers are **little-endian**.
Strings are encoded as `u16 len + utf8 bytes` (the nickname is trimmed to 16 bytes).
Every message starts with a single type byte.

### Client → server

| Type | Code | Body |
|------|------|------|
| `JOIN` | `0x01` | `nickname: string` |
| `INPUT` | `0x02` | `seq: u32`, `flags: u8`, `aim: u8`, `ability: u8`, `target_x: f32`, `target_y: f32` |
| `RESPAWN` | `0x03` | — |
| `PING` | `0x04` | `client_time: u64` |

`flags` is a bit mask: `1` up, `2` down, `4` left, `8` right, `16` fire,
`32` apply ability (a one-shot flag).

`aim` is the turret direction, `0…63` (5.625° per step): `0` is right, then clockwise.
`ability` is `0` gun, `1` mortar, `2` air strike. `target_x/target_y` is the world point
where the ability is applied (the crosshair is drawn there).

### Server → client

| Type | Code | Body |
|------|------|------|
| `WELCOME` | `0x11` | `version: u16`, `player_id: u32`, `tick: u64`, map |
| `SNAPSHOT` | `0x12` | world snapshot (see below) |
| `LEADERBOARD` | `0x13` | `count: u16`, entries `{ id: u32, name: string, score: u32, kills: u32, is_bot: u8 }` |
| `DEATH` | `0x14` | `score: u32`, `kills: u32`, `deaths: u32`, `survived_ms: u64` |
| `RESPAWNED` | `0x15` | `tick: u64`, `x: f32`, `y: f32` |
| `PONG` | `0x16` | `client_time: u64`, `server_time: u64` |
| `ERROR` | `0x17` | `code: u16`, `message: string` |

**Map** (inside `WELCOME`): `width: u32`, `height: u32`, `tile_size: u32`,
`layers: u8`, then for every deck — `rle_len: u32` and the bytes of the RLE stream.

**`SNAPSHOT`:**

```
tick: u64
focus_x: f32, focus_y: f32          // the point the camera should centre on
players: u16 × { id: u32, name: string, flags: u8, x: f32, y: f32,
                  dir: u8, aim: u8, deck: u8, level: u8, hp: u8, max_hp: u8,
                  score: u32, kills: u32, ability: u8,
                  mortar_cd: u16, airstrike_cd: u16 }
bullets: u16 × { id: u32, x: f32, y: f32, dir: u8, deck: u8, level: u8 }
pickups: u16 × { id: u32, x: f32, y: f32, kind: u8, ttl: u32, deck: u8 }
tiles:   u16 × { x: u16, y: u16, value: u8, deck: u8 }
effects: u16 × { id: u32, kind: u8, x: f32, y: f32, x2: f32, y2: f32, phase: u8, deck: u8 }
sounds:  u16 × { id: u32, x: f32, y: f32, kind: u8, owner: u32, deck: u8 }
events:  u16 × { kind: u8, ... }     // 0 explosion, 1 tile destroyed, 2 pickup taken,
                                     // 3 kill, 4 level up, 5 ability used
```

A tile value is packed into a single byte: bits `0..=2` hold the base type, bits `3..=5`
hold the damage stage (0–3 for brick).

`dir`: `0` up, `1` down, `2` left, `3` right, `4` up-left, `5` up-right, `6` down-left,
`7` down-right. That is the **hull** direction.

`aim` is the **turret** direction, 64 values with a 5.625° step; bullets fly along it.
`deck` is the deck number: `0` — lower, `1` — upper. Every entity has one: tanks,
bullets, pickups, ability effects, tile deltas and sounds.
`mortar_cd` and `airstrike_cd` are the remaining cooldowns of the mortar and the air
strike in ticks; they are **always** sent, so the panel can show the readiness of
both abilities.

`sounds` are sound events: `kind` `0` — shot, `1` — explosion, `2` — hit; `owner` is
whose sound it is (`0` — a bot, otherwise a player id). This is the only channel that is
deliberately **not hidden by the fog of war**: events arrive even for sources outside
the vision cone, so the player hears which side the attack comes from.

`effects` holds visual effects: `kind = 1` a mortar in flight (`x,y` is the launch point,
`x2,y2` the impact point, `phase` the flight progress in percent), `kind = 2` an air strike
call (`x,y` is the impact point, `phase` the call progress).

The `Explosion` and `TileBroken` events carry a `deck` as well — the deck they happened
on.

The protocol version is **4**: version 1 had 4 directions, version 2 added 8 directions
and a kill counter, version 3 added the independent turret (16 directions), abilities and
fog of war, version 4 added 64 turret directions (5.625° per step), the multi-level map
with decks (`deck` on every entity), separate cooldowns for the mortar and the air strike
and the sound channel outside the vision cone.

---

## 🛡 Fault tolerance

* There is **no `unwrap()`/`expect()` in the network code**: a malformed packet is returned
  to the client as an `ERROR`, a dropped connection removes the player cleanly, and one
  broken connection never affects the game loop.
* Protocol decoding checks buffer bounds on every read and never panics.
* The outgoing queue is bounded: a slow client simply skips a frame instead of stalling
  the server.
* `panic = "abort"` is deliberately **not** enabled in the release profile — a panic in one
  task must not take down the whole game server.

---

## 🧪 Development and tests

```bash
cargo check --all-targets   # 0 errors, 0 warnings
cargo test                  # 41 tests
cargo build --release
```

Covered: RLE round-trip, map connectivity, brick and steel damageability, direction
resolution, diagonal movement, the fire cadence following the reload timer, tanks getting
stuck in walls, pickup collection, population behaviour when humans join, protocol
robustness against corrupted input, the turret being independent of the hull, bot speed
relative to the player, hiding enemies outside the vision cone, the mortar flight and the
delayed air strike, ability cooldowns and range limits, the 64 turret directions without
gaps, ramps on both decks and shots being heard outside the vision cone.

Useful commands while developing:

```bash
# per-phase profiling of every 60th tick
TANCHIKI_PROFILE=1 cargo run --release

# log to a file
RUST_LOG=debug cargo run --release 2> server.log
```

---

## 🩹 Troubleshooting

| Symptom | Cause and fix |
|---------|---------------|
| Port 3000 is busy | `TANCHIKI_BIND=0.0.0.0:3001 cargo run` |
| `cargo: command not found` | Rust is not installed or not in `PATH`: see <https://rustup.rs> |
| The game does not update after editing `static/` | assets are embedded in the binary — rebuild with `cargo build` and restart |
| The connection drops immediately | make sure you open `ws://<host>:3000/ws` and not `https://` |
| Low FPS in the browser | disable hardware acceleration / shrink the window; the mouse wheel changes the world zoom |
| “too many open files” on Linux | raise the limit: `ulimit -n 4096` |

A standalone Russian copy of this documentation lives in [`README.ru.md`](README.ru.md).

---

## 📄 License

MIT — see [LICENSE](LICENSE).

The project is open source: issues and pull requests are welcome.

</details>
