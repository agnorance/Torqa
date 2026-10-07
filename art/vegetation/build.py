"""Builds the vegetation models — trees, a bush and rocks — and exports them for Godot
(ADR 0009, ADR 0011).

Run in the art container:
    scripts/art.sh blender --background --factory-startup --python art/vegetation/build.py

The look is faceted low-poly (skill torqa-look): conifers as stacked cones, broadleaf crowns as
a few chunky facets, rocks as rough blocks. Each model is one mesh with named materials
(`leaves`, `trunk`, `rock`); the app colours them — leaves and rocks per instance from the
palette — so the files carry only shape and material names. `models.json` beside them lists
each model's kind and height for the world to choose and scale them. Pass model names after
`--` to build only those. Random shapes use fixed seeds, so rebuilds give the same files.
"""

import json
import math
import os
import random
import sys

import bmesh
import bpy

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.normpath(os.path.join(HERE, "..", "..", "app", "assets", "models", "vegetation"))

# Preview colours in Blender only; the app replaces the materials by name.
PREVIEW = {"leaves": (0.55, 0.6, 0.33), "trunk": (0.48, 0.34, 0.33), "rock": (0.76, 0.73, 0.7)}


class Mesh:
    """Faces with material names, built up piece by piece (z up, metres)."""

    def __init__(self):
        self.verts = []
        self.faces = []
        self.materials = []

    def face(self, points, material):
        start = len(self.verts)
        self.verts.extend(points)
        self.faces.append(list(range(start, start + len(points))))
        self.materials.append(material)


def ring(centre, radius, sides, turn, height):
    return [
        (centre[0] + radius * math.cos(turn + 2 * math.pi * i / sides),
         centre[1] + radius * math.sin(turn + 2 * math.pi * i / sides),
         height)
        for i in range(sides)
    ]


def frustum(mesh, base, top, r_base, r_top, sides, material, turn=0.0, cap=True):
    """A cone (r_top 0) or truncated cone from height base to top, counter-clockwise from
    outside; `cap` closes its underside."""
    low = ring((0, 0), r_base, sides, turn, base)
    if r_top > 0:
        high = ring((0, 0), r_top, sides, turn, top)
        for i in range(sides):
            j = (i + 1) % sides
            mesh.face([low[i], low[j], high[j], high[i]], material)
    else:
        for i in range(sides):
            j = (i + 1) % sides
            mesh.face([low[i], low[j], (0.0, 0.0, top)], material)
    if cap:
        mesh.face(list(reversed(low)), material)


ICO_FACES = [(0, 11, 5), (0, 5, 1), (0, 1, 7), (0, 7, 10), (0, 10, 11), (1, 5, 9), (5, 11, 4),
             (11, 10, 2), (10, 7, 6), (7, 1, 8), (3, 9, 4), (3, 4, 2), (3, 2, 6), (3, 6, 8),
             (3, 8, 9), (4, 9, 5), (2, 4, 11), (6, 2, 10), (8, 6, 7), (9, 8, 1)]


def blob(mesh, centre, radius, material, seed, scale=(1.0, 1.0, 1.0), jitter=0.18, floor=None):
    """A rough ball of 20 facets: an icosahedron with its corners pushed in and out; `floor`
    flattens its underside at that height."""
    t = (1 + 5 ** 0.5) / 2
    corners = [(-1, t, 0), (1, t, 0), (-1, -t, 0), (1, -t, 0), (0, -1, t), (0, 1, t), (0, -1, -t),
               (0, 1, -t), (t, 0, -1), (t, 0, 1), (-t, 0, -1), (-t, 0, 1)]
    rng = random.Random(seed)
    turn = rng.uniform(0, 2 * math.pi)
    points = []
    for x, y, z in corners:
        length = math.sqrt(x * x + y * y + z * z)
        k = radius * (1 + rng.uniform(-jitter, jitter)) / length
        x, y, z = x * k * scale[0], y * k * scale[1], z * k * scale[2]
        x, y = x * math.cos(turn) - y * math.sin(turn), x * math.sin(turn) + y * math.cos(turn)
        z = centre[2] + z
        if floor is not None:
            z = max(z, floor)
        points.append((centre[0] + x, centre[1] + y, z))
    for a, b, c in ICO_FACES:
        face = [points[a], points[b], points[c]]
        if not outward(face, centre):
            face.reverse()
        mesh.face(face, material)


def outward(face, centre):
    """Whether the face's corners run counter-clockwise seen from outside, away from centre."""
    (ax, ay, az), (bx, by, bz), (cx, cy, cz) = face
    ux, uy, uz = bx - ax, by - ay, bz - az
    vx, vy, vz = cx - ax, cy - ay, cz - az
    normal = (uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx)
    middle = [(ax + bx + cx) / 3 - centre[0], (ay + by + cy) / 3 - centre[1],
              (az + bz + cz) / 3 - centre[2]]
    return sum(n * m for n, m in zip(normal, middle)) > 0


