"""The kinds of building, each built from a catalogue entry (see catalogue.json).

Every builder returns the mesh and the footprint its walls stand on (length along x, width
along y), which the app fits to the map's outlines.
"""

import math

from mathutils import Vector

from kit import (
    UP, Facade, Mesh, Opening, Rect, band, balcony, canopy, chimney, clock, door, finial, flag,
    flat_roof, gable_roof, half_hipped_roof, hipped_roof, log_corners, louvres, needle,
    roof_units, wall, window,
)

# Floor-to-floor height.
STOREY = 2.9
# The ground floor sits this far above the ground.
FLOOR = 0.3
# Walls reach this far below the ground, so slopes never show a gap (the world places models
# at the highest ground of their plot and keeps shells on steeper plots).
BASEMENT = 3.0
# Walls end this far above the top storey's floor plus its height.
PLATE = 0.25


def columns(length, spacing, margin):
    """Centres of window columns spread evenly along a wall."""
    usable = length - 2.0 * margin
    count = max(1, int(usable / spacing) + 1)
    if count == 1:
        return [length / 2.0]
    return [margin + usable * i / (count - 1) for i in range(count)]


def eaves_height(storeys):
    return FLOOR + storeys * STOREY + PLATE


def footprint(length, width, eaves, top, **more):
    return {"length": length, "width": width, "eaves": eaves, "height": top, **more}


def attic_openings(span, rise, eaves, width, height, base=None):
    """Windows that fit into a gable `span` wide rising `rise` above the eaves: two side by
    side where there is room, else one, else none."""
    sill, top = 0.55, 0.55 + height
    if rise < top + 0.5:
        return []
    half_at_top = span / 2.0 * (1.0 - top / rise)
    centre = span / 2.0
    if half_at_top >= 1.25 + width / 2.0 + 0.3:
        offsets = (-1.25, 1.25)
    elif half_at_top >= width / 2.0 + 0.3:
        offsets = (0.0,)
    else:
        return []
    return [
        Opening(centre + d - width / 2.0, centre + d + width / 2.0, eaves + sill, eaves + top)
        for d in offsets
    ]


def house(spec):
    """A plastered family house: windows with shutters on every storey, a door with a canopy,
    a gable or hipped tiled roof with a chimney and windows in the gables."""
    rect = Rect(spec["length"], spec["width"])
    storeys = spec["storeys"]
    eaves = eaves_height(storeys)
    mesh = Mesh()
    width, height, sill = 1.1, 1.4, 0.85
    south, east, north, west = rect.facades()
    for facade in (south, east, north, west):
        long_side = facade in (south, north)
        centres = columns(facade.length, 2.9 if long_side else 2.7, 1.5)
        openings, doors = [], []
        for storey in range(storeys):
            floor = FLOOR + storey * STOREY
            for k, u in enumerate(centres):
                if facade is south and storey == 0 and k == len(centres) // 3:
                    doors.append(Opening(u - 0.55, u + 0.55, floor, floor + 2.15, depth=0.2))
                    continue
                openings.append(Opening(u - width / 2, u + width / 2, floor + sill,
                                        floor + sill + height))
        wall(mesh, facade, -BASEMENT, FLOOR, "stone")
        wall(mesh, facade, FLOOR, eaves, "plaster", openings + doors)
        for o in openings:
            window(mesh, facade, o, shutters="shutter")
        for o in doors:
            door(mesh, facade, o, canopy="tiles")

    pitch = spec.get("pitch", 40.0)
    slope = math.tan(math.radians(pitch))
    if spec["roof"] == "gable":
        rise = rect.width / 2.0 * slope
        ridge = gable_roof(
            mesh, rect, eaves, pitch, overhang=0.65, verge=0.5,
            gable_openings=attic_openings(rect.width, rise, eaves, 0.9, 1.15),
            gable_window=lambda facade, o: window(mesh, facade, o, shutters="shutter"),
        )
        inset = rect.width * 0.3
    else:
        ridge = hipped_roof(mesh, rect, eaves, pitch, overhang=0.65)
        inset = min(rect.width, rect.length) * 0.3
    chimney(mesh, rect.length * 0.22, -(rect.width / 2.0 - inset), eaves + inset * slope - 0.4,
            ridge + 0.7)
    return mesh, footprint(rect.length, rect.width, eaves, ridge + 1.0, storeys=storeys)


