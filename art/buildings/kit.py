"""Building blocks for the scripted building models.

A `Mesh` collects faces with named materials; texture coordinates are metres on each surface
(walls: along and up; roofs: along the eaves and up the slope). Walls get recessed openings
with windows, doors, sills and shutters; roofs get thickness, soffits, fascia and ridge caps.
Parts are chunky and few (ADR 0011): nothing that does not show at riding distance — no
gutters, downpipes, rafters or glazing bars.

Coordinates are metres: x along the building, y across it, z up, the origin at the centre of
the footprint at ground-floor level. Faces wind counter-clockwise seen from the front (the
glTF convention).
"""

import math

from mathutils import Vector

UP = Vector((0.0, 0.0, 1.0))


class Mesh:
    """Faces with a material name each, turned into one Blender mesh at the end."""

    def __init__(self):
        self.verts = []
        self.faces = []
        self.materials = []
        self.uvs = []
        self.smooth = []

    def add(self, other, offset=(0.0, 0.0, 0.0), turn=0.0):
        """Adds the faces of `other`, turned `turn` degrees about z and then moved by `offset`.
        Texture coordinates are recomputed where they are metric."""
        cos, sin = math.cos(math.radians(turn)), math.sin(math.radians(turn))
        shift = Vector(offset)
        for face, material, uvs, smooth in zip(other.faces, other.materials, other.uvs,
                                               other.smooth):
            points = [
                Vector((v.x * cos - v.y * sin, v.x * sin + v.y * cos, v.z)) + shift
                for v in (other.verts[i] for i in face)
            ]
            metric = uvs == metric_uvs([other.verts[i] for i in face])
            self.face(points, material, None if metric else uvs, smooth)

    def face(self, points, material, uvs=None, smooth=False):
        """A planar polygon, counter-clockwise seen from the side it faces."""
        points = [Vector(p) for p in points]
        if newell(points).length < 1e-9:
            return  # degenerate
        base = len(self.verts)
        self.verts.extend(points)
        self.faces.append(tuple(range(base, base + len(points))))
        self.materials.append(material)
        self.uvs.append(uvs if uvs is not None else metric_uvs(points))
        self.smooth.append(smooth)

    def facing(self, points, normal, material, uvs=None, smooth=False):
        """A planar polygon wound so that it faces `normal`."""
        points = [Vector(p) for p in points]
        if newell(points).dot(Vector(normal)) < 0.0:
            points.reverse()
            if uvs is not None:
                uvs = list(reversed(uvs))
        self.face(points, material, uvs, smooth)

    def box(self, low, high, material, skip=()):
        """An axis-aligned box; `skip` names faces to leave out ("-x", "+x", … "+z")."""
        (x0, y0, z0), (x1, y1, z1) = low, high
        sides = {
            "-x": ([(x0, y0, z0), (x0, y0, z1), (x0, y1, z1), (x0, y1, z0)], (-1, 0, 0)),
            "+x": ([(x1, y0, z0), (x1, y1, z0), (x1, y1, z1), (x1, y0, z1)], (1, 0, 0)),
            "-y": ([(x0, y0, z0), (x1, y0, z0), (x1, y0, z1), (x0, y0, z1)], (0, -1, 0)),
            "+y": ([(x0, y1, z0), (x0, y1, z1), (x1, y1, z1), (x1, y1, z0)], (0, 1, 0)),
            "-z": ([(x0, y0, z0), (x0, y1, z0), (x1, y1, z0), (x1, y0, z0)], (0, 0, -1)),
            "+z": ([(x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)], (0, 0, 1)),
        }
        for name, (points, normal) in sides.items():
            if name not in skip:
                self.facing(points, normal, material)

    def beam(self, start, end, width, height, material, up=UP, caps=True, top=True):
        """A box from `start` to `end` with a `width` × `height` cross-section; `up` tilts it.
        `top=False` leaves out the side facing `up`, for beams under a roof."""
        start, end = Vector(start), Vector(end)
        along = (end - start).normalized()
        side = along.cross(Vector(up)).normalized()
        if side.length < 1e-6:
            side = along.cross(Vector((1.0, 0.0, 0.0))).normalized()
        upward = side.cross(along).normalized()
        w, h = side * (width / 2.0), upward * (height / 2.0)
        ring = [-w - h, w - h, w + h, -w + h]
        for i in range(4):
            a, b = ring[i], ring[(i + 1) % 4]
            outward = (a + b).normalized()
            if not top and outward.dot(upward) > 0.9:
                continue
            self.facing([start + a, start + b, end + b, end + a], outward, material)
        if caps:
            self.facing([start + r for r in ring], -along, material)
            self.facing([end + r for r in ring], along, material)

    def cylinder(self, start, end, radius, sides, material, caps=True):
        """A cylinder from `start` to `end`, smooth-shaded."""
        start, end = Vector(start), Vector(end)
        along = (end - start).normalized()
        helper = UP if abs(along.dot(UP)) < 0.9 else Vector((1.0, 0.0, 0.0))
        u = along.cross(helper).normalized()
        v = along.cross(u).normalized()
        ring = [
            (u * math.cos(2 * math.pi * k / sides) + v * math.sin(2 * math.pi * k / sides))
            * radius
            for k in range(sides)
        ]
        for k in range(sides):
            a, b = ring[k], ring[(k + 1) % sides]
            self.facing(
                [start + a, start + b, end + b, end + a],
                (a + b).normalized(),
                material,
                smooth=True,
            )
        if caps:
            self.facing([start + r for r in ring], -along, material)
            self.facing([end + r for r in ring], along, material)

    def sphere(self, centre, radius, material, rings=4, segments=8):
        """A low sphere, smooth-shaded: flower heads, finials."""
        centre = Vector(centre)

        def point(ring, segment):
            phi = math.pi * ring / rings
            theta = 2 * math.pi * segment / segments
            return centre + radius * Vector(
                (math.sin(phi) * math.cos(theta), math.sin(phi) * math.sin(theta), math.cos(phi))
            )

        for r in range(rings):
            for s in range(segments):
                corners = [point(r, s), point(r + 1, s), point(r + 1, s + 1), point(r, s + 1)]
                middle = sum(corners, Vector()) / 4.0 - centre
                unique = []
                for c in corners:
                    if all((c - u).length > 1e-6 for u in unique):
                        unique.append(c)
                if len(unique) >= 3:
                    self.facing(unique, middle, material, smooth=True)