# --- kinds ---------------------------------------------------------------------------------


def conifer(spec):
    """A trunk under stacked cones, each turned a little against the one below."""
    mesh = Mesh()
    frustum(mesh, -0.6, spec["trunk"], 0.32, 0.22, 5, "trunk", cap=False)
    for k, (base, top, radius) in enumerate(spec["tiers"]):
        frustum(mesh, base, top, radius, 0.0, spec.get("sides", 6), "leaves", turn=k * 0.45)
    return mesh, max(top for _, top, _ in spec["tiers"])


def broadleaf(spec):
    """A trunk under one or more chunky blobs of leaves."""
    mesh = Mesh()
    frustum(mesh, -0.6, spec["trunk"], 0.34, 0.2, 5, "trunk", cap=False)
    height = 0.0
    for k, (x, y, z, radius, squash) in enumerate(spec["crowns"]):
        blob(mesh, (x, y, z), radius, "leaves", spec["seed"] + k, scale=(1.0, 1.0, squash))
        height = max(height, z + radius * squash * 1.15)
    return mesh, height


def bush(spec):
    mesh = Mesh()
    height = 0.0
    for k, (x, y, z, radius, squash) in enumerate(spec["clumps"]):
        blob(mesh, (x, y, z), radius, "leaves", spec["seed"] + k, scale=(1.15, 1.0, squash),
             floor=-0.2)
        height = max(height, z + radius * squash * 1.15)
    return mesh, height


def rock(spec):
    """Rough blocks, sunk into the ground a little so slopes do not show their undersides."""
    mesh = Mesh()
    height = 0.0
    for k, (x, y, z, radius, scale) in enumerate(spec["blocks"]):
        blob(mesh, (x, y, z), radius, "rock", spec["seed"] + k, scale=scale, jitter=0.3)
        height = max(height, z + radius * scale[2] * 1.3)
    return mesh, height


def both_sides(mesh, points, material):
    """A thin surface seen from either side (leaves): the face and its reverse."""
    mesh.face(list(points), material)
    mesh.face(list(reversed(points)), material)


def leaf(mesh, base, heading, length, width, rise, droop, segments, material, fold=0.25,
         tip_width=0.0):
    """A long leaf from `base` out along `heading` (radians): first rising `rise`, then
    drooping `droop` towards its tip, `width` wide where it leaves the stem and `tip_width`
    at its end, folded along its midrib (V-shaped, `fold` of its width deep) so its facets
    catch the light differently."""
    dx, dy = math.cos(heading), math.sin(heading)
    sx, sy = -dy, dx
    spine, edges = [], []
    for k in range(segments + 1):
        t = k / segments
        reach = length * t
        z = base[2] + rise * math.sin(math.pi * 0.5 * min(1.0, 2.0 * t)) - droop * t * t
        half = (width + (tip_width - width) * t) / 2.0 * (1.0 if 0 < k else 0.35)
        centre = (base[0] + dx * reach, base[1] + dy * reach, z)
        spine.append((centre[0], centre[1], z - fold * half))
        edges.append(((centre[0] + sx * half, centre[1] + sy * half, z),
                      (centre[0] - sx * half, centre[1] - sy * half, z)))
    for k in range(segments):
        (l0, r0), (l1, r1) = edges[k], edges[k + 1]
        both_sides(mesh, [spine[k], spine[k + 1], l1, l0], material)
        both_sides(mesh, [spine[k], r0, r1, spine[k + 1]], material)


def bent_trunk(mesh, height, lean, r_base, r_top, segments, sides, material):
    """A trunk curving over by `lean` metres at its top; returns the top's centre."""
    def centre_at(t):
        return (lean * t * t, 0.0, -0.6 + (height + 0.6) * t)

    for k in range(segments):
        t0, t1 = k / segments, (k + 1) / segments
        (x0, y0, z0), (x1, y1, z1) = centre_at(t0), centre_at(t1)
        r0 = r_base + (r_top - r_base) * t0
        r1 = r_base + (r_top - r_base) * t1
        low = ring((x0, y0), r0, sides, 0.0, z0)
        high = ring((x1, y1), r1, sides, 0.0, z1)
        for i in range(sides):
            j = (i + 1) % sides
            mesh.face([low[i], low[j], high[j], high[i]], material)
    return centre_at(1.0)