def chalet(spec):
    """A mountain chalet: a plastered ground floor with log-built storeys above, log ends at
    the corners, balconies with geraniums across the front gable and flower boxes below it, a
    shallow roof with deep eaves and purlins."""
    rect = Rect(spec["length"], spec["width"])
    storeys = spec["storeys"]
    eaves = eaves_height(storeys)
    base = FLOOR + STOREY
    mesh = Mesh()
    south, east, north, west = rect.facades()
    width, height, sill = 0.9, 1.25, 0.8
    for facade in (south, east, north, west):
        front = facade is east
        centres = columns(facade.length, 2.1 if front else 3.0, 1.2)
        lower, upper, glazed, doors = [], [], [], []
        for storey in range(storeys):
            floor = FLOOR + storey * STOREY
            for k, u in enumerate(centres):
                if facade is south and storey == 0 and k == len(centres) // 2:
                    doors.append(Opening(u - 0.5, u + 0.5, floor, floor + 2.05, depth=0.25))
                elif front and storey > 0 and k % 2 == 1:
                    # Doors out onto the balcony.
                    glazed.append(Opening(u - 0.45, u + 0.45, floor + 0.05, floor + 2.15,
                                          depth=0.18))
                elif storey == 0:
                    lower.append(Opening(u - width / 2, u + width / 2, floor + sill,
                                         floor + sill + height, depth=0.22))
                else:
                    upper.append(Opening(u - width / 2, u + width / 2, floor + sill,
                                         floor + sill + height, depth=0.18))
        wall(mesh, facade, -BASEMENT, FLOOR, "stone")
        wall(mesh, facade, FLOOR, base, "plaster", lower + doors)
        wall(mesh, facade, base, eaves, "wood", upper + glazed)
        # Geraniums on the front's ground floor; above, the balconies carry them.
        for o in lower:
            window(mesh, facade, o, shutters="shutter", flowers=front)
        for o in upper:
            window(mesh, facade, o, sill="wood", shutters="shutter")
        for o in glazed:
            window(mesh, facade, o, sill=None)
        for o in doors:
            door(mesh, facade, o)
    log_corners(mesh, rect, base, eaves)
    for storey in range(1, storeys):
        balcony(mesh, east, 0.25, east.length - 0.25, FLOOR + storey * STOREY)

    pitch = spec.get("pitch", 22.0)
    slope = math.tan(math.radians(pitch))
    rise = rect.width / 2.0 * slope
    ridge = gable_roof(
        mesh, rect, eaves, pitch, overhang=1.3, verge=1.8, roof=spec.get("cover", "slate"),
        gable="wood", gable_openings=attic_openings(rect.width, rise, eaves, 0.8, 1.0),
        gable_window=lambda facade, o: window(mesh, facade, o, sill="wood", shutters="shutter"),
        purlins=True,
    )
    inset = rect.width * 0.32
    chimney(mesh, -rect.length * 0.18, rect.width / 2.0 - inset, eaves + inset * slope - 0.4,
            ridge + 0.8, size=0.7)
    return mesh, footprint(rect.length, rect.width, eaves, ridge + 1.1, storeys=storeys)


def farmhouse(spec):
    """A Bernese farmhouse: house and barn under one huge half-hipped roof reaching low, rows
    of windows with geraniums in the living part at the front, the Ründi arch under the
    front hip, boarded barn walls and a barn door at the back."""
    rect = Rect(spec["length"], spec["width"])
    l = rect.length / 2.0
    eaves = FLOOR + STOREY + 1.0
    mesh = Mesh()
    south, east, north, west = rect.facades()
    living = rect.length * 0.42

    def split(facade, at):
        """Two facades: up to `at` metres along, and the rest."""
        middle = facade.point(at, 0.0)
        return Facade(facade.a, middle), Facade(middle, facade.b)

    def window_band(length, start, z0):
        """A row of windows close together, as Bernese houses have them."""
        count = max(2, int((length - 1.6) / 1.15))
        first = start + (length - count * 1.15) / 2.0
        return [Opening(first + k * 1.15 + 0.12, first + k * 1.15 + 1.03, z0, z0 + 1.25,
                        depth=0.15) for k in range(count)]

    # South: barn then living part (x grows along it); north: living part then barn.
    barn_south, living_south = split(south, rect.length - living)
    living_north, barn_north = split(north, living)
    for facade in (living_south, living_north):
        openings = window_band(facade.length, 0.0, FLOOR + 0.85)
        wall(mesh, facade, -BASEMENT, FLOOR, "stone")
        wall(mesh, facade, FLOOR, eaves, "plaster", openings)
        for o in openings:
            window(mesh, facade, o, shutters=None, flowers=True)
    for facade, gate in ((barn_south, False), (barn_north, True)):
        openings = []
        if gate:
            middle = facade.length / 2.0
            openings.append(Opening(middle - 1.7, middle + 1.7, FLOOR, FLOOR + 3.3, depth=0.2))
        else:
            for u in columns(facade.length, 3.5, 2.0):
                openings.append(Opening(u - 0.35, u + 0.35, FLOOR + 1.6, FLOOR + 2.2, depth=0.15))
        wall(mesh, facade, -BASEMENT, FLOOR, "stone")
        wall(mesh, facade, FLOOR, eaves, "wood", openings)
        for o in openings:
            if gate:
                door(mesh, facade, o, leaf="wood_dark", step="stone")
            else:
                window(mesh, facade, o, sill="wood")
    front_openings = window_band(east.length, 0.0, FLOOR + 0.85)
    wall(mesh, east, -BASEMENT, FLOOR, "stone")
    wall(mesh, east, FLOOR, eaves, "plaster", front_openings)
    for o in front_openings:
        window(mesh, east, o, flowers=True)
    wall(mesh, west, -BASEMENT, FLOOR, "stone")
    wall(mesh, west, FLOOR, eaves, "wood")

    pitch = spec.get("pitch", 45.0)
    slope = math.tan(math.radians(pitch))
    w = rect.width / 2.0
    knee = eaves + 0.6 * w * slope
    gable_rows = []
    for z0 in (eaves + 0.35, eaves + 0.35 + 2.6):
        z1 = z0 + 1.15
        if z1 > knee - 0.4:
            continue
        half = w - (z1 - eaves) / slope - 0.4
        if half < 1.2:
            continue
        gable_rows += window_band(2.0 * half, w - half, z0)
    ridge = half_hipped_roof(
        mesh, rect, eaves, pitch, overhang=1.2, verge=1.5, roof=spec.get("cover", "tiles"),
        gable_openings={1.0: gable_rows},
        gable_window=lambda facade, o: window(mesh, facade, o, sill="wood", flowers=True),
    )
    chimney(mesh, l * 0.45, -w * 0.25, eaves + w * 0.75 * slope - 0.5, ridge + 0.6, size=0.8)
    return mesh, footprint(rect.length, rect.width, eaves, ridge + 0.9, storeys=1)