def newell(points):
    """Area-weighted normal of a polygon (zero when degenerate)."""
    normal = Vector()
    for i, a in enumerate(points):
        b = points[(i + 1) % len(points)]
        normal.x += (a.y - b.y) * (a.z + b.z)
        normal.y += (a.z - b.z) * (a.x + b.x)
        normal.z += (a.x - b.x) * (a.y + b.y)
    return normal


def metric_uvs(points):
    """Texture coordinates in metres on the polygon's plane: level faces use x and y, upright
    ones the horizontal along the face and the height, slopes the horizontal along the slope
    and the distance up it."""
    normal = newell(points).normalized()
    if abs(normal.z) > 0.98:
        return [(p.x, p.y) for p in points]
    along = Vector((-normal.y, normal.x, 0.0)).normalized()
    if abs(normal.z) < 0.02:
        return [(p.dot(along), p.z) for p in points]
    up_slope = (UP - normal * normal.z).normalized()
    return [(p.dot(along), p.dot(up_slope)) for p in points]


class Facade:
    """One straight wall from `a` to `b` (x, y on the outer surface), facing to the right of
    travel, i.e. outwards for footprints going counter-clockwise."""

    def __init__(self, a, b):
        self.a, self.b = Vector((a[0], a[1], 0.0)), Vector((b[0], b[1], 0.0))
        self.length = (self.b - self.a).length
        self.along = (self.b - self.a) / self.length
        self.out = Vector((self.along.y, -self.along.x, 0.0))

    def point(self, u, z, depth=0.0):
        """The point `u` metres along the wall at height `z`, `depth` metres into it."""
        return self.a + self.along * u - self.out * depth + UP * z


class Opening:
    """A rectangular hole in a facade, `u0`–`u1` along it and `z0`–`z1` up, recessed `depth`."""

    def __init__(self, u0, u1, z0, z1, depth=0.16, arch=False):
        self.u0, self.u1, self.z0, self.z1, self.depth, self.arch = u0, u1, z0, z1, depth, arch

    @property
    def width(self):
        return self.u1 - self.u0


def wall(mesh, facade, z0, z1, material, openings=(), outline=None):
    """The facade between heights `z0` and `z1`, with holes for `openings` and their reveals.

    `outline`, if given, is a function of height returning the (left, right) extent along the
    facade, for gables narrowing towards the ridge; walls are otherwise full length."""
    full = outline or (lambda z: (0.0, facade.length))
    breaks_z = sorted({z0, z1, *[z for o in openings for z in (o.z0, o.z1) if z0 < z < z1]})
    for zb, zt in zip(breaks_z, breaks_z[1:]):
        inside = [o for o in openings if o.z0 <= zb and o.z1 >= zt]
        inside.sort(key=lambda o: o.u0)
        left = full(zb)[0], full(zt)[0]
        right = full(zb)[1], full(zt)[1]
        # Pieces between the openings of this band, the outer ones following the outline.
        starts = [left] + [(o.u1, o.u1) for o in inside]
        ends = [(o.u0, o.u0) for o in inside] + [right]
        for (s_bottom, s_top), (e_bottom, e_top) in zip(starts, ends):
            corners = [
                facade.point(s_bottom, zb),
                facade.point(e_bottom, zb),
                facade.point(e_top, zt),
                facade.point(s_top, zt),
            ]
            if e_bottom - s_bottom > 1e-4 or e_top - s_top > 1e-4:
                if e_top - s_top <= 1e-4:
                    corners = corners[:3]
                mesh.facing(corners, facade.out, material)
    for o in openings:
        reveal(mesh, facade, o, material)


def reveal(mesh, facade, o, material, sill=True):
    """The sides of an opening, from the facade back to where its window or door sits."""
    p = facade.point
    d = o.depth
    mesh.facing(
        [p(o.u0, o.z0), p(o.u0, o.z1), p(o.u0, o.z1, d), p(o.u0, o.z0, d)], facade.along, material
    )
    mesh.facing(
        [p(o.u1, o.z0), p(o.u1, o.z0, d), p(o.u1, o.z1, d), p(o.u1, o.z1)], -facade.along, material
    )
    mesh.facing(
        [p(o.u0, o.z1), p(o.u1, o.z1), p(o.u1, o.z1, d), p(o.u0, o.z1, d)], -UP, material
    )
    if not sill:
        mesh.facing(
            [p(o.u0, o.z0), p(o.u0, o.z0, d), p(o.u1, o.z0, d), p(o.u1, o.z0)], UP, material
        )
    if o.arch:
        spandrels(mesh, facade, o, material)


