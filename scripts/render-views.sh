#!/usr/bin/env sh
# Renders the standard views (app/tools/render_views.gd: fixed shots on the fixture routes) into
# screenshots/views/ with software Vulkan, to compare a visual change before and after (ADR 0011).
# Runs inside the dev container: scripts/dev.sh scripts/render-views.sh
# VIEWS="village-chase lake-drone" renders only some; OUT_DIR changes the folder, QUALITY the
# graphics preset (medium by default). GPX=<file> with SHOTS="name:distance:camera ..." renders
# shots of any route instead (e.g. one from an issue), camera 0 chase, 1 first person, 2 drone,
# with :right,up,back a camera standing aside (see app/tools/render_views.gd).
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${OUT_DIR:-$root/screenshots/views}"
mkdir -p "$out"
"$root/scripts/build-gdext.sh" debug >/dev/null
godot --headless --path "$root/app" --import >/dev/null 2>&1 || true
# A script that does not compile leaves Godot waiting for the world forever: find out now.
errors="$(godot --headless --path "$root/app" --quit-after 2 2>&1 \
    | grep -A3 "SCRIPT ERROR\|SHADER ERROR" || true)"
if [ -n "$errors" ]; then
    echo "$errors" >&2
    exit 1
fi
status=0
# One Godot run per route: each loads its route and rides it to the views on it.
routes="gurtenstrasse bielersee kirchenfeldbruecke oberalp"
if [ -n "${GPX:-}" ]; then
    routes="$(realpath "$GPX")"
fi
for route in $routes; do
    # A script error leaves Godot waiting for the world forever, hence the timeout.
    output="$(ROUTE="$route" OUT_DIR="$out" timeout 1800 xvfb-run -a -s "-screen 0 1600x900x24" \
        godot --path "$root/app" --rendering-driver vulkan --resolution 1280x720 \
        -s res://tools/render_views.gd 2>&1)" || status=1
    echo "$output" | grep -E "^saved" || true
    if echo "$output" | grep -q "SCRIPT ERROR\|SHADER ERROR"; then
        echo "$output" | grep -A3 "SCRIPT ERROR\|SHADER ERROR" >&2
        status=1
    fi
done
exit "$status"
