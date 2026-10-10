---
name: torqa-render-review
description: How to see Torqa's 3D world and models without a GPU — render screenshots in the dev container with software Vulkan (lavapipe), at a chosen place on a route and from a chosen camera, then judge and share them. Use whenever a change affects what the world, roads, terrain, buildings, vegetation, riders, sky or lighting look like, before declaring it done.
---

# Rendering Torqa for review

Godot renders in the dev container with **lavapipe** (software Vulkan) on a virtual display.
It is correct but slow: a town scene can take minutes per frame burst, so take few, chosen
shots, run them in the background, and read the PNGs with the Read tool. Output goes to
`screenshots/` (gitignored).

## Ready-made

- `scripts/dev.sh scripts/render-views.sh` — the **standard views** (`app/tools/render_views.gd`):
  fixed shots on the fixture routes (village, climb, hairpin, lake, rain, evening, bridge) into
  `screenshots/views/`. Render them before a visual change (`OUT_DIR=…/views-before`, pass it inside
  `sh -c '…'`) and after, and compare pairs; `VIEWS="village-chase lake-drone"` limits it.

- `scripts/dev.sh scripts/screenshots.sh` — the standard ride screenshots (`app/tests/
  screenshots.gd`).
- `scripts/dev.sh scripts/render-models.sh` — every model of `app/assets/models/buildings` as
  the world draws it, three variants and a close-up each, into `screenshots/models/`;
  `MODELS="house_gable_2_m chalet_2_m"` limits it (pass it inside `sh -c '…'` so it reaches the
  container).

## A view of a place on a route

Copy a template from `templates/` into `app/tests/` (they are scratch scripts: **never commit
them**; delete them and their `.uid` files afterwards), then run a small runner script with
`scripts/dev.sh sh <runner>`:

- `templates/tmp_road.gd` — loads `core/fixtures/$ROUTE.gpx`, starts a simulated ride, jumps to
  `$DIST` metres, uses camera `$CAM` (0 chase, 1 first person, 2 drone), saves `$OUT`.
- `templates/tmp_buildings.gd` — the same, then a free camera `$HEIGHT` up, `$BACK` behind,
  `$SIDE` aside, looking `$AHEAD` and `$LOOK_SIDE`; with `NEAR_MODEL=any` it aims at the model
  instance nearest the rider from eye height instead (good for street-level detail).

Runner pattern (one `godot` per shot; each loads the route and builds the world):

```sh
set -e
cd /workspaces/torqa
scripts/build-gdext.sh debug >/dev/null
godot --headless --path app --import >/dev/null 2>&1 || true
shot() {
  name=$1; shift
  env "$@" OUT=$PWD/screenshots/q/$name.png timeout 1500 \
    xvfb-run -a -s "-screen 0 1600x900x24" godot --path app --rendering-driver vulkan \
    --resolution 1280x720 -s res://tests/tmp_road.gd 2>&1 | grep -E "ERROR|SCRIPT" | head -5
  echo "done $name"
}
shot hairpin ROUTE=gurtenstrasse DIST=1240 CAM=2
```

Fixtures: `gurtenstrasse` (Wabern village, a hillside climb with a hairpin), `kirchenfeldbruecke`
(Bern, a bridge, blocks and halls), `bielersee` (lakeside road, vineyards, rock cuttings), and
`oberalp`, the reference route (#167): 33.6 km of a planner's file from Andermatt over the
Oberalp pass to Disentis, with hairpins, three road tunnels, the rack railway alongside, a lake
the planner drew the road through, the summit tunnel and two villages. Render its views after
any change to routes, roads, tunnels or railways; `torqa-cli route core/fixtures/oberalp.gpx`
prints its figures, and `cargo test -p torqa-app -- --ignored reference_route` checks them
against the cached tiles. Map and terrain tiles are cached in the container's volumes after
the first run.

## Pitfalls

- A script error makes Godot wait forever for `world_ready`: always use `timeout`, and grep the
  output for `SCRIPT ERROR` / `SHADER ERROR` — a scene that fails to compile renders nothing.
- Warnings are errors in this project: untyped `Variant` arguments fail to parse; give values a
  typed local first.
- Free-camera shots can end up inside a hillside; prefer `NEAR_MODEL` or the drone camera.
- Very dense scenes are slow under lavapipe; that says little about a real GPU. For frame rate,
  ask the user to check on their M1.

## Sharing results

Look at every image before reporting. For a PR, convert a few to JPEG on the host
(`sips -s format jpeg -s formatOptions 78 in.png --out docs/images/<topic>/name.jpg`, about
150 KB each), commit them, and reference them in the PR body by the commit that adds them:
`https://raw.githubusercontent.com/bossm8/Torqa/<commit>/docs/images/<topic>/name.jpg`. A
commit link keeps working after the branch is deleted and after docs refreshes remove images no
doc uses (#79); a branch link does not.