def spandrels(mesh, facade, o, material, segments=8):
    """Fills the top corners of an opening so that it shows a round arch."""
    radius = o.width / 2.0
    centre_u, spring = (o.u0 + o.u1) / 2.0, o.z1 - radius
    arc = [
        (centre_u - radius * math.cos(math.pi * k / segments),
         spring + radius * math.sin(math.pi * k / segments))
        for k in range(segments + 1)
    ]
    half = segments // 2
    for corner, points in ((o.u0, arc[: half + 1]), (o.u1, arc[half:])):
        for (u1, z1), (u2, z2) in zip(points, points[1:]):
            mesh.facing(
                [facade.point(corner, o.z1, -0.001), facade.point(u1, z1, -0.001),
                 facade.point(u2, z2, -0.001)],
                facade.out,
                material,
            )


def window(mesh, facade, o, frame="frame", sill="stone", shutters=None, flowers=False):
    """A window in opening `o`: a frame, glass, a sill outside and, if `shutters` names a
    material, shutters folded back against the wall."""
    p = facade.point
    fw, d = 0.07, o.depth
    inner = (o.u0 + fw, o.u1 - fw, o.z0 + fw, o.z1 - fw)
    # Frame: a ring facing out just in front of the glass, with its inner edges.
    front = d - 0.03
    ring_outer = [(o.u0, o.z0), (o.u1, o.z0), (o.u1, o.z1), (o.u0, o.z1)]
    ring_inner = [(inner[0], inner[2]), (inner[1], inner[2]), (inner[1], inner[3]),
                  (inner[0], inner[3])]
    for i in range(4):
        (a, b), (c, e) = ring_outer[i], ring_outer[(i + 1) % 4]
        (f, g), (h, k) = ring_inner[(i + 1) % 4], ring_inner[i]
        mesh.facing([p(a, b, front), p(c, e, front), p(f, g, front), p(h, k, front)],
                    facade.out, frame)
    glass = d + 0.02
    for (a, b), (c, e) in zip(ring_inner, ring_inner[1:] + ring_inner[:1]):
        mid = Vector(((a + c) / 2.0, 0.0, (b + e) / 2.0))
        centre = Vector(((inner[0] + inner[1]) / 2.0, 0.0, (inner[2] + inner[3]) / 2.0))
        toward = centre - mid
        normal = facade.along * toward.x + UP * toward.z
        mesh.facing([p(a, b, front), p(c, e, front), p(c, e, glass), p(a, b, glass)],
                    normal, frame)
    # Church windows (metal frames) have stained glass of their own colour.
    mesh.facing([p(inner[0], inner[2], glass), p(inner[1], inner[2], glass),
                 p(inner[1], inner[3], glass), p(inner[0], inner[3], glass)],
                facade.out, "leaded" if frame == "metal" else "glass")
    if sill:
        mesh_sill(mesh, facade, o, sill)
    else:
        mesh.facing([p(o.u0, o.z0), p(o.u0, o.z0, d), p(o.u1, o.z0, d), p(o.u1, o.z0)], UP,
                    frame)
    if shutters:
        width = o.width / 2.0
        for u0, u1, side in ((o.u0 - width - 0.03, o.u0 - 0.03, -1.0),
                             (o.u1 + 0.03, o.u1 + width + 0.03, 1.0)):
            shutter(mesh, facade, u0, u1, o.z0, o.z1, shutters, side)
    if flowers:
        flower_box(mesh, facade, o)


def mesh_sill(mesh, facade, o, material):
    """A sill below an opening, sloping out over the wall."""
    p = facade.point
    u0, u1 = o.u0 - 0.05, o.u1 + 0.05
    top, bottom, out, back = o.z0, o.z0 - 0.06, -0.06, o.depth
    corners_top = [p(u0, top, back), p(u1, top, back), p(u1, top - 0.02, out), p(u0, top - 0.02, out)]
    mesh.facing(corners_top, UP + facade.out * 0.1, material)
    mesh.facing([p(u0, bottom, out), p(u1, bottom, out), p(u1, top - 0.02, out), p(u0, top - 0.02, out)],
                facade.out, material)


def shutter(mesh, facade, u0, u1, z0, z1, material, side):
    """A shutter folded back against the wall, left (`side` −1) or right (1) of its window;
    slats are drawn by its material."""
    p = facade.point
    near, far = -0.02, -0.055
    corners = [(u0, z0), (u1, z0), (u1, z1), (u0, z1)]
    mesh.facing([p(u, z, far) for u, z in corners], facade.out, material,
                uvs=[(0, 0), (1, 0), (1, 1), (0, 1)])
    # Its thickness only shows on the side away from the window.
    outer = u0 if side < 0 else u1
    mesh.facing([p(outer, z0, near), p(outer, z0, far), p(outer, z1, far), p(outer, z1, near)],
                facade.along * side, material)