def palm(spec):
    """A palm: a slender trunk, curving over a little for coconut palms, under a crown of
    long feathery fronds arching out and drooping, with a few coconuts; or for fan palms a
    straight trunk under a ball of stiff fans."""
    mesh = Mesh()
    rng = random.Random(spec["seed"])
    top = bent_trunk(mesh, spec["height"], spec.get("lean", 0.0), spec.get("base", 0.28),
                     spec.get("top", 0.18), spec.get("segments", 4), 5, "trunk")
    fronds = spec["fronds"]
    if spec.get("fans"):
        for k in range(fronds):
            heading = 2 * math.pi * k / fronds + rng.uniform(-0.2, 0.2)
            tilt = rng.uniform(-0.3, 0.6)
            dx, dy = math.cos(heading), math.sin(heading)
            stalk = 0.9
            hub = (top[0] + dx * stalk, top[1] + dy * stalk, top[2] + tilt * stalk)
            # A fan: a half disc of a few facets, standing out and up from the stalk's end.
            fan = [hub]
            lift = spec["leaf"] * (0.45 + tilt * 0.5)
            for i in range(5):
                spread = i / 4.0 - 0.5
                a = heading + spread * 2.0
                fan.append((hub[0] + math.cos(a) * spec["leaf"],
                            hub[1] + math.sin(a) * spec["leaf"],
                            hub[2] + lift * math.cos(spread)))
            for i in range(1, 5):
                both_sides(mesh, [fan[0], fan[i], fan[i + 1]], "leaves")
    else:
        for k in range(fronds):
            heading = 2 * math.pi * k / fronds + rng.uniform(-0.25, 0.25)
            length = spec["leaf"] * rng.uniform(0.85, 1.1)
            leaf(mesh, (top[0], top[1], top[2] - 0.1), heading, length, 1.4, 0.7,
                 rng.uniform(2.8, 3.6), 3, "leaves", tip_width=0.2)
        # A short upright tuft of young fronds in the middle.
        tuft = spec.get("tuft", 3)
        for k in range(tuft):
            heading = 2 * math.pi * k / tuft + 0.5
            leaf(mesh, top, heading, spec["leaf"] * 0.45, 0.5, 1.4, 0.2, 2, "leaves")
        for k in range(spec.get("nuts", 0)):
            a = 2 * math.pi * k / spec["nuts"]
            blob(mesh, (top[0] + 0.32 * math.cos(a), top[1] + 0.32 * math.sin(a), top[2] - 0.45),
                 0.2, "trunk", spec["seed"] + 100 + k, jitter=0.1)
    return mesh, max(v[2] for v in mesh.verts)


def banana(spec):
    """A banana plant: a few stout green stems under big paddle leaves rising and arching
    out."""
    mesh = Mesh()
    rng = random.Random(spec["seed"])
    stem = spec["stem"]
    frustum(mesh, -0.4, stem, 0.2, 0.13, 5, "leaves", cap=False)
    for k in range(spec["leaves"]):
        heading = 2 * math.pi * k / spec["leaves"] + rng.uniform(-0.3, 0.3)
        leaf(mesh, (0.0, 0.0, stem - 0.3 * (k % 3)), heading,
             spec["leaf"] * rng.uniform(0.8, 1.1), 1.0, 1.3, rng.uniform(0.8, 1.4), 3, "leaves",
             fold=0.15, tip_width=0.6)
    return mesh, max(v[2] for v in mesh.verts)


def tropical_bush(spec):
    """A broadleaf shrub of the tropics: a dense clump with big leaves fanning out of it."""
    mesh, height = bush(spec)
    rng = random.Random(spec["seed"] + 7)
    for k in range(spec["blades"]):
        heading = 2 * math.pi * k / spec["blades"] + rng.uniform(-0.3, 0.3)
        leaf(mesh, (0.0, 0.0, height * 0.4), heading, spec["blade"], 0.6, 0.4, 0.6, 2,
             "leaves", fold=0.2, tip_width=0.15)
    return mesh, max(height, max(v[2] for v in mesh.verts))


KINDS = {"conifer": conifer, "broadleaf": broadleaf, "bush": bush, "rock": rock, "palm": palm,
         "banana": banana, "tropical_bush": tropical_bush}