def church(spec):
    """A village church: a high nave with tall arched windows and a choir behind under steep
    roofs, a tower in front with quoins, clocks, a louvred belfry and a needle spire or a
    saddle roof, and the door at its foot."""
    nave_length, width = spec["length"], spec["width"]
    side = spec["tower"]
    choir_length = width * 0.55
    eaves = spec.get("eaves", 8.4)
    tower_top = spec.get("tower_height", 22.0)
    total = side - 0.4 + nave_length + choir_length - 0.3
    # Everything is placed along x from the tower's front, then centred.
    nave_x = side - 0.4 + nave_length / 2.0
    choir_x = side - 0.4 + nave_length + choir_length / 2.0 - 0.3
    mesh = Mesh()

    # Nave.
    nave = Mesh()
    rect = Rect(nave_length, width)
    for facade in rect.facades():
        openings = []
        if abs(facade.out.y) > 0.5:
            for u in columns(nave_length, 4.6, 3.4):
                openings.append(Opening(u - 0.75, u + 0.75, 2.6, 6.9, depth=0.35, arch=True))
        wall(nave, facade, -BASEMENT, 0.4, "stone")
        wall(nave, facade, 0.4, eaves, "plaster", openings)
        for o in openings:
            window(nave, facade, o, frame="metal", sill="stone")
    ridge = gable_roof(nave, rect, eaves, spec.get("pitch", 52.0), overhang=0.45, verge=0.35,
                       roof=spec.get("cover", "tiles"), under="plaster", fascia="plaster",
                       gable="plaster")
    mesh.add(nave, (nave_x, 0.0, 0.0))

    # Choir: narrower and lower, a hipped roof against the nave.
    choir = Mesh()
    choir_rect = Rect(choir_length, width * 0.65)
    choir_eaves = eaves - 1.4
    for facade in choir_rect.facades():
        openings = []
        # Not the end against the nave.
        if facade.out.x > -0.5:
            u = facade.length / 2.0
            openings.append(Opening(u - 0.6, u + 0.6, 2.4, 5.6, depth=0.35, arch=True))
        wall(choir, facade, -BASEMENT, 0.4, "stone")
        wall(choir, facade, 0.4, choir_eaves, "plaster", openings)
        for o in openings:
            window(choir, facade, o, frame="metal", sill="stone")
    hipped_roof(choir, choir_rect, choir_eaves, 50.0, overhang=0.4,
                roof=spec.get("cover", "tiles"), under="plaster", fascia="plaster")
    mesh.add(choir, (choir_x, 0.0, 0.0))

    # Tower.
    tower = Mesh()
    half = side / 2.0
    tower_rect = Rect(side, side)
    belfry_bottom, belfry_top = tower_top - 4.6, tower_top - 1.2
    clock_z = belfry_bottom - 1.7
    for facade in tower_rect.facades():
        openings = [Opening(half - 0.6, half + 0.6, belfry_bottom, belfry_top, depth=0.4,
                            arch=True)]
        doorway = facade.out.x < -0.5
        if doorway:
            openings.append(Opening(half - 0.85, half + 0.85, 0.4, 3.6, depth=0.5, arch=True))
        else:
            openings.append(Opening(half - 0.2, half + 0.2, 7.0, 8.4, depth=0.3, arch=True))
        wall(tower, facade, -BASEMENT, 0.4, "stone")
        wall(tower, facade, 0.4, tower_top, "plaster", openings)
        louvres(tower, facade, openings[0])
        if doorway:
            door(tower, facade, openings[1], leaf="door", step="stone")
        else:
            window(tower, facade, openings[1], frame="metal", sill=None)
        clock(tower, facade, half, clock_z, min(1.0, half * 0.45))
    # Quoins: stone corner strips standing a little proud of the walls.
    for sx in (-1.0, 1.0):
        for sy in (-1.0, 1.0):
            x0, x1 = sorted((sx * (half - 0.37), sx * (half + 0.03)))
            y0, y1 = sorted((sy * (half - 0.37), sy * (half + 0.03)))
            tower.box((x0, y0, 0.4), (x1, y1, tower_top), "stone", skip=("-z",))
    for z in (belfry_bottom - 0.35, tower_top - 0.3):
        tower.box((-half - 0.12, -half - 0.12, z), (half + 0.12, half + 0.12, z + 0.3), "stone")
    if spec.get("spire", "needle") == "needle":
        tower.box((-half - 0.15, -half - 0.15, tower_top), (half + 0.15, half + 0.15,
                  tower_top + 0.25), "metal", skip=("-z",))
        spire_height = side * spec.get("needle", 2.6)
        needle(tower, (0.0, 0.0), half + 0.1, tower_top + 0.25, spire_height,
               spec.get("spire_cover", "copper"))
        top = tower_top + 0.25 + spire_height + 1.7
    else:
        saddle = Mesh()
        roof_top = gable_roof(saddle, Rect(side, side), tower_top, 58.0, overhang=0.35,
                              verge=0.35, roof=spec.get("spire_cover", "tiles"),
                              under="plaster", fascia="plaster", gable="plaster")
        tower.add(saddle, turn=90.0)
        # The ridge runs across the nave once turned.
        finial(tower, (0.0, half - 0.4, roof_top))
        finial(tower, (0.0, -half + 0.4, roof_top))
        top = roof_top + 1.7
    mesh.add(tower, (half, 0.0, 0.0))

    centre = total / 2.0
    centred = Mesh()
    centred.add(mesh, (-centre, 0.0, 0.0))
    return centred, footprint(total, width, eaves, top, tower=side, spire=spec.get("spire", "needle"))