def flower_box(mesh, facade, o):
    """A box of red geraniums under a window, as on Swiss farmhouses and chalets."""
    p = facade.point
    u0, u1 = o.u0 + 0.02, o.u1 - 0.02
    z = o.z0 - 0.06
    out = -0.28
    corners = [p(u0, z - 0.2, -0.06), p(u1, z - 0.2, -0.06), p(u1, z - 0.2, out), p(u0, z - 0.2, out)]
    mesh.facing(corners, -UP, "wood")
    mesh.facing([p(u0, z - 0.2, out), p(u1, z - 0.2, out), p(u1, z, out), p(u0, z, out)],
                facade.out, "wood")
    for u in (u0, u1):
        side = -1.0 if u == u0 else 1.0
        mesh.facing([p(u, z - 0.2, -0.06), p(u, z - 0.2, out), p(u, z, out), p(u, z, -0.06)],
                    facade.along * side, "wood")
    plants(mesh, p(u0 + 0.04, z + 0.04, -0.17), p(u1 - 0.04, z + 0.04, -0.17), facade.out)


def plants(mesh, start, end, out, spacing=0.4):
    """Geraniums along a box from `start` to `end`: a strip of leaves dotted with blossoms."""
    start, end = Vector(start), Vector(end)
    mesh.beam(start, end, 0.22, 0.2, "leaves")
    along = end - start
    count = max(2, int(along.length / spacing))
    for k in range(count):
        # Staggered front and back, a little up and down, like real plants.
        t = (k + 0.5) / count
        lean = out.normalized() * (0.06 if k % 2 else -0.04)
        lift = UP * (0.12 + 0.04 * ((k * 7) % 3) / 2.0)
        mesh.sphere(start + along * t + lean + lift, 0.09, "flowers", rings=2, segments=3)


def door(mesh, facade, o, leaf="door", step="stone", canopy=None):
    """A door in opening `o` (from the ground floor up), a step and perhaps a canopy roof."""
    p = facade.point
    d = o.depth
    mesh.facing([p(o.u0, o.z0, d), p(o.u1, o.z0, d), p(o.u1, o.z1, d), p(o.u0, o.z1, d)],
                facade.out, leaf, uvs=[(0, 0), (1, 0), (1, 1), (0, 1)])
    mesh.facing([p(o.u0, o.z0), p(o.u0, o.z0, d), p(o.u1, o.z0, d), p(o.u1, o.z0)], UP, step)
    s0, s1 = o.u0 - 0.25, o.u1 + 0.25
    top, out = o.z0, -0.45
    corners_top = [p(s0, top, 0.0), p(s0, top, out), p(s1, top, out), p(s1, top, 0.0)]
    mesh.facing(corners_top, UP, step)
    mesh.facing([p(s0, top - 0.18, out), p(s1, top - 0.18, out), p(s1, top, out), p(s0, top, out)],
                facade.out, step)
    for u, side in ((s0, -1.0), (s1, 1.0)):
        mesh.facing([p(u, top - 0.18, 0.0), p(u, top - 0.18, out), p(u, top, out), p(u, top, 0.0)],
                    facade.along * side, step)
    if canopy:
        c0, c1 = o.u0 - 0.5, o.u1 + 0.5
        low, high, reach = o.z1 + 0.25, o.z1 + 0.6, -1.1
        mesh.facing([p(c0, low, reach), p(c1, low, reach), p(c1, high, 0.0), p(c0, high, 0.0)],
                    UP + facade.out, canopy)
        mesh.facing([p(c0, low - 0.08, reach), p(c0, high - 0.08, 0.0), p(c1, high - 0.08, 0.0),
                     p(c1, low - 0.08, reach)], -UP, "wood")
        mesh.facing([p(c0, low - 0.08, reach), p(c1, low - 0.08, reach), p(c1, low, reach),
                     p(c0, low, reach)], facade.out, "wood")
        for u in (c0 + 0.1, c1 - 0.1):
            mesh.beam(p(u, high - 0.6, 0.0), p(u, low - 0.04, reach + 0.1), 0.08, 0.1, "wood")


class Rect:
    """A rectangular footprint `length` × `width` centred on `centre`, x along its length."""

    def __init__(self, length, width, centre=(0.0, 0.0)):
        self.length, self.width, self.centre = length, width, centre

    def corners(self):
        l, w = self.length / 2.0, self.width / 2.0
        cx, cy = self.centre
        return [(cx - l, cy - w), (cx + l, cy - w), (cx + l, cy + w), (cx - l, cy + w)]

    def facades(self):
        """The four walls counter-clockwise from the south-west corner: south (−y), east (+x),
        north (+y), west (−x)."""
        c = self.corners()
        return [Facade(c[i], c[(i + 1) % 4]) for i in range(4)]


