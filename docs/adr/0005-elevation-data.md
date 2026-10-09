# ADR 0005 — Elevation data source

- Status: accepted
- Date: 2026-10-02

## Context

Route elevations from GPX files are often noisy or missing (R12), and the 3D world needs terrain
heights (R16). The source must be free, need no account, allow local caching for offline rides
(R3), and preferably be European.

## Decision

- Primary: **[Mapterhorn](https://mapterhorn.com)** terrain tiles
  (`https://tiles.mapterhorn.com/{z}/{x}/{y}.webp`, 512 px, Terrarium encoding, zoom 12).
  Copernicus GLO-30 worldwide plus high-resolution national LIDAR across much of Europe.
  Data CC BY 4.0 — attribution required (see <https://mapterhorn.com/attribution>).
- Where high-resolution national data exists (e.g. swissALTI3D in Switzerland), Mapterhorn
  also serves zoom 15 (~1.6 m per pixel); it is tried first, falling back to zoom 12 where
  tiles are missing (added 2026-10-03).
- Fallback: **AWS Open Data Terrain Tiles** (256 px Terrarium PNG, zoom 13), per tile, when
  Mapterhorn is unreachable or lacks a tile.
- Tiles are cached on disk under the user's cache directory and sent with a descriptive
  User-Agent; the cache makes prepared routes rideable offline.
- If no terrain data is available (offline, uncached), the GPX elevations are used.

## Rationale

- A comparison against swissALTI3D measured Mapterhorn at about 1.8 m RMSE versus 17–27 m for
  AWS Terrarium and plain Copernicus. Our own spot check at the Zytglogge in Bern (≈542 m) gave
  540.6 m (Mapterhorn) and 548.4 m (AWS).
- Copernicus' own portal (CDSE) requires an account and OAuth tokens, unsuitable for an app.
- Same Terrarium encoding for both sources keeps one decoder.

## Consequences

- Torqa must display terrain attribution wherever heights are used (CLI docs, app credits).
- Terrain models are bare-earth: bridges and tunnels show up as dips and humps. Planned fix:
  use OpenStreetMap bridge/tunnel tags during world generation (Phase 3).
- A 30 m terrain cell straddles the cliff above a road on a ledge, or the roof of a gallery
  the map does not know, and the profile climbs a wall no road has (#172). Along the road the
  model is checked twice (`torqa-routes`): where it departs from the file's own elevations by
  more than 30 m beyond their offset, the file wins there; and a rise steeper than 18 % that
  falls as steeply back to its level within 400 m is cut straight across. A climb that goes on
  is kept, however steep.
- Mapterhorn is served via Cloudflare; no usage policy is published, so caching aggressively is
  also a courtesy.