def chapel(spec):
    """A chapel: a small nave under a steep roof with arched windows, the door in the front
    gable and a slender turret with a spire on the ridge."""
    rect = Rect(spec["length"], spec["width"])
    eaves = spec.get("eaves", 4.8)
    mesh = Mesh()
    south, east, north, west = rect.facades()
    for facade in (south, east, north, west):
        openings = []
        if facade in (south, north):
            for u in columns(facade.length, 3.2, 2.2):
                openings.append(Opening(u - 0.5, u + 0.5, 1.9, 4.1, depth=0.3, arch=True))
        elif facade is west:
            u = facade.length / 2.0
            openings.append(Opening(u - 0.65, u + 0.65, 0.3, 2.9, depth=0.35, arch=True))
        wall(mesh, facade, -BASEMENT, 0.3, "stone")
        wall(mesh, facade, 0.3, eaves, "plaster", openings)
        for o in openings:
            if o.z0 < 1.0:
                door(mesh, facade, o)
            else:
                window(mesh, facade, o, frame="metal", sill="stone")
    pitch = spec.get("pitch", 50.0)
    slope = math.tan(math.radians(pitch))
    ridge = gable_roof(mesh, rect, eaves, pitch, overhang=0.4, verge=0.3,
                       roof=spec.get("cover", "slate"), under="plaster", fascia="plaster",
                       gable="plaster")
    # Ridge turret near the front.
    x = -rect.length / 2.0 + 1.4
    half = 0.7
    base = ridge - half * slope - 0.4
    top = ridge + 2.0
    mesh.box((x - half, -half, base), (x + half, half, top), "wood", skip=("-z",))
    for facade in Rect(2 * half, 2 * half, (x, 0.0)).facades():
        o = Opening(half - 0.3, half + 0.3, top - 1.4, top - 0.4, depth=0.1)
        louvres(mesh, facade, o)
    spire = 3.6
    needle(mesh, (x, 0.0), half + 0.12, top, spire, spec.get("spire_cover", "slate"))
    return mesh, footprint(rect.length, rect.width, eaves, top + spire + 1.7)


def shed(spec):
    """A wooden shed or a plastered garage: low walls, a door, a small window, a light roof."""
    rect = Rect(spec["length"], spec["width"])
    garage = spec.get("garage", False)
    eaves = 2.5 if garage else 2.3
    mesh = Mesh()
    south, east, north, west = rect.facades()
    material = "plaster" if garage else "wood"
    for facade in (south, east, north, west):
        openings = []
        if facade is south:
            u = facade.length / 2.0
            width = min(2.6, facade.length - 1.0) if garage else 1.0
            openings.append(Opening(u - width / 2, u + width / 2, 0.1, 2.1, depth=0.12))
        elif facade is east:
            u = facade.length / 2.0
            openings.append(Opening(u - 0.35, u + 0.35, 1.2, 1.8, depth=0.1))
        wall(mesh, facade, -BASEMENT, 0.1, "stone")
        wall(mesh, facade, 0.1, eaves, material, openings)
        for o in openings:
            if o.z0 < 0.5:
                door(mesh, facade, o, leaf="garage" if garage else "wood_dark", step="stone")
            else:
                window(mesh, facade, o, sill="wood")
    pitch = spec.get("pitch", 22.0)
    ridge = gable_roof(mesh, rect, eaves, pitch, overhang=0.35, verge=0.3,
                       roof=spec.get("cover", "sheet"), gable=material)
    return mesh, footprint(rect.length, rect.width, eaves, ridge)


# Offices and hotels have taller floors than houses; their ground floors are taller still.
OFFICE_STOREY = 3.4
LOBBY = 4.0
# Flat roofs end in a parapet this high (as the world's shells, `Roof::Flat`).
PARAPET = 0.7


def lobby(mesh, facade, z0, z1, entrance=None, piers=4.5):
    """A glazed ground floor between piers from `z0` to `z1`: big panes in frames, and if
    `entrance` gives its width, double doors in the middle of the facade."""
    count = max(1, round((facade.length - 1.2) / piers))
    bay = (facade.length - 0.6) / count
    openings, doors = [], []
    for k in range(count):
        u0, u1 = 0.3 + k * bay + 0.3, 0.3 + (k + 1) * bay - 0.3
        middle = abs((u0 + u1) / 2.0 - facade.length / 2.0) < bay / 2.0
        if entrance and middle:
            centre = facade.length / 2.0
            doors.append(Opening(centre - entrance / 2.0, centre + entrance / 2.0, z0,
                                 z0 + 2.5, depth=0.35))
            for a, b in ((u0, centre - entrance / 2.0 - 0.3), (centre + entrance / 2.0 + 0.3, u1)):
                if b - a > 0.8:
                    openings.append(Opening(a, b, z0 + 0.4, z1 - 0.6, depth=0.3))
            continue
        openings.append(Opening(u0, u1, z0 + 0.4, z1 - 0.6, depth=0.3))
    wall(mesh, facade, z0, z1, "plaster", openings + doors)
    for o in openings:
        window(mesh, facade, o, sill=None)
    for o in doors:
        door(mesh, facade, o, leaf="glass", step="stone")
    return doors