def gable_roof(mesh, rect, eaves, pitch_deg, overhang, verge, roof="tiles", under="wood",
               fascia="wood", gable="plaster", gable_openings=(), gable_window=None,
               purlins=False, thickness=0.25):
    """A gable roof with its ridge along x over walls `rect` ending at height `eaves`.
    Returns the ridge height. `gable_window` builds what goes into the gable openings."""
    l, w = rect.length / 2.0, rect.width / 2.0
    slope = math.tan(math.radians(pitch_deg))
    ridge = eaves + w * slope
    edge = eaves - overhang * slope
    reach = l + verge
    t = thickness
    for side in (-1.0, 1.0):
        y = side * (w + overhang)
        out = Vector((0.0, side, 0.0))
        top = [(-reach, y, edge + t), (reach, y, edge + t), (reach, 0.0, ridge + t),
               (-reach, 0.0, ridge + t)]
        mesh.facing(top, out + UP, roof)
        bottom = [(-reach, y, edge), (reach, y, edge), (reach, 0.0, ridge), (-reach, 0.0, ridge)]
        mesh.facing(bottom, -(out + UP), under)
        mesh.facing([(-reach, y, edge), (reach, y, edge), (reach, y, edge + t),
                     (-reach, y, edge + t)], out, fascia)
        for end in (-1.0, 1.0):
            x = end * reach
            mesh.facing([(x, y, edge), (x, 0.0, ridge), (x, 0.0, ridge + t), (x, y, edge + t)],
                         Vector((end, 0.0, 0.0)), fascia)
    # Ridge cap.
    mesh.beam((-reach - 0.02, 0.0, ridge + t + 0.03), (reach + 0.02, 0.0, ridge + t + 0.03),
              0.28, 0.12, roof)
    if purlins:
        for y in (-w * 0.55, 0.0, w * 0.55):
            z = ridge - abs(y) * slope - 0.14
            for end in (-1.0, 1.0):
                mesh.beam((end * (l - 0.05), y, z), (end * (reach - 0.05), y, z), 0.2, 0.24,
                          "wood_dark")
    for end in (-1.0, 1.0):
        x = end * l
        facade = Facade((x, -end * w), (x, end * w))

        def outline(z, w=w):
            half = max(w * (1.0 - (z - eaves) / (ridge - eaves)), 0.0)
            return (w - half, w + half)

        wall(mesh, facade, eaves, ridge, gable, gable_openings, outline=outline)
        if gable_window:
            for o in gable_openings:
                gable_window(facade, o)
    return ridge + t


def hipped_roof(mesh, rect, eaves, pitch_deg, overhang, roof="tiles", under="wood",
                fascia="wood", thickness=0.25, caps=None):
    """A hipped roof over walls `rect`: four slopes up to a ridge along x; `caps` names the
    ridge and hip caps' material if it is not the roof's. Returns its height."""
    l, w = rect.length / 2.0, rect.width / 2.0
    slope = math.tan(math.radians(pitch_deg))
    ridge = eaves + w * slope
    edge = eaves - overhang * slope
    lr, wr, half_ridge = l + overhang, w + overhang, max(l - w, 0.0)
    t = thickness
    for lift, material, sign in ((t, roof, 1.0), (0.0, under, -1.0)):
        for side in (-1.0, 1.0):
            out = Vector((0.0, side, 0.0))
            points = [(-lr, side * wr, edge + lift), (lr, side * wr, edge + lift)]
            if half_ridge > 1e-3:
                points += [(half_ridge, 0.0, ridge + lift), (-half_ridge, 0.0, ridge + lift)]
            else:
                points += [(0.0, 0.0, ridge + lift)]
            mesh.facing(points, (out + UP) * sign, material)
            outx = Vector((side, 0.0, 0.0))
            mesh.facing([(side * lr, -wr, edge + lift), (side * lr, wr, edge + lift),
                         (side * half_ridge, 0.0, ridge + lift)], (outx + UP) * sign, material)
    corners = [(-lr, -wr), (lr, -wr), (lr, wr), (-lr, wr)]
    for i in range(4):
        (x0, y0), (x1, y1) = corners[i], corners[(i + 1) % 4]
        out = Vector(((y1 - y0), -(x1 - x0), 0.0)).normalized()
        mesh.facing([(x0, y0, edge), (x1, y1, edge), (x1, y1, edge + t), (x0, y0, edge + t)],
                     out, fascia)
    # Ridge and hip caps.
    if half_ridge > 1e-3:
        mesh.beam((-half_ridge, 0.0, ridge + t + 0.03), (half_ridge, 0.0, ridge + t + 0.03), 0.28,
                  0.12, caps or roof)
    for sx in (-1.0, 1.0):
        for sy in (-1.0, 1.0):
            mesh.beam((sx * lr, sy * wr, edge + t + 0.03), (sx * half_ridge, 0.0, ridge + t + 0.03),
                      0.24, 0.1, caps or roof)
    return ridge + t


def chimney(mesh, x, y, base, top, size=0.6, material="plaster"):
    """A chimney from inside the roof (`base`) to `top`, with a cap."""
    s = size / 2.0
    mesh.box((x - s, y - s, base), (x + s, y + s, top), material, skip=("-z",))
    mesh.box((x - s - 0.06, y - s - 0.06, top), (x + s + 0.06, y + s + 0.06, top + 0.08),
             "metal")
    mesh.box((x - s * 0.5, y - s * 0.5, top + 0.08), (x + s * 0.5, y + s * 0.5, top + 0.3),
             "metal", skip=("-z",))


