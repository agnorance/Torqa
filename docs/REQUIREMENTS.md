# Torqa — Requirements

Outcome of the initial requirements-elicitation session (2026-10-02, ~70 questions), extended
2026-10-03 with course previews, start page and realistic graphics (R36–R47) and in-ride
settings, ride names, per-rider HUD layouts, UI quality and direct HUD placement (R48–R54),
plus overlay mode and constant-power / heart-rate workouts (R55–R56), refined 2026-10-06: an
overlay over normal windows only, a Workouts tab and Di2 shifting (R57–R59).
Requirement IDs (`R<n>`) are referenced from code, tests and ADRs. The list is **append-only**:
existing requirements are never reworded or renumbered; changes are new requirements that state
what they supersede, and the old entry only gets a short pointer.

## Vision

Torqa is a modern, **offline-first**, open-source indoor cycling app. Import a GPX route and ride it
on a smart trainer (primary: Wahoo KICKR Core 2) through a generated semi-realistic 3D world,
a synced ride video, or street-level imagery.

## General

| ID | Requirement |
|---|---|
| R1 | macOS (Apple Silicon M1+) first; Windows and Linux later. Stack must be cross-platform from day one. |
| R2 | Audience: the author + friends. Open source under **GPL-3.0**. **Zero budget**: free/open data only; users bring their own API keys for paid services. |
| R3 | Offline-first: a prepared ride works without internet. Online extras (DEM correction, world data download, Street View, uploads) are used when connected. |
| R4 | Extensibility is a core principle: interface-driven plugins for route importers, devices, shift inputs, uploaders, ride modes and workout parsers. |

## Devices

| ID | Requirement |
|---|---|
| R5 | Bluetooth LE **FTMS** (MVP). **ANT+ FE-C** later (no ANT+ dongle available for testing yet). |
| R6 | Direct-drive trainers first (KICKR Core 2). Architecture generic enough for any FTMS/FE-C trainer and smart bikes. |
| R7 | Sensors: BLE heart-rate strap. Zwift Click as shift input (no official protocol → reverse-engineered BLE, isolated and optional); also OpenBikeControl and keyboard/gamepad as shift inputs. *(Shift inputs: R59.)* |
| R8 | Trainer modes: **SIM** (slope simulation, MVP), **ERG** (target power), **resistance level**; free ride. |
| R9 | Drivetrain: real cassette (trainer handles shifting naturally) and Zwift Cog (app-side **virtual gears** — resistance offset via FTMS). Post-MVP. See ADR 0003. |
| R10 | Software **fake trainer** for development and automated tests. |

## Routes

| ID | Requirement |
|---|---|
| R11 | Import **GPX** (MVP); importer interface allows TCX/FIT/KML later. Routes are used as-is (no route editor). |
| R12 | Elevation: when online, correct with a terrain model (Mapterhorn, AWS Terrain Tiles fallback — ADR 0005) and smooth; offline, use cached tiles or GPX elevation with smoothing. |

## Physics

| ID | Requirement |
|---|---|
| R13 | Full physics model: rider + bike mass, CdA, Crr, gradient, air density; optional wind and drafting. |
| R14 | Trainer difficulty adjustable 0–100 % (scales the felt grade; virtual speed uses the real grade). |
| R15 | Descent behaviour configurable per ride: simulated coasting vs. clamp to flat. |

## Ride modes