def office(spec):
    """An office building: a glazed ground floor between piers, a band of glass along every
    storey between plain spandrels, a cornice in the accent colour round a flat roof with a
    plant room, and an entrance canopy."""
    rect = Rect(spec["length"], spec["width"])
    storeys = spec["storeys"]
    mesh = Mesh()
    ground_top = FLOOR + LOBBY
    roof_z = ground_top + (storeys - 1) * OFFICE_STOREY + PLATE
    facades = rect.facades()
    for facade in facades:
        wall(mesh, facade, -BASEMENT, FLOOR, "stone")
        doors = lobby(mesh, facade, FLOOR, ground_top,
                      entrance=3.2 if facade is facades[0] else None)
        ribbons = []
        for storey in range(storeys - 1):
            floor = ground_top + storey * OFFICE_STOREY
            ribbons.append(Opening(0.9, facade.length - 0.9, floor + 0.9, floor + 2.6, depth=0.22))
        wall(mesh, facade, ground_top, roof_z + PARAPET, "plaster", ribbons)
        for o in ribbons:
            window(mesh, facade, o, sill=None)
        for o in doors:
            canopy(mesh, facade, o.u0 - 1.2, o.u1 + 1.2, o.z1 + 0.35, 2.2, posts=False)
    band(mesh, rect, ground_top - 0.15, ground_top + 0.2, 0.12, "accent")
    top = flat_roof(mesh, rect, roof_z, PARAPET)
    roof_units(mesh, rect, roof_z, storeys, count=2 if rect.length < 30 else 3)
    return mesh, footprint(rect.length, rect.width, roof_z, top + 2.8, storeys=storeys)


def hotel(spec):
    """A hotel: a glazed lobby with a deep canopy over its entrance, balconies with French
    windows in every column of the long sides, windows on the ends, a cornice round a flat
    roof."""
    rect = Rect(spec["length"], spec["width"])
    storeys = spec["storeys"]
    mesh = Mesh()
    ground_top = FLOOR + LOBBY
    roof_z = ground_top + (storeys - 1) * STOREY + PLATE
    south, east, north, west = rect.facades()
    for facade in (south, east, north, west):
        long_side = facade in (south, north)
        wall(mesh, facade, -BASEMENT, FLOOR, "stone")
        doors = lobby(mesh, facade, FLOOR, ground_top,
                      entrance=2.8 if facade is south else None)
        centres = columns(facade.length, 3.2 if long_side else 3.0, 1.6)
        openings = []
        for storey in range(storeys - 1):
            floor = ground_top + storey * STOREY
            for u in centres:
                if long_side:
                    openings.append(Opening(u - 0.7, u + 0.7, floor + 0.1, floor + 2.3))
                else:
                    openings.append(Opening(u - 0.6, u + 0.6, floor + 0.85, floor + 2.25))
        wall(mesh, facade, ground_top, roof_z + PARAPET, "plaster", openings)
        for o in openings:
            window(mesh, facade, o, sill=None if long_side else "stone")
        if long_side:
            for storey in range(storeys - 1):
                floor = ground_top + storey * STOREY
                for u in centres:
                    slab(mesh, facade, u - 1.3, u + 1.3, floor + 0.1)
        for o in doors:
            canopy(mesh, facade, o.u0 - 1.0, o.u1 + 1.0, o.z1 + 0.3, 3.0)
    band(mesh, rect, ground_top - 0.2, ground_top + 0.15, 0.15, "accent")
    top = flat_roof(mesh, rect, roof_z, PARAPET)
    roof_units(mesh, rect, roof_z, storeys + 1, count=2)
    return mesh, footprint(rect.length, rect.width, roof_z, top + 2.8, storeys=storeys)


def slab(mesh, facade, u0, u1, floor, depth=1.3, rail=1.05):
    """A hotel balcony: a slab with a solid balustrade in the accent colour, as one box."""
    p = facade.point
    out = -depth
    bottom, top = floor - 0.2, floor + rail
    corners = {
        "front": [p(u0, bottom, out), p(u1, bottom, out), p(u1, top, out), p(u0, top, out)],
        "top": [p(u0, top, 0.0), p(u1, top, 0.0), p(u1, top, out), p(u0, top, out)],
        "bottom": [p(u0, bottom, 0.0), p(u0, bottom, out), p(u1, bottom, out),
                   p(u1, bottom, 0.0)],
    }
    mesh.facing(corners["front"], facade.out, "accent")
    mesh.facing(corners["top"], UP, "accent")
    mesh.facing(corners["bottom"], -UP, "accent")
    for u, side in ((u0, -1.0), (u1, 1.0)):
        mesh.facing([p(u, bottom, 0.0), p(u, bottom, out), p(u, top, out), p(u, top, 0.0)],
                    facade.along * side, "accent")