def half_hipped_roof(mesh, rect, eaves, pitch_deg, overhang, verge, kink=0.6, roof="tiles",
                     under="wood", fascia="wood", gable="wood", gable_openings=None,
                     gable_window=None, arches=(1.0,), thickness=0.28):
    """A half-hipped roof as on Bernese farmhouses: long slopes reaching low, the gables
    rising to `kink` of the roof's height, where small hips take over; under them a round
    boarded arch (the Ründi) frames the gables at `arches` (1 the +x end, −1 the other).
    `gable_openings` maps each end to its openings. Returns the ridge height."""
    l, w = rect.length / 2.0, rect.width / 2.0
    slope = math.tan(math.radians(pitch_deg))
    ridge = eaves + w * slope
    edge = eaves - overhang * slope
    knee = eaves + kink * (ridge - eaves)
    wk = w * (1.0 - kink)
    reach = l + verge
    ridge_end = reach - (ridge - knee) / slope
    t = thickness
    for lift, material, sign in ((t, roof, 1.0), (0.0, under, -1.0)):
        for side in (-1.0, 1.0):
            y = side * (w + overhang)
            points = [(-reach, y, edge + lift), (reach, y, edge + lift),
                      (reach, side * wk, knee + lift), (ridge_end, 0.0, ridge + lift),
                      (-ridge_end, 0.0, ridge + lift), (-reach, side * wk, knee + lift)]
            mesh.facing(points, Vector((0.0, side, 1.0)) * sign, material)
            hip = [(side * reach, -wk, knee + lift), (side * reach, wk, knee + lift),
                   (side * ridge_end, 0.0, ridge + lift)]
            mesh.facing(hip, Vector((side, 0.0, 1.0)) * sign, material)
    for side in (-1.0, 1.0):
        y = side * (w + overhang)
        mesh.facing([(-reach, y, edge), (reach, y, edge), (reach, y, edge + t),
                     (-reach, y, edge + t)], Vector((0.0, side, 0.0)), fascia)
        for end in (-1.0, 1.0):
            x = end * reach
            mesh.facing([(x, y, edge), (x, side * wk, knee), (x, side * wk, knee + t),
                         (x, y, edge + t)], Vector((end, 0.0, 0.0)), fascia)
            mesh.facing([(x, -wk, knee), (x, wk, knee), (x, wk, knee + t), (x, -wk, knee + t)],
                         Vector((end, 0.0, 0.0)), fascia)
    mesh.beam((-ridge_end, 0.0, ridge + t + 0.03), (ridge_end, 0.0, ridge + t + 0.03), 0.28,
              0.12, roof)
    for end in (-1.0, 1.0):
        x = end * l
        facade = Facade((x, -end * w), (x, end * w))

        def outline(z, w=w):
            half = max(w - (z - eaves) / slope, wk)
            return (w - half, w + half)

        openings = (gable_openings or {}).get(end, [])
        wall(mesh, facade, eaves, knee, gable, openings, outline=outline)
        if gable_window:
            for o in openings:
                gable_window(facade, o)
        if end in arches:
            rundi(mesh, end, l, reach, w, overhang, edge, knee, slope, wk)
    return ridge + t


def rundi(mesh, end, l, reach, w, overhang, edge, knee, slope, wk, segments=14):
    """The round boarded arch under a half-hip, from eaves to eaves."""
    front = end * (reach - 0.06)
    back = end * l
    span = w + overhang * 0.85
    spring, apex = edge + 0.25, knee - 0.15

    def arc(k):
        phi = math.pi * k / segments
        return -span * math.cos(phi), spring + (apex - spring) * math.sin(phi)

    def roof_line(y):
        # The underside of the roof at the front: the long slopes, then the hip's eaves.
        return min(edge + (w + overhang - abs(y)) * slope, knee)

    for k in range(segments):
        (y0, z0), (y1, z1) = arc(k), arc(k + 1)
        mesh.facing([(front, y0, z0), (front, y1, z1), (front, y1, roof_line(y1)),
                     (front, y0, roof_line(y0))], Vector((end, 0.0, 0.0)), "wood")
        middle = Vector((0.0, (y0 + y1) / 2.0, (z0 + z1) / 2.0 - spring))
        mesh.facing([(front, y0, z0), (back, y0, z0), (back, y1, z1), (front, y1, z1)],
                    -middle, "wood_light")
    for y_edge in (-(w + overhang), w + overhang):
        y_arc = math.copysign(span, y_edge)
        mesh.facing([(front, y_arc, spring), (front, y_edge, edge), (front, y_arc, roof_line(y_arc))],
                    Vector((end, 0.0, 0.0)), "wood")


def needle(mesh, centre, half, base, height, material, sides=8):
    """A pointed spire: a pyramid with `sides` faces on a base `half` wide, with a ball and
    a cross on its tip."""
    cx, cy = centre
    tip = Vector((cx, cy, base + height))
    ring = []
    for k in range(sides):
        phi = 2 * math.pi * (k + 0.5) / sides
        radius = half / math.cos(math.pi / sides) if sides == 4 else half
        ring.append(Vector((cx + radius * math.cos(phi), cy + radius * math.sin(phi), base)))
    for k in range(sides):
        a, b = ring[k], ring[(k + 1) % sides]
        outward = ((a + b) / 2.0 - Vector((cx, cy, base))).normalized() + UP * 0.2
        mesh.facing([a, b, tip], outward, material)
    finial(mesh, tip)


def finial(mesh, tip):
    """A ball and a cross on top of a spire."""
    tip = Vector(tip)
    mesh.cylinder(tip - UP * 0.3, tip + UP * 0.6, 0.04, 6, "metal")
    mesh.sphere(tip + UP * 0.35, 0.18, "copper", rings=4, segments=8)
    mesh.beam(tip + UP * 0.6, tip + UP * 1.7, 0.08, 0.08, "metal")
    mesh.beam(tip + UP * 1.3 - Vector((0.0, 0.35, 0.0)), tip + UP * 1.3 + Vector((0.0, 0.35, 0.0)),
              0.07, 0.07, "metal")