CATALOGUE = [
    {"name": "conifer_tall", "kind": "conifer", "trunk": 2.4, "sides": 6,
     "tiers": [(1.6, 6.6, 2.6), (4.4, 9.2, 2.1), (7.0, 11.8, 1.6), (9.6, 14.0, 1.1)]},
    {"name": "conifer_broad", "kind": "conifer", "trunk": 1.8, "sides": 7,
     "tiers": [(1.2, 5.6, 3.0), (3.8, 8.0, 2.3), (6.2, 10.4, 1.5)]},
    {"name": "broadleaf_round", "kind": "broadleaf", "trunk": 3.6, "seed": 11,
     "crowns": [(0.0, 0.0, 5.6, 3.2, 0.9)]},
    {"name": "broadleaf_cluster", "kind": "broadleaf", "trunk": 3.4, "seed": 23,
     "crowns": [(0.6, 0.3, 4.9, 2.4, 0.85), (-0.8, -0.4, 5.3, 2.3, 0.85),
                (0.1, -0.1, 6.9, 2.0, 0.9)]},
    {"name": "bush", "kind": "bush", "seed": 37,
     "clumps": [(0.0, 0.0, 0.55, 1.05, 0.75), (0.75, 0.45, 0.4, 0.8, 0.7)]},
    {"name": "rock_block", "kind": "rock", "seed": 41,
     "blocks": [(0.0, 0.0, 0.25, 1.0, (1.3, 1.0, 0.75))]},
    {"name": "rock_pair", "kind": "rock", "seed": 53,
     "blocks": [(0.0, 0.0, 0.2, 0.9, (1.2, 1.0, 0.7)), (1.1, 0.5, 0.0, 0.6, (1.1, 1.0, 0.8))]},
    # The tropics (#136): palms, banana plants and broadleaf shrubs.
    {"name": "palm_coconut", "kind": "palm", "seed": 61, "height": 9.5, "lean": 1.6,
     "fronds": 8, "leaf": 3.6, "nuts": 2, "tuft": 2},
    {"name": "palm_short", "kind": "palm", "seed": 67, "height": 5.5, "lean": 0.5, "base": 0.32,
     "top": 0.22, "fronds": 7, "leaf": 3.0, "nuts": 2},
    {"name": "palm_fan", "kind": "palm", "seed": 71, "height": 6.5, "base": 0.3, "top": 0.24,
     "fronds": 9, "leaf": 1.5, "fans": True},
    {"name": "banana", "kind": "banana", "seed": 73, "stem": 2.2, "leaves": 6, "leaf": 2.2},
    {"name": "tropical_bush", "kind": "tropical_bush", "seed": 79, "blades": 6, "blade": 1.7,
     "clumps": [(0.0, 0.0, 0.6, 1.0, 0.8), (0.7, -0.4, 0.45, 0.75, 0.75)]},
]


# --- export ----------------------------------------------------------------------------------


def material(name):
    found = bpy.data.materials.get(name)
    if found:
        return found
    created = bpy.data.materials.new(name)
    created.diffuse_color = (*PREVIEW[name], 1.0)
    return created


def to_object(mesh, name):
    data = bpy.data.meshes.new(name)
    data.from_pydata(mesh.verts, [], mesh.faces)
    names = sorted(set(mesh.materials))
    for n in names:
        data.materials.append(material(n))
    index = {n: i for i, n in enumerate(names)}
    data.polygons.foreach_set("material_index", [index[m] for m in mesh.materials])
    # Flat: one normal per face (skill torqa-look); faces stay separate for that.
    data.polygons.foreach_set("use_smooth", [False] * len(mesh.faces))
    shared = bmesh.new()
    shared.from_mesh(data)
    bmesh.ops.remove_doubles(shared, verts=shared.verts, dist=1e-4)
    shared.to_mesh(data)
    shared.free()
    data.update()
    obj = bpy.data.objects.new(name, data)
    bpy.context.scene.collection.objects.link(obj)
    return obj


def export(obj, path):
    for other in bpy.context.scene.objects:
        other.select_set(False)
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    bpy.ops.export_scene.gltf(
        filepath=path,
        export_format="GLB",
        use_selection=True,
        export_apply=True,
        export_materials="EXPORT",
        export_yup=True,
    )


def main():
    wanted = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    os.makedirs(OUT, exist_ok=True)
    manifest = {}
    for spec in CATALOGUE:
        if wanted and spec["name"] not in wanted:
            continue
        bpy.ops.wm.read_factory_settings(use_empty=True)
        mesh, height = KINDS[spec["kind"]](spec)
        export(to_object(mesh, spec["name"]), os.path.join(OUT, spec["name"] + ".glb"))
        manifest[spec["name"]] = {"kind": spec["kind"], "height": round(height, 2)}
        triangles = sum(len(face) - 2 for face in mesh.faces)
        print(f"built {spec['name']}: {triangles} triangles, {height:.1f} m")
    path = os.path.join(OUT, "models.json")
    if wanted and os.path.exists(path):
        with open(path, encoding="utf-8") as file:
            manifest = {**json.load(file)["models"], **manifest}
    with open(path, "w", encoding="utf-8") as file:
        json.dump({"models": dict(sorted(manifest.items()))}, file, indent=1)
        file.write("\n")


main()