| ID | Requirement |
|---|---|
| R16 | **3D world (MVP)**: semi-realistic style *(now stylized, R44)*. World source user-selectable, default **hybrid** (real DEM + OpenStreetMap when available, procedural fallback). Weather/time-of-day presets. Cameras: first person, chase, drone — user's choice. Optional cadence-synced avatar. 60 fps on an M1 integrated GPU. *(Performance superseded by R43; visual target extended by R44–R47.)* |
| R17 | **Video**: own GoPro/Insta360 footage, downloaded videos, and plain videos without GPS, and Tacx RLV courses (`.rlv` + `.pgmf`, added by #42). Auto-sync from embedded GPS (GoPro GPMF) with manual sync-point fallback. 1080p target; higher resolutions transcoded down on import. Variable playback speed with frame blending. |
| R18 | **Street imagery**: Google Street View (online only, user's API key, no caching per Google ToS) and Mapillary. Smooth crossfade/zoom transitions between panoramas. |
| R19 | Overlays: 2D minimap (OpenStreetMap tiles, cached for offline) and elevation profile with current position. |
| R20 | Ghosts / pacers, selectable per ride: own previous best, fixed W/kg or power pacer, ghost from a GPX/FIT activity. |

## Training & UX

| ID | Requirement |
|---|---|
| R21 | Structured workouts later, but architected for now: ZWO, ERG/MRC, FIT workouts, built-in editor. No multi-week training plans. *(Constant-power and heart-rate workouts: R56.)* |
| R22 | Rider profile: weight, FTP, max HR, power and HR zones, built-in FTP test. **Multiple user profiles** per installation. |
| R23 | Fully customizable HUD widgets: power (instant/3 s/10 s), cadence, HR, speed, distance, time, elevation gain, current & upcoming gradient, W/kg, NP, TSS, kJ, zone. *(Per-rider layout and editor: R51.)* |
| R24 | Metric and imperial units. English UI first, i18n-ready from day one. |
| R25 | Single window / fullscreen, external monitor / TV. Phone companion app is a possible future extension. *(Overlay mode: R55.)* |
| R26 | Audio, selectable per ride: ambient sounds, original video audio, music app control. |
| R27 | Auto-detected climbs with personal KOM-style times; personal records per route. Multiplayer possibly later — keep simulation state separable from rendering. |

## Data

| ID | Requirement |
|---|---|
| R28 | Export activities as **FIT** (MVP). |
| R29 | Uploaders behind a common interface: Strava, intervals.icu, Garmin Connect, TrainingPeaks, Komoot. (Garmin/TrainingPeaks/Komoot require partner approval → best effort.) |
| R30 | Local storage in a **configurable data directory** that may live in a synced folder (e.g. Nextcloud) → sync-safe design, see ADR 0002. |
| R31 | Detailed ride history and analysis: power/HR/cadence charts, zones, PRs. |

## Courses

| ID | Requirement |
|---|---|
| R32 | **Course files** (`.tqc`): a prepared course is saved as one self-contained file that can be stored, copied, shared (mail, Nextcloud, USB, websites) and imported again, fully offline. See ADR 0007. |
| R33 | A course contains the original GPX, the processed route (elevations, bridges/tunnels), the corridor's terrain heights and OpenStreetMap features, the **pre-built 3D world**, a preview image and a manifest (format and generator versions, name, stats, attribution). |
| R34 | **Course library**: a `courses/` folder in the data directory (may be synced, R30). The app lists its courses to pick from, imports `.tqc` files into it and saves newly prepared routes as courses. |
| R35 | Courses built by an older generator still ride instantly from their stored world; they can be rebuilt from the stored inputs with a newer generator. Video courses (R17) use the same format and reference their video file instead of embedding it. |
| R36 | **Course screenshots**: every course carries a small screenshot gallery stored in the `.tqc`. 3D courses render it automatically from the generated world when the course is built (e.g. start, highest point, scenic spots); video courses take frames from the video at fixed distances. The user can pick which image is the **cover**. |
| R37 | **Path card**: every course also has a route image: the GPX path in the logo blue **`#2EB0FF`** with start/finish markers, drawn on the course's own map (the ride's flat map of its surroundings, kept in the course file as a picture when the course is prepared, #192), or on black for a course without a place. The elevation profile is shown on the course detail page only (review of #28). |

Sharing is file-based for now; a built-in online catalog may follow later.

## App structure & ride flow

| ID | Requirement |
|---|---|
| R38 | A real **start page** separate from the ride view, with a top tab bar: **Courses**, **History**, **Profile**, **Devices & Settings**. The ride view only shows the ride. *(Workouts tab: R58.)* |
| R39 | **Courses tab**: gallery overview of the course library (R34) — each card shows the cover or path card, distance, elevation gain, a small map, the course type (3D / video) and other key stats. Importing a GPX / preparing a new course starts here. |
| R40 | **Course detail page** (opened from a card): screenshot gallery, path card, map, elevation profile, stats, personal records on the course, and the per-ride options (difficulty, descent mode, weather, time of day, camera, ghost) → **Ride**. |
| R41 | The last-used trainer and sensors **reconnect automatically** in the background at app start; the user is only prompted if that fails when a ride starts. |
| R42 | At the end of a ride a **summary screen** (stats, charts, PRs, save/discard, upload) is shown, then the app returns to the start page. |
| R48 | **In-ride settings dialog**: the ride view's controls (today's bottom buttons: camera, HUD customization) move into one settings dialog opened from a single button or key. It is shared: the same dialog and settings (camera, HUD, difficulty, descent mode, weather, time of day, …) are used from the course detail page (R40), so options are set the same way before and during a ride. |
| R49 | The in-ride dialog also ends the ride: **finish and save** (→ summary, R42) or **abort without saving**, the latter after a confirmation. |
| R50 | **Ride names**: every ride in the history has a name, defaulting to the course name and date. It can be set on the summary screen (R42) and renamed later in the history; it is used as the activity name in FIT export and uploads. Files on disk keep stable identifiers, so renaming is sync-safe (ADR 0002). |
| R51 | **HUD per rider**: each rider profile has its own HUD layout (R23), editable in the rider's profile settings. The editor shows a separate HUD preview, and widgets are added, removed and **reordered by drag and drop**. It is the same editor as the HUD section of the in-ride settings dialog (R48), so changes made during a ride are saved to the rider's layout. *(Placement refined by R54.)* |
| R52 | **Modern, coherent UI** in a *sleek minimal* style: slim typography, thin translucent panels, generous spacing, few controls visible at once; during a ride the 3D view dominates. All screens and dialogs share one design system (type scale, spacing, colours, corner radii, control styles); no unstyled stock widgets. Course summaries (course cards, detail page, the loaded-course panel) are designed as polished cards with a clear hierarchy of name, path card (R37), key stats and elevation profile. |
| R53 | **Stable, responsive layouts**: loading a course or other content never changes font sizes or lets the layout grow or jump. Resizable windows and dialogs (e.g. rider settings) reflow and resize their inputs with the window instead of leaving them fixed in the middle. Layouts work from 1280×720 up to 4K / TV (R25). |
| R54 | Refines R51: HUD widgets are arranged **directly in the HUD preview**, not only in a list. A widget is dragged from the widget list straight to the position where it should appear, moved around within the HUD, and dragged out to remove it; a drop-position indicator shows where it will land. |

## Graphics

| ID | Requirement |
|---|---|
| R43 | Supersedes the R16 performance target. **Quality presets** Low / Medium / High / Ultra. An M1 (base) integrated GPU holds **60 fps on Medium**; High/Ultra target stronger Apple GPUs and, later, discrete GPUs on Windows/Linux. |
| R44 | **Visual target** *(revised 2026-10-05, [ADR 0011](adr/0011-stylized-look.md): stylized instead of realistic)*: a **stylized, faceted low-poly** world and riders in a **soft pastel palette** — a bright, toy-like diorama of the real place (real terrain, roads and buildings), not a photograph. No racing UI or racing accessories (banners, arches, crowds). |
| R45 | Look priorities, all four: **vegetation** (faceted trees and bushes, grass and flowers moving in the wind); **terrain & road** (faceted ground coloured by land cover, clean roads and tracks, verges); **lighting & atmosphere** (warm soft sun, soft shadows, ambient occlusion, pastel gradient sky, low-poly clouds, haze toward distant terrain); **rider & bike**. |
| R46 | **Rider**: stylized, faceted female or male rider (user's choice) of natural proportions (not toy-like, ADR 0011) with natural, cadence-driven pedaling and body motion on a stylized bike. Customization (bikes, kits, …) may follow; the rider and bike are kept modular for it. |
| R47 | **Assets** must stay redistributable as open source with the GPL-3.0 project: own work, CC0, CC-BY or CC-BY-SA, credited in a credits file. Models are produced by documented Blender Python scripts (riders scripted from scratch and from CC0 stylized bases); scripts and exported models are both committed. See ADR 0009 and ADR 0011. |

## Workouts & overlay

| ID | Requirement |
|---|---|
| R55 | **Overlay mode**: Torqa can be started as an overlay only — a transparent, always-on-top HUD window over any other app, so a video can be streamed full-screen while the rider's HUD (R51) shows on top. It also stays visible over full-screen apps. The overlay can be moved and resized anywhere on screen, and its position and size are remembered. Only workouts can be selected in the overlay (no courses, no 3D world). An overlay session is recorded as a ride (history, FIT). macOS first; Windows and Linux later (on Wayland only where the compositor allows always-on-top). *(Refined by R57: no full-screen apps; entered from any ride.)* |
| R56 | **Constant-power and heart-rate workouts** as additional workout modes (alongside structured workouts, R21): **constant power** (ERG at a target, e.g. 200 W) and **heart-rate hold** — the target is a zone (e.g. Zone 3, aiming at the middle of the zone) or a specific bpm, and the target power is adjusted continuously from the measured heart rate. The controller stays within user-set minimum and maximum power and ramps gently, since heart rate lags power by 30–60 s. |
| R57 | Refines R55 (2026-10-06): the overlay stays on top of **normal (windowed) apps only**; showing it over full-screen apps would need native macOS window code (a new dependency and unsafe code) and is not done. The overlay is entered with a button during **any ride or workout** (a course ride goes on following its gradient, its world hidden) or by ticking **"start as overlay"** when starting a workout. |
| R58 | Extends R38 (2026-10-06): a **Workouts** tab on the start page, right after Courses. Workouts (R56, later R21) are started there, either on their own (a flat road, the HUD only) or **on a course in the 3D world** (the trainer holds the workout's power while the world goes by). |
| R59 | Supersedes R7's shift inputs (2026-10-06): the virtual gears are shifted from the **keyboard** and from **Shimano Di2** directly over Bluetooth — the buttons the rider assigns to D-Fly channels in E-TUBE shift up and down. Each channel's press, hold and double press can be given an action instead (#139): shift one or two gears, next camera, overlay on/off, or the music (play/pause, next, previous). No dependency on paid bridge apps (BikeControl needs a subscription on macOS and Android), so no OpenBikeControl-through-BikeControl; the Zwift Click (encrypted, the v2 unlocked daily in Zwift) is not supported. |

## MVP definition

Import a GPX → generate the 3D world → ride it on the KICKR Core 2 in SIM mode → save a FIT file.

## Test hardware available

KICKR Core 2, BLE heart-rate strap, Zwift Click + Cog. No ANT+ dongle.

## Open points

- Trademark / GitHub org availability check for "Torqa" before publishing.
- Logo: an orca riding a bike.
- License compatibility of BikeControl before reusing any Zwift Click protocol knowledge from it.