def clock(mesh, facade, u, z, radius, segments=20):
    """A clock face on a facade; the material draws the dial and hands."""
    points, uvs = [], []
    for k in range(segments):
        phi = 2 * math.pi * k / segments
        points.append(facade.point(u + radius * math.cos(phi), z + radius * math.sin(phi), -0.04))
        uvs.append((0.5 + 0.5 * math.cos(phi), 0.5 + 0.5 * math.sin(phi)))
    mesh.facing(points, facade.out, "clock", uvs=uvs)
    for k in range(segments):
        a, b = points[k], points[(k + 1) % segments]
        mid = (a + b) / 2.0 - facade.point(u, z, -0.04)
        mesh.facing([a, b, b + facade.out * -0.04, a + facade.out * -0.04], mid, "metal")


def louvres(mesh, facade, o, material="wood_dark"):
    """Slanted boards across a belfry opening, letting the sound out and keeping rain off."""
    p = facade.point
    count = max(3, int((o.z1 - o.z0) / 0.32))
    for k in range(count):
        z = o.z0 + (k + 0.5) * (o.z1 - o.z0) / count
        mesh.beam(p(o.u0, z, o.depth * 0.5), p(o.u1, z, o.depth * 0.5), 0.03, 0.26,
                  material, up=UP + facade.out * 0.8)


def balcony(mesh, facade, u0, u1, floor, depth=1.3, rail=1.0, flowers=True):
    """A wooden balcony on a facade between `u0` and `u1`: floor, board balustrade on three
    sides, a hand rail and boxes of geraniums along it."""
    p = facade.point
    out = -depth
    corners_top = [p(u0, floor, 0.0), p(u1, floor, 0.0), p(u1, floor, out), p(u0, floor, out)]
    mesh.facing(corners_top, UP, "wood")
    bottom = floor - 0.18
    mesh.facing([p(u0, bottom, 0.0), p(u0, bottom, out), p(u1, bottom, out), p(u1, bottom, 0.0)],
                -UP, "wood_dark")
    mesh.facing([p(u0, bottom, out), p(u1, bottom, out), p(u1, floor, out), p(u0, floor, out)],
                facade.out, "wood_dark")
    for u, side in ((u0, -1.0), (u1, 1.0)):
        mesh.facing([p(u, bottom, 0.0), p(u, bottom, out), p(u, floor, out), p(u, floor, 0.0)],
                    facade.along * side, "wood_dark")
    # A boarded balustrade along the front and the two sides, as solid panels: single boards
    # would not show at riding distance.
    front = out + 0.05
    top = floor + rail
    mesh.facing([p(u0, floor, front), p(u1, floor, front), p(u1, top, front), p(u0, top, front)],
                facade.out, "wood")
    mesh.facing([p(u0, floor, front + 0.06), p(u1, floor, front + 0.06),
                 p(u1, top, front + 0.06), p(u0, top, front + 0.06)], -facade.out, "wood")
    for u in (u0 + 0.05, u1 - 0.05):
        for face in (-1.0, 1.0):
            mesh.facing([p(u, floor, 0.0), p(u, floor, front), p(u, top, front), p(u, top, 0.0)],
                        facade.along * face, "wood")
    mesh.beam(p(u0, floor + rail, front), p(u1, floor + rail, front), 0.12, 0.08, "wood_dark")
    for u in (u0 + 0.05, u1 - 0.05):
        mesh.beam(p(u, floor + rail, 0.0), p(u, floor + rail, front), 0.12, 0.08, "wood_dark")
    if flowers:
        plants(mesh, p(u0 + 0.1, floor + rail + 0.12, front - 0.12),
               p(u1 - 0.1, floor + rail + 0.12, front - 0.12), facade.out)
        mesh.facing([p(u0, floor + rail - 0.2, front - 0.25), p(u1, floor + rail - 0.2, front - 0.25),
                     p(u1, floor + rail + 0.05, front - 0.25), p(u0, floor + rail + 0.05, front - 0.25)],
                    facade.out, "wood")


def log_corners(mesh, rect, z0, z1, step=0.5, reach=0.35):
    """Log ends crossing at the corners of a log-built storey: chunky blocks, alternately
    along and across."""
    l, w = rect.length / 2.0, rect.width / 2.0
    z = z0 + 0.1
    level = 0
    while z + 0.3 < z1:
        for sx in (-1.0, 1.0):
            for sy in (-1.0, 1.0):
                x, y = sx * l, sy * w
                if level % 2 == 0:
                    low = (min(x, x + sx * reach), min(y - sy * 0.12, y + sy * 0.12), z)
                    high = (max(x, x + sx * reach), max(y - sy * 0.12, y + sy * 0.12), z + 0.3)
                else:
                    low = (min(x - sx * 0.12, x + sx * 0.12), min(y, y + sy * reach), z)
                    high = (max(x - sx * 0.12, x + sx * 0.12), max(y, y + sy * reach), z + 0.3)
                mesh.box(low, high, "wood_dark", skip=())
        z += step
        level += 1