def public(spec):
    """A school, town hall or hospital: symmetrical, with an entrance bay standing out of the
    front under a canopy on columns, tall windows in rows, and a flag. Classic ones
    (`roof` hipped) have a stone ground floor, white trim and a hipped roof, the bay its own
    gable; modern ones (`roof` flat) a band of colour at every floor and a flat roof, the bay
    rising over it."""
    rect = Rect(spec["length"], spec["width"])
    storeys = spec["storeys"]
    classic = spec["roof"] == "hipped"
    mesh = Mesh()
    eaves = eaves_height(storeys)
    south, east, north, west = rect.facades()
    # The entrance bay: a box standing out of the front.
    bay_width = max(6.0, rect.length * 0.22)
    bay_out = 1.0
    bay = Rect(bay_width, bay_out * 2.0, (0.0, -rect.width / 2.0))
    for facade in (south, east, north, west):
        long_side = facade in (south, north)
        centres = columns(facade.length, 3.3 if long_side else 3.1, 2.0)
        openings = []
        for storey in range(storeys):
            floor = FLOOR + storey * STOREY
            for u in centres:
                if facade is south and abs(u - facade.length / 2.0) < bay_width / 2.0 + 0.6:
                    continue
                openings.append(Opening(u - 0.75, u + 0.75, floor + 0.6, floor + 2.45))
        wall(mesh, facade, -BASEMENT, FLOOR, "stone")
        top = eaves + (0.0 if classic else PARAPET)
        if classic:
            low = [o for o in openings if o.z0 < FLOOR + STOREY]
            high = [o for o in openings if o.z0 >= FLOOR + STOREY]
            wall(mesh, facade, FLOOR, FLOOR + STOREY, "stone", low)
            wall(mesh, facade, FLOOR + STOREY, top, "plaster", high)
        else:
            wall(mesh, facade, FLOOR, top, "plaster", openings)
        for o in openings:
            window(mesh, facade, o, sill="stone" if classic else None)
    # The bay: its own walls, windows over the entrance, the door under a canopy.
    bay_top = eaves + (0.0 if classic else PARAPET + 1.6)
    bay_south, bay_east, bay_north, bay_west = bay.facades()
    if not classic:
        # Where the bay rises over the roof, its back shows.
        wall(mesh, bay_north, eaves + PARAPET - 0.3, bay_top, "plaster")
    for facade in (bay_south, bay_east, bay_west):
        openings, doors = [], []
        if facade is bay_south:
            centre = facade.length / 2.0
            doors.append(Opening(centre - 1.4, centre + 1.4, FLOOR, FLOOR + 2.7, depth=0.3))
            for storey in range(1, storeys):
                floor = FLOOR + storey * STOREY
                for d in (-1.2, 1.2):
                    openings.append(Opening(centre + d - 0.55, centre + d + 0.55, floor + 0.5,
                                            floor + 2.55))
        wall(mesh, facade, -BASEMENT, bay_top, "stone" if classic else "plaster",
             openings + doors)
        for o in openings:
            window(mesh, facade, o, sill="stone" if classic else None)
        for o in doors:
            door(mesh, facade, o, leaf="glass" if not classic else "door", step="stone")
            canopy(mesh, facade, o.u0 - 1.6, o.u1 + 1.6, o.z1 + 0.5, 2.6,
                   material="frame" if classic else "accent", posts=False)
            columns_material = "stone" if classic else "metal"
            for u in (o.u0 - 1.3, o.u1 + 1.3):
                mesh.beam(facade.point(u, FLOOR, -2.3), facade.point(u, o.z1 + 0.5, -2.3), 0.4,
                          0.4, columns_material)
    if classic:
        band(mesh, rect, FLOOR + STOREY - 0.12, FLOOR + STOREY + 0.18, 0.1, "frame")
        band(mesh, rect, eaves - 0.35, eaves, 0.12, "frame")
        ridge = hipped_roof(mesh, rect, eaves, spec.get("pitch", 30.0), overhang=0.6,
                            under="plaster", fascia="frame")
        # A pediment over the bay: a small gable roof across the front, its gable facing out.
        pediment = Mesh()
        reach = bay_out + 1.0
        gable_roof(pediment, Rect(2.0 * reach, bay_width), eaves, spec.get("pitch", 30.0),
                   overhang=0.3, verge=0.3, under="plaster", fascia="frame")
        mesh.add(pediment, offset=(0.0, -rect.width / 2.0 - bay_out + reach, 0.0), turn=90.0)
        top = ridge
    else:
        for storey in range(1, storeys):
            z = FLOOR + storey * STOREY
            band(mesh, rect, z - 0.2, z + 0.2, 0.1, "accent")
        top = flat_roof(mesh, rect, eaves, PARAPET)
        flat_roof(mesh, bay, bay_top - PARAPET, PARAPET, cap="accent")
        roof_units(mesh, rect, eaves, storeys, count=2)
        top = max(top + 2.8, bay_top)
    flag(mesh, (bay_width / 2.0 + 2.5, -rect.width / 2.0 - 3.5, 0.0), 8.0)
    return mesh, footprint(rect.length, rect.width, eaves, top + 0.5, storeys=storeys)


# Castles and lighthouses stand on hills and rocks: their walls reach this far below the
# ground, and the world lets them stand on plots falling nearly as much (`DEEP_BASEMENT` in
# core/torqa-world/src/buildings/mod.rs).
DEEP = 8.0


def ring_points(centre, radius, sides, z, turn=0.0):
    """Corners of a regular polygon round `centre` at height `z`, counter-clockwise."""
    cx, cy = centre
    return [
        Vector((cx + radius * math.cos(turn + 2 * math.pi * k / sides),
                cy + radius * math.sin(turn + 2 * math.pi * k / sides), z))
        for k in range(sides)
    ]


