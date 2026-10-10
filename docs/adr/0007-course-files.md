# ADR 0007 — Course files (`.tqc`)

- Status: accepted; amended 2026-10-03 (format 1 bundles inputs only, see below)
- Date: 2026-10-02

## Context

Preparing a course downloads terrain and map data and builds a 3D world. Today the downloads
live in OS cache folders (which may be purged, hold far more than one course needs and cannot be
shared) and the world is rebuilt each time. Riders want to keep, pick and share prepared courses
offline (R32–R35).

## Decision

A course is a **zip container** with the extension **`.tqc`**:

```
manifest.json      format version, generator version, name, length, climbing, max grade,
                   created, source, attribution (OSM ODbL, terrain CC BY)
route.gpx          the original track
route.json         processed route points (position, elevation, distance, surface)
terrain.bin        corridor terrain heights (compressed)
map.json           OpenStreetMap features within the corridor only
world/             pre-built world: road, water, terrain chunks, buildings, trees (binary)
preview.png        image for the course list
```

- The **pre-built world is stored** so a course starts instantly on any machine. The inputs are
  stored too, so a course can be rebuilt when the world generator improves: the manifest's
  generator version tells whether the stored world is current.
- Files are written atomically and versioned; unknown newer format versions are rejected with a
  clear message rather than misread.
- The **library** is the `courses/` folder of the data directory; any `.tqc` placed there is
  listed. Import copies a file into it; export is a plain file copy.
- **Video courses** use the same format with sync data and a reference (name, size, hash) to the
  video file next to the course, not the video itself. Preparing a course puts the course's own
  copy of the video there (a hard link on one file system), so the course outlives the file it
  was imported from (#165).
- Sharing is file-based (mail, Nextcloud, USB, websites); no online catalog for now.

## Consequences

- Course files are larger than inputs alone: roughly 4–6 MB for a 2.4 km climb with a village,
  potentially a few hundred MB for long routes. Mesh quantisation can shrink this later.
- Attribution travels with the data, satisfying ODbL and CC BY when courses are shared.
- New dependency: `zip` (MIT) with only the pure-Rust deflate backend.

## Amendment 2026-10-03 — format 1

The first implemented format bundles the **inputs** and no pre-built world:

```
manifest.json      format, generator, name, length, climbing, max grade, created, attribution
route.gpx          the original track
data/…             every terrain tile and map tile the course was built from,
                   under its path relative to the download cache
```

Data providers record which cached files they read or wrote while a course is prepared; saving
packs exactly those. Opening a course puts them back into the cache (existing files are kept;
cache paths are versioned) and builds the course offline. This needs no own format for terrain,
map or meshes, and every course is always built by the current generator (R35).

Measured on the 7 km Lake Biel route: 6.2 MB, opened on an empty machine offline in ~23 s
(debug build). Storing the pre-built world for an instant start remains a later step
(format 2); readers reject formats newer than they know.