def band(mesh, rect, z0, z1, out, material):
    """A band round the walls of `rect` from `z0` to `z1`, standing `out` from them: a cornice,
    a string course or a coloured floor band."""
    l, w = rect.length / 2.0 + out, rect.width / 2.0 + out
    cx, cy = rect.centre
    corners = [(cx - l, cy - w), (cx + l, cy - w), (cx + l, cy + w), (cx - l, cy + w)]
    for i in range(4):
        (x0, y0), (x1, y1) = corners[i], corners[(i + 1) % 4]
        normal = Vector(((y1 - y0), -(x1 - x0), 0.0)).normalized()
        mesh.facing([(x0, y0, z0), (x1, y1, z0), (x1, y1, z1), (x0, y0, z1)], normal, material)
    inner = [(cx - l + out, cy - w + out), (cx + l - out, cy - w + out),
             (cx + l - out, cy + w - out), (cx - l + out, cy + w - out)]
    for z, normal in ((z1, UP), (z0, -UP)):
        for i in range(4):
            a, b = corners[i], corners[(i + 1) % 4]
            c, d = inner[(i + 1) % 4], inner[i]
            mesh.facing([(*a, z), (*b, z), (*c, z), (*d, z)], normal, material)


def flat_roof(mesh, rect, roof_z, parapet, roof="roof_flat", wall_material="plaster",
              cap="accent", cap_out=0.25, cap_height=0.35):
    """A flat roof at `roof_z` behind a parapet `parapet` high (the walls below reach up to
    it), with a cap round its top standing `cap_out` from the walls. Returns the cap's top."""
    l, w = rect.length / 2.0, rect.width / 2.0
    t = 0.3
    cx, cy = rect.centre
    top = roof_z + parapet
    inner = Rect(rect.length - 2 * t, rect.width - 2 * t, rect.centre)
    il, iw = inner.length / 2.0, inner.width / 2.0
    mesh.facing([(cx - il, cy - iw, roof_z), (cx + il, cy - iw, roof_z),
                 (cx + il, cy + iw, roof_z), (cx - il, cy + iw, roof_z)], UP, roof)
    # The parapet's inner faces, looking into the roof.
    for facade in inner.facades():
        a, b = facade.point(0.0, roof_z), facade.point(facade.length, roof_z)
        mesh.facing([a, b, b + UP * parapet, a + UP * parapet], -facade.out, wall_material)
    # The cap, its top reaching in over the parapet to the roof's edge.
    band(mesh, rect, top - cap_height, top, cap_out, cap)
    outer = [(cx - l, cy - w), (cx + l, cy - w), (cx + l, cy + w), (cx - l, cy + w)]
    edge = [(cx - il, cy - iw), (cx + il, cy - iw), (cx + il, cy + iw), (cx - il, cy + iw)]
    for i in range(4):
        a, b = outer[i], outer[(i + 1) % 4]
        c, d = edge[(i + 1) % 4], edge[i]
        mesh.facing([(*a, top), (*b, top), (*c, top), (*d, top)], UP, cap)
    return top


def roof_units(mesh, rect, roof_z, seed, count=2):
    """Machinery on a flat roof: a plant room and a few air-conditioning units."""
    l, w = rect.length / 2.0, rect.width / 2.0
    rl, rw = min(l * 0.3, 3.5), min(w * 0.45, 2.5)
    x = l * (0.35 if seed % 2 else -0.3)
    mesh.box((x - rl, -rw, roof_z), (x + rl, rw, roof_z + 2.6), "plaster", skip=("-z",))
    mesh.box((x - rl - 0.12, -rw - 0.12, roof_z + 2.6), (x + rl + 0.12, rw + 0.12, roof_z + 2.8),
             "metal", skip=())
    for k in range(count):
        ux = -x * 0.8 + (k - (count - 1) / 2.0) * 2.4
        uy = w * 0.3 * (1 if (seed + k) % 2 else -1)
        mesh.box((ux - 0.7, uy - 0.5, roof_z), (ux + 0.7, uy + 0.5, roof_z + 0.9), "metal",
                 skip=("-z",))


def canopy(mesh, facade, u0, u1, z, reach, material="accent", posts=True, thickness=0.3):
    """A flat canopy over an entrance from `u0` to `u1` at height `z`, reaching `reach` out,
    on two posts at its front corners if `posts`."""
    p = facade.point
    out = -reach
    low, high = z, z + thickness
    mesh.facing([p(u0, high, 0.0), p(u1, high, 0.0), p(u1, high, out), p(u0, high, out)], UP,
                material)
    mesh.facing([p(u0, low, 0.0), p(u0, low, out), p(u1, low, out), p(u1, low, 0.0)], -UP,
                material)
    mesh.facing([p(u0, low, out), p(u1, low, out), p(u1, high, out), p(u0, high, out)],
                facade.out, material)
    for u, side in ((u0, -1.0), (u1, 1.0)):
        mesh.facing([p(u, low, 0.0), p(u, low, out), p(u, high, out), p(u, high, 0.0)],
                    facade.along * side, material)
    if posts:
        for u in (u0 + 0.2, u1 - 0.2):
            mesh.beam(p(u, -0.1, out + 0.2), p(u, low, out + 0.2), 0.22, 0.22, "metal")


def flag(mesh, base, height, material="accent"):
    """A flagpole with its flag, by an entrance."""
    x, y, z = base
    mesh.beam((x, y, z), (x, y, z + height), 0.12, 0.12, "metal")
    mesh.facing([(x, y, z + height - 1.4), (x + 1.9, y, z + height - 1.4),
                 (x + 1.9, y, z + height - 0.2), (x, y, z + height - 0.2)], (0, -1, 0), material)
    mesh.facing([(x, y, z + height - 1.4), (x, y, z + height - 0.2),
                 (x + 1.9, y, z + height - 0.2), (x + 1.9, y, z + height - 1.4)], (0, 1, 0),
                material)