def frustum(mesh, centre, r0, r1, z0, z1, sides, material, turn=0.0):
    """The faceted sides of an upright prism, tapering from radius `r0` at `z0` to `r1` at
    `z1` (a cone if `r1` is 0), without caps."""
    low = ring_points(centre, r0, sides, z0, turn)
    high = ring_points(centre, r1, sides, z1, turn)
    middle = Vector((centre[0], centre[1], 0.0))
    for k in range(sides):
        a, b = low[k], low[(k + 1) % sides]
        out = (a + b) / 2.0 - middle
        out.z = 0.0
        if r1 > 1e-6:
            mesh.facing([a, b, high[(k + 1) % sides], high[k]], out, material)
        else:
            mesh.facing([a, b, Vector((centre[0], centre[1], z1))], out + UP * 0.2, material)


def disc(mesh, centre, radius, sides, z, material, up=True, turn=0.0):
    """A level regular polygon facing up, or down."""
    mesh.facing(ring_points(centre, radius, sides, z, turn), UP if up else -UP, material)


def merlons(mesh, rect, z, height=0.8, width=0.9, spacing=1.7, thickness=0.5,
            material="plaster"):
    """Crenellations: blocks along the top of the walls of `rect`, standing on `z`."""
    for facade in rect.facades():
        count = max(2, int(facade.length / spacing))
        step = facade.length / count
        for k in range(count):
            u = (k + 0.5) * step
            a = facade.point(u - width / 2.0, z, -0.05)
            b = facade.point(u + width / 2.0, z, thickness)
            low = (min(a.x, b.x), min(a.y, b.y), z)
            high = (max(a.x, b.x), max(a.y, b.y), z + height)
            mesh.box(low, high, material, skip=("-z",))


def castle(spec):
    """A castle: a keep with crenellated walls round a steep hipped roof, round towers with
    pointed roofs at its corners, small arched windows and an arched gate."""
    rect = Rect(spec["length"], spec["width"])
    eaves = spec["eaves"]
    radius = spec["tower"]
    tower_top = spec["tower_height"]
    cover = spec.get("cover", "tiles")
    mesh = Mesh()
    south, east, north, west = rect.facades()
    for facade in (south, east, north, west):
        openings, gates = [], []
        if facade is south:
            middle = facade.length / 2.0
            gates.append(Opening(middle - 1.6, middle + 1.6, 0.0, 4.6, depth=0.8, arch=True))
        # Windows clear of the corner towers and the gate.
        for u in columns(facade.length - 2.0 * radius, 4.8, 2.0):
            u += radius
            if gates and abs(u - facade.length / 2.0) < 3.0:
                continue
            for z0 in (eaves * 0.45, eaves * 0.72):
                openings.append(Opening(u - 0.45, u + 0.45, z0, z0 + 1.6, depth=0.5,
                                        arch=True))
        wall(mesh, facade, -DEEP, 0.0, "stone")
        wall(mesh, facade, 0.0, eaves + 1.0, "plaster", openings + gates)
        for o in openings:
            window(mesh, facade, o, sill="stone")
        for g in gates:
            door(mesh, facade, g, leaf="wood_dark", step="stone")
    # The wall walk: the parapet's inner faces, its top, and the floor behind it.
    l, w = rect.length / 2.0, rect.width / 2.0
    walk = eaves + 1.0
    inner = Rect(rect.length - 1.0, rect.width - 1.0)
    for facade in inner.facades():
        a, b = facade.point(0.0, eaves), facade.point(facade.length, eaves)
        mesh.facing([a, b, b + UP * 1.0, a + UP * 1.0], -facade.out, "plaster")
    il, iw = l - 0.5, w - 0.5
    mesh.facing([(-il, -iw, eaves), (il, -iw, eaves), (il, iw, eaves), (-il, iw, eaves)], UP,
                "roof_flat")
    outer = [(-l, -w), (l, -w), (l, w), (-l, w)]
    edge = [(-il, -iw), (il, -iw), (il, iw), (-il, iw)]
    for k in range(4):
        j = (k + 1) % 4
        mesh.facing([(*outer[k], walk), (*outer[j], walk), (*edge[j], walk), (*edge[k], walk)],
                    UP, "plaster")
    merlons(mesh, rect, walk)
    ridge = hipped_roof(mesh, Rect(rect.length - 1.6, rect.width - 1.6), eaves + 0.2,
                        spec.get("pitch", 48.0), overhang=0.0, roof=cover, under="plaster",
                        fascia="plaster")
    # Corner towers: eight-sided, a corbelled ring under a pointed roof.
    sides = 8
    turn = math.pi / sides
    spire = radius * spec.get("spire", 2.2)
    apothem = math.cos(math.pi / sides)
    for sx in (-1.0, 1.0):
        for sy in (-1.0, 1.0):
            centre = (sx * l, sy * w)
            frustum(mesh, centre, radius, radius, -DEEP, 0.0, sides, "stone", turn)
            frustum(mesh, centre, radius, radius * 0.94, 0.0, tower_top, sides, "plaster", turn)
            frustum(mesh, centre, radius * 0.94, radius + 0.35, tower_top, tower_top + 0.6,
                    sides, "stone", turn)
            frustum(mesh, centre, radius + 0.35, radius + 0.35, tower_top + 0.6,
                    tower_top + 1.0, sides, "stone", turn)
            disc(mesh, centre, radius + 0.35, sides, tower_top + 1.0, "stone", turn=turn)
            frustum(mesh, centre, radius + 0.5, 0.0, tower_top + 1.0, tower_top + 1.0 + spire,
                    sides, cover, turn)
            disc(mesh, centre, radius + 0.5, sides, tower_top + 1.0, "plaster", up=False,
                 turn=turn)
            tip = Vector((centre[0], centre[1], tower_top + 1.0 + spire))
            mesh.beam(tip - UP * 0.4, tip + UP * 1.2, 0.08, 0.08, "metal")
            # Slit windows looking out of the tower's outer face.
            out = Vector((sx, sy, 0.0)).normalized()
            left = Vector((-out.y, out.x, 0.0))
            for z in (tower_top * 0.45, tower_top * 0.75):
                reach = apothem * radius * (1.0 - 0.06 * z / tower_top) + 0.03
                face_centre = Vector((centre[0], centre[1], 0.0)) + out * reach
                mesh.facing([face_centre - left * 0.2 + UP * z, face_centre + left * 0.2 + UP * z,
                             face_centre + left * 0.2 + UP * (z + 1.3),
                             face_centre - left * 0.2 + UP * (z + 1.3)], out, "glass")
    top = max(ridge, tower_top + 1.0 + spire + 1.2)
    return mesh, footprint(rect.length, rect.width, eaves, top, tower=radius)


def lighthouse(spec):
    """A lighthouse: a round tower tapering up in bands of white and colour from a stone
    plinth, a door and a few small windows, a gallery with a solid railing round a glazed
    lantern under a pointed cap."""
    r0, r1 = spec["diameter"] / 2.0, spec["top"] / 2.0
    gallery = spec["height"]
    bands = spec.get("bands", 3)
    sides = 12
    turn = math.pi / sides
    mesh = Mesh()
    centre = (0.0, 0.0)

    def radius(z):
        return r0 + (r1 - r0) * max(0.0, z) / gallery

    frustum(mesh, centre, r0 + 0.4, r0 + 0.4, -DEEP, 1.0, sides, "stone", turn)
    disc(mesh, centre, r0 + 0.4, sides, 1.0, "stone", turn=turn)
    # Bands from the plinth to the gallery: white, colour, white, … ending white.
    count = 2 * bands + 1
    step = (gallery - 1.0) / count
    for k in range(count):
        z0, z1 = 1.0 + k * step, 1.0 + (k + 1) * step
        frustum(mesh, centre, radius(z0), radius(z1), z0, z1, sides,
                "accent" if k % 2 else "plaster", turn)

    def on_face(k, z0, z1, half, material):
        """A flat panel (door, window) on face `k` of the tower from `z0` to `z1`."""
        phi = turn + 2 * math.pi * (k + 0.5) / sides
        out = Vector((math.cos(phi), math.sin(phi), 0.0))
        side = Vector((-out.y, out.x, 0.0))
        apothem = math.cos(math.pi / sides)
        low = out * (radius(z0) * apothem + 0.04) + UP * z0
        high = out * (radius(z1) * apothem + 0.04) + UP * z1
        mesh.facing([low - side * half, low + side * half, high + side * half,
                     high - side * half], out, material)

    # The door towards −y, small windows up the tower on other sides.
    front = sides * 3 // 4 - 1
    on_face(front, 1.0, 3.2, 0.55, "door")
    for k, share in enumerate((0.3, 0.55, 0.8)):
        on_face((front + 4 * (k + 1)) % sides, gallery * share, gallery * share + 1.0, 0.3,
                "glass")
    # The gallery: a slab reaching out, a solid railing round it.
    out_r = r1 + 0.9
    deck = gallery + 0.35
    frustum(mesh, centre, out_r, out_r, gallery, deck, sides, "stone", turn)
    disc(mesh, centre, out_r, sides, gallery, "stone", up=False, turn=turn)
    disc(mesh, centre, out_r, sides, deck, "stone", turn=turn)
    rail = deck + 1.0
    frustum(mesh, centre, out_r, out_r, deck, rail, sides, "metal", turn)
    inner = ring_points(centre, out_r - 0.1, sides, deck, turn)
    inner_top = ring_points(centre, out_r - 0.1, sides, rail, turn)
    outer_top = ring_points(centre, out_r, sides, rail, turn)
    for k in range(sides):
        j = (k + 1) % sides
        inward = -(inner[k] + inner[j]) / 2.0
        inward.z = 0.0
        mesh.facing([inner[k], inner[j], inner_top[j], inner_top[k]], inward, "metal")
        mesh.facing([outer_top[k], outer_top[j], inner_top[j], inner_top[k]], UP, "metal")
    # The lantern: a low metal wall, glass, a cap in the band colour, a ball and a rod.
    lantern = r1 * 0.72
    lantern_sides = 10
    frustum(mesh, centre, lantern, lantern, deck, deck + 0.7, lantern_sides, "metal")
    frustum(mesh, centre, lantern, lantern, deck + 0.7, deck + 2.5, lantern_sides, "glass")
    frustum(mesh, centre, lantern + 0.15, lantern + 0.15, deck + 2.5, deck + 2.75,
            lantern_sides, "metal")
    disc(mesh, centre, lantern + 0.15, lantern_sides, deck + 2.5, "metal", up=False)
    cap = lantern * 1.6
    frustum(mesh, centre, lantern + 0.2, 0.0, deck + 2.75, deck + 2.75 + cap, lantern_sides,
            "accent")
    disc(mesh, centre, lantern + 0.2, lantern_sides, deck + 2.75, "metal", up=False)
    tip = Vector((0.0, 0.0, deck + 2.75 + cap))
    mesh.sphere(tip + UP * 0.15, 0.22, "metal", rings=3, segments=6)
    mesh.beam(tip, tip + UP * 1.1, 0.07, 0.07, "metal")
    return mesh, footprint(2.0 * r0, 2.0 * r0, gallery, tip.z + 1.1)

