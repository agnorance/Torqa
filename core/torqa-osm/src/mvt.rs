//! Decoding `OpenMapTiles` vector tiles into map features.
//! Schema: <https://openmaptiles.org/schema/>.

use std::collections::HashMap;
use std::f64::consts::PI;

use geo_types::{Geometry, LineString, Polygon};
use mvt_reader::Reader;
use mvt_reader::feature::Value;

use crate::{
    Area, Building, LandCover, LatLon, MapData, OsmError, Railway, Road, RoadClass, StructureKind,
    Waterway, ZOOM,
};

/// Adds the features of tile (x, y) to `data`.
pub(crate) fn merge(
    (x, y): (u32, u32),
    bytes: Vec<u8>,
    data: &mut MapData,
) -> Result<(), OsmError> {
    let invalid = |e: mvt_reader::error::ParserError| OsmError::Invalid(e.to_string());
    let reader = Reader::new(bytes).map_err(invalid)?;
    for layer in reader.get_layer_metadata().map_err(invalid)? {
        let projection = TileProjection {
            x: f64::from(x),
            y: f64::from(y),
            extent: f64::from(layer.extent),
        };
        for feature in reader
            .get_features_as::<f64>(layer.layer_index)
            .map_err(invalid)?
        {
            let tags = Tags(feature.properties.unwrap_or_default());
            let geometry = &feature.geometry;
            match layer.name.as_str() {
                "building" => add_building(data, &tags, geometry, &projection),
                "landcover" | "landuse" | "park" | "water" => {
                    add_area(data, &layer.name, &tags, geometry, &projection);
                }
                "waterway" => add_waterway(data, &tags, geometry, &projection),
                "transportation" => add_road(data, &tags, geometry, &projection),
                // Other religions' buildings would need other shapes; unknown ones are mostly
                // chapels where churches are.
                "poi"
                    if tags.text("class") == "place_of_worship"
                        && matches!(tags.text("subclass"), "christian" | "place_of_worship") =>
                {
                    for point in points(geometry) {
                        if projection.owns(point) {
                            data.churches.push(projection.point(point.0, point.1));
                        }
                    }
                }
                "poi" if castle(tags.text("class"), tags.text("subclass")) => {
                    for point in points(geometry) {
                        if projection.owns(point) {
                            data.castles.push(projection.point(point.0, point.1));
                        }
                    }
                }
                "poi" if lighthouse(&tags) => {
                    for point in points(geometry) {
                        if projection.owns(point) {
                            data.lighthouses.push(projection.point(point.0, point.1));
                        }
                    }
                }
                "poi" => {
                    let list = match tags.text("class") {
                        class if shop(class) => &mut data.shops,
                        "lodging" if hotel(tags.text("subclass")) => &mut data.hotels,
                        "office" => &mut data.offices,
                        class if public(class, tags.text("subclass")) => &mut data.public,
                        _ => continue,
                    };
                    for point in points(geometry) {
                        if projection.owns(point) {
                            list.push(projection.point(point.0, point.1));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Whether a point of interest of this class is a shop, café, restaurant or the like: a
/// business with a front onto the street.
fn shop(class: &str) -> bool {
    matches!(
        class,
        "shop"
            | "grocery"
            | "bakery"
            | "butcher"
            | "clothing_store"
            | "shoe"
            | "jewelry"
            | "books"
            | "music"
            | "toys"
            | "gift"
            | "florist"
            | "optician"
            | "pharmacy"
            | "hairdresser"
            | "laundry"
            | "bicycle"
            | "alcohol_shop"
            | "beer"
            | "restaurant"
            | "cafe"
            | "bar"
            | "fast_food"
            | "ice_cream"
            | "bank"
    )
}

/// Whether lodging of this subclass is a hotel-like building (not a chalet or a campsite).
fn hotel(subclass: &str) -> bool {
    matches!(subclass, "hotel" | "guest_house" | "hostel" | "motel")
}

/// Whether a point of interest marks a public building: a school, hospital, town hall and the
/// like.
fn public(class: &str, subclass: &str) -> bool {
    match class {
        "school" | "college" | "library" | "police" | "fire_station" | "town_hall" => true,
        "hospital" => subclass == "hospital",
        "post" => subclass == "post_office",
        _ => false,
    }
}

/// Whether a point of interest is a castle: `OpenMapTiles` puts `historic=castle` and
/// `historic=ruins` in class `castle`; ruins (mostly a few walls) are left out.
fn castle(class: &str, subclass: &str) -> bool {
    class == "castle" && subclass == "castle"
}

/// Words in names that make a point a lighthouse wherever they appear, also inside compounds
/// ("Leuchtturm Westerheversand", "Hornbæk Fyrtårn", "観音埼灯台").
const LIGHTHOUSE_PARTS: [&str; 12] = [
    "lighthouse",
    "leuchtturm",
    "leuchtfeuer",
    "vuurtoren",
    "fyrtårn",
    "fyrtorn",
    "灯台",
    "燈台",
    "灯塔",
    "燈塔",
    "등대",
    "φάρος",
];
/// Words that make a point a lighthouse only standing alone ("Faro de Cabo Mayor", "Phare
/// du Créac'h"), since they are also parts of other words.
const LIGHTHOUSE_WORDS: [&str; 8] = [
    "phare", "faro", "farol", "fyr", "majak", "маяк", "fener", "latarnia",
];

/// Whether a point of interest is a lighthouse. `OpenMapTiles` has no class for
/// `man_made=lighthouse`; lighthouses show up as attractions (`tourism=attraction`) or museums
/// named as lighthouses, so the name decides. A `lighthouse` subclass, should the schema gain
/// one, counts too. Bus stops and information boards named after a lighthouse do not.
fn lighthouse(tags: &Tags) -> bool {
    if tags.text("subclass") == "lighthouse" {
        return true;
    }
    if !matches!(tags.text("class"), "attraction" | "museum") {
        return false;
    }
    tags.0.iter().any(|(key, value)| match value {
        Value::String(name) if key.starts_with("name") => lighthouse_name(name),
        _ => false,
    })
}

/// Whether a name says its feature is a lighthouse.
fn lighthouse_name(name: &str) -> bool {
    let name = name.to_lowercase();
    LIGHTHOUSE_PARTS.iter().any(|part| name.contains(part))
        || name
            .split(|c: char| !c.is_alphanumeric())
            .any(|word| LIGHTHOUSE_WORDS.contains(&word))
}

/// Feature properties.
struct Tags(HashMap<String, Value>);

impl Tags {
    fn text(&self, key: &str) -> &str {
        match self.0.get(key) {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        }
    }

    fn number(&self, key: &str) -> Option<f64> {
        // Tag values are small (heights in metres); f64 holds them exactly.
        #[allow(clippy::cast_precision_loss)]
        match self.0.get(key) {
            Some(Value::Int(v) | Value::SInt(v)) => Some(*v as f64),
            Some(Value::UInt(v)) => Some(*v as f64),
            Some(Value::Float(v)) => Some(f64::from(*v)),
            Some(Value::Double(v)) => Some(*v),
            _ => None,
        }
    }
}

/// The height `OpenMapTiles` gives buildings without a height or levels tag.
const DEFAULT_RENDER_HEIGHT: f64 = 5.0;

fn add_building(
    data: &mut MapData,
    tags: &Tags,
    geometry: &Geometry<f64>,
    projection: &TileProjection,
) {
    let height = tags
        .number("render_height")
        .filter(|&h| h > 0.0 && (h - DEFAULT_RENDER_HEIGHT).abs() > f64::EPSILON);
    let color = color(tags.text("colour"));
    // Buildings with equal tags arrive merged into one feature, so neither the feature nor
    // its id stands for a single building: each polygon is one.
    for polygon in polygons(geometry) {
        let Some(centre) = bounds_centre(polygon.exterior()) else {
            continue;
        };
        if !projection.owns(centre) {
            continue;
        }
        data.buildings.push(Building {
            id: projection.id(centre),
            outline: projection.ring(polygon.exterior()),
            height,
            levels: None,
            color,
        });
    }
}

/// The centre of a ring's bounding box, in tile pixels.
fn bounds_centre(ring: &LineString<f64>) -> Option<(f64, f64)> {
    let first = ring.0.first()?;
    let (mut min, mut max) = ((first.x, first.y), (first.x, first.y));
    for c in &ring.0 {
        min = (min.0.min(c.x), min.1.min(c.y));
        max = (max.0.max(c.x), max.1.max(c.y));
    }
    Some((f64::midpoint(min.0, max.0), f64::midpoint(min.1, max.1)))
}

/// A mapped façade colour (`building:colour`, or a material's colour): `#rrggbb`, `#rgb` or a
/// name. Named colours are the muted tones façades have rather than pure CSS colours.
fn color(value: &str) -> Option<[f32; 3]> {
    let value = value.trim().to_ascii_lowercase();
    let named = match value.as_str() {
        "white" => Some([0.93, 0.93, 0.91]),
        "black" => Some([0.12, 0.12, 0.12]),
        "grey" | "gray" => Some([0.55, 0.55, 0.55]),
        "silver" | "lightgrey" | "lightgray" => Some([0.75, 0.75, 0.75]),
        "red" => Some([0.62, 0.22, 0.18]),
        "maroon" | "darkred" => Some([0.45, 0.15, 0.12]),
        "brown" => Some([0.48, 0.33, 0.22]),
        "beige" => Some([0.89, 0.84, 0.72]),
        "cream" => Some([0.95, 0.91, 0.78]),
        "yellow" => Some([0.93, 0.83, 0.45]),
        "orange" => Some([0.90, 0.60, 0.30]),
        "pink" => Some([0.92, 0.70, 0.70]),
        "green" => Some([0.45, 0.60, 0.42]),
        "blue" => Some([0.40, 0.52, 0.70]),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let hex = value.strip_prefix('#')?;
    let digits: Vec<u8> = hex
        .chars()
        .map(|c| c.to_digit(16).and_then(|d| u8::try_from(d).ok()))
        .collect::<Option<_>>()?;
    let channels = match digits.as_slice() {
        [r, g, b] => [r * 17, g * 17, b * 17],
        [r1, r2, g1, g2, b1, b2] => [r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2],
        _ => return None,
    };
    Some(channels.map(|c| f32::from(c) / 255.0))
}

fn add_area(
    data: &mut MapData,
    layer: &str,
    tags: &Tags,
    geometry: &Geometry<f64>,
    projection: &TileProjection,
) {
    let Some(cover) = land_cover(layer, tags.text("class")) else {
        return;
    };
    for polygon in polygons(geometry) {
        data.areas.push(Area {
            cover,
            outer: vec![projection.ring(polygon.exterior())],
            inner: polygon
                .interiors()
                .iter()
                .map(|ring| projection.ring(ring))
                .collect(),
        });
    }
}

fn add_waterway(
    data: &mut MapData,
    tags: &Tags,
    geometry: &Geometry<f64>,
    projection: &TileProjection,
) {
    // Culverts and tunnels carry the water underground, under roads and towns.
    if tags.text("brunnel") == "tunnel" {
        return;
    }
    let width = match tags.text("class") {
        "river" => 12.0,
        "canal" => 8.0,
        _ => 3.0,
    };
    for line in lines(geometry) {
        data.waterways.push(Waterway {
            width,
            line: projection.line(line),
        });
    }
}

fn add_road(
    data: &mut MapData,
    tags: &Tags,
    geometry: &Geometry<f64>,
    projection: &TileProjection,
) {
    let class = road_class(
        tags.text("class"),
        tags.text("subclass"),
        tags.text("bicycle"),
    );
    let kind = match tags.text("brunnel") {
        "bridge" => Some(StructureKind::Bridge),
        "tunnel" => Some(StructureKind::Tunnel),
        _ => None,
    };
    let railway = drawn_railway(tags.text("class"), tags.text("subclass"));
    let funicular = tags.text("subclass") == "funicular";
    for line in lines(geometry)
        .into_iter()
        .flat_map(|l| projection.own_parts(l))
    {
        if railway {
            data.railways.push(Railway {
                line: line.clone(),
                structure: kind,
                funicular,
            });
        }
        if let Some(class) = class {
            data.roads.push(Road {
                class,
                line,
                structure: kind,
            });
        }
    }
}

/// What a way of the transportation layer is to Torqa, from its `class`, `subclass` and
/// `bicycle` access. `None` for railways, ferries and ways for pedestrians only: footways and
/// mapped sidewalks, pedestrian zones, steps, platforms and paths closed to bikes. Those carry
/// no ride and do not belong among the streets drawn; footways open to bikes stay as paths.
/// Whether a transportation feature is a railway drawn in the world: main lines, narrow gauge,
/// funiculars and light rail; trams run in the streets, subways underground.
fn drawn_railway(class: &str, subclass: &str) -> bool {
    match class {
        "rail" => subclass != "subway",
        "transit" => matches!(subclass, "light_rail" | "monorail"),
        _ => false,
    }
}

fn road_class(class: &str, subclass: &str, bicycle: &str) -> Option<RoadClass> {
    let bikes = matches!(bicycle, "yes" | "designated" | "permissive");
    match (class, subclass) {
        ("motorway" | "trunk" | "primary" | "secondary", _) => Some(RoadClass::Major),
        ("tertiary" | "minor", _) => Some(RoadClass::Street),
        ("service", _) => Some(RoadClass::Service),
        ("track", _) => Some(RoadClass::Track),
        ("path", "steps" | "platform" | "corridor") => None,
        ("path", "footway" | "pedestrian") => bikes.then_some(RoadClass::Path),
        // Cycleways and trails: routes often follow them.
        ("path", _) => (bicycle != "no").then_some(RoadClass::Path),
        _ => None,
    }
}

fn land_cover(layer: &str, class: &str) -> Option<LandCover> {
    match (layer, class) {
        ("landcover", "wood") => Some(LandCover::Forest),
        ("landcover", "grass") | ("park", _) => Some(LandCover::Meadow),
        ("landcover", "farmland") => Some(LandCover::Farmland),
        ("landcover", "rock" | "ice" | "sand") => Some(LandCover::Rock),
        ("landuse", "residential" | "suburb") => Some(LandCover::Residential),
        ("landuse", "industrial") => Some(LandCover::Industrial),
        ("landuse", "commercial" | "retail") => Some(LandCover::Commercial),
        ("landuse", "school" | "college" | "university" | "kindergarten" | "hospital") => {
            Some(LandCover::Public)
        }
        ("water", _) => Some(LandCover::Water),
        _ => None,
    }
}

fn points(geometry: &Geometry<f64>) -> Vec<(f64, f64)> {
    match geometry {
        Geometry::Point(p) => vec![(p.x(), p.y())],
        Geometry::MultiPoint(m) => m.iter().map(|p| (p.x(), p.y())).collect(),
        _ => Vec::new(),
    }
}

fn polygons(geometry: &Geometry<f64>) -> Vec<&Polygon<f64>> {
    match geometry {
        Geometry::Polygon(polygon) => vec![polygon],
        Geometry::MultiPolygon(multi) => multi.0.iter().collect(),
        _ => Vec::new(),
    }
}

fn lines(geometry: &Geometry<f64>) -> Vec<&LineString<f64>> {
    match geometry {
        Geometry::LineString(line) => vec![line],
        Geometry::MultiLineString(multi) => multi.0.iter().collect(),
        _ => Vec::new(),
    }
}

/// Converts tile pixel coordinates (y down) to latitude/longitude.
struct TileProjection {
    x: f64,
    y: f64,
    extent: f64,
}

impl TileProjection {
    fn point(&self, px: f64, py: f64) -> LatLon {
        let n = f64::from(1u32 << ZOOM);
        let lon = (self.x + px / self.extent) / n * 360.0 - 180.0;
        let mercator = PI * (1.0 - 2.0 * (self.y + py / self.extent) / n);
        (mercator.sinh().atan().to_degrees(), lon)
    }

    fn line(&self, line: &LineString<f64>) -> Vec<LatLon> {
        line.0.iter().map(|c| self.point(c.x, c.y)).collect()
    }

    /// The parts of `line` (tile pixels) within the tile itself, as latitude/longitude. Tiles
    /// repeat their neighbours' ways in a buffer around them, cut at its edge: two copies of a
    /// way crossing a tile border would overlap there, each ending somewhere along the other
    /// (#116, #117), where a bridge would get a deck down to the ground mid-span and two decks
    /// would cross. Cut at the border instead, the neighbours' parts meet end to end there.
    fn own_parts(&self, line: &LineString<f64>) -> Vec<Vec<LatLon>> {
        let mut parts: Vec<Vec<(f64, f64)>> = Vec::new();
        let mut part: Vec<(f64, f64)> = Vec::new();
        for pair in line.0.windows(2) {
            let (from, to) = ((pair[0].x, pair[0].y), (pair[1].x, pair[1].y));
            let Some((t0, t1)) = clip(from, to, self.extent) else {
                parts.push(std::mem::take(&mut part));
                continue;
            };
            // The points themselves where they lie inside, so the parts of a line run on
            // exactly from one segment to the next.
            let at = |share: f64| {
                (
                    from.0 + (to.0 - from.0) * share,
                    from.1 + (to.1 - from.1) * share,
                )
            };
            let start = if t0 > 0.0 { at(t0) } else { from };
            let end = if t1 < 1.0 { at(t1) } else { to };
            if part.last() != Some(&start) {
                parts.push(std::mem::take(&mut part));
                part.push(start);
            }
            part.push(end);
        }
        parts.push(part);
        parts
            .into_iter()
            .filter(|points| points.len() >= 2)
            .map(|points| points.iter().map(|&(x, y)| self.point(x, y)).collect())
            .collect()
    }

    /// Whether a feature centred at `(x, y)` (tile pixels) is this tile's to keep. Tiles repeat
    /// features near their borders. Copies of a building are identical unless it is so large
    /// that both tiles cut it, so exactly one tile contains their centre; copies of such
    /// large buildings may both be kept, but none is ever lost.
    fn owns(&self, (x, y): (f64, f64)) -> bool {
        (0.0..self.extent).contains(&x) && (0.0..self.extent).contains(&y)
    }

    /// A stable id for the building centred at `(x, y)`, from its place in the world-wide
    /// pixel grid (doubled, as centres fall on half pixels).
    #[allow(clippy::cast_possible_truncation)] // below 2^28 at zoom 14
    fn id(&self, (x, y): (f64, f64)) -> i64 {
        let global = |tile: f64, pixel: f64| ((tile * self.extent + pixel) * 2.0).round() as i64;
        (global(self.x, x) << 32) | global(self.y, y)
    }

    /// A closed ring.
    fn ring(&self, ring: &LineString<f64>) -> Vec<LatLon> {
        let mut points = self.line(ring);
        if points.first() != points.last()
            && let Some(&first) = points.first()
        {
            points.push(first);
        }
        points
    }
}

/// The part of segment `a`–`b` (tile pixels) within the tile `0..=extent` square, as positions
/// along it (0–1), if any (Liang–Barsky).
fn clip(a: (f64, f64), b: (f64, f64), extent: f64) -> Option<(f64, f64)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-dx, a.0),
        (dx, extent - a.0),
        (-dy, a.1),
        (dy, extent - a.1),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    (t0 < t1).then_some((t0, t1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tile() -> MapData {
        let mut data = MapData::default();
        merge(
            (8531, 5767),
            include_bytes!("../tests/data/14_8531_5767.pbf").to_vec(),
            &mut data,
        )
        .unwrap();
        data
    }

    #[test]
    fn tile_coordinates_map_to_the_tile_area() {
        let projection = TileProjection {
            x: 8531.0,
            y: 5767.0,
            extent: 4096.0,
        };
        let (north, west) = projection.point(0.0, 0.0);
        let (south, east) = projection.point(4096.0, 4096.0);

        // Tile 14/8531/5767 spans about 46.92–46.94 °N, 7.44–7.47 °E.
        assert!(north > south && east > west);
        assert!((46.91..46.95).contains(&south) && (46.91..46.95).contains(&north));
        assert!((7.43..7.48).contains(&west) && (7.43..7.48).contains(&east));
    }

    #[test]
    fn decodes_buildings_with_heights_and_closed_outlines() {
        let data = test_tile();

        assert!(data.buildings.len() > 50, "{}", data.buildings.len());
        assert!(
            data.buildings
                .iter()
                .all(|b| b.outline.first() == b.outline.last())
        );
        assert!(data.buildings.iter().any(|b| b.height.is_some()));
    }

    #[test]
    fn ways_for_pedestrians_only_are_left_out() {
        // Sidewalks, footpaths, steps, platforms, pedestrian zones, paths closed to bikes.
        for (subclass, bicycle) in [
            ("footway", ""),
            ("footway", "no"),
            ("pedestrian", ""),
            ("steps", "yes"),
            ("platform", ""),
            ("corridor", ""),
            ("path", "no"),
        ] {
            assert_eq!(road_class("path", subclass, bicycle), None, "{subclass}");
        }
        // Shared foot and cycle paths, cycleways and trails stay.
        for (subclass, bicycle) in [
            ("footway", "yes"),
            ("footway", "designated"),
            ("cycleway", ""),
            ("path", ""),
            ("path", "designated"),
        ] {
            assert_eq!(
                road_class("path", subclass, bicycle),
                Some(RoadClass::Path),
                "{subclass} {bicycle}"
            );
        }
        assert_eq!(road_class("minor", "", ""), Some(RoadClass::Street));
        assert_eq!(road_class("rail", "rail", ""), None);
    }

    #[test]
    fn hotels_offices_and_public_buildings_are_told_apart() {
        for subclass in ["hotel", "guest_house", "hostel", "motel"] {
            assert!(hotel(subclass), "{subclass}");
        }
        // Chalets and campsites to rent are no hotel buildings.
        assert!(!hotel("chalet") && !hotel("camp_site"));
        for (class, subclass) in [
            ("school", "school"),
            ("school", "kindergarten"),
            ("college", "university"),
            ("hospital", "hospital"),
            ("town_hall", "townhall"),
            ("police", "police"),
            ("post", "post_office"),
            ("library", "library"),
            ("fire_station", "fire_station"),
        ] {
            assert!(public(class, subclass), "{class} / {subclass}");
        }
        // Clinics and post boxes are not.
        assert!(!public("hospital", "clinic") && !public("post", "post_box"));
        assert!(!public("office", "company"));
    }

    #[test]
    fn castles_and_lighthouses_are_found_among_the_points_of_interest() {
        assert!(castle("castle", "castle"));
        // Ruins are mostly a few walls; forts and the rest are no castle class at all.
        assert!(!castle("castle", "ruins") && !castle("attraction", "castle"));

        let poi = |class: &str, name_key: &str, name: &str| {
            Tags(HashMap::from([
                ("class".to_owned(), Value::String(class.to_owned())),
                ("subclass".to_owned(), Value::String(class.to_owned())),
                (name_key.to_owned(), Value::String(name.to_owned())),
            ]))
        };
        // As OpenFreeMap has them: attractions named as lighthouses, in any language.
        for (key, name) in [
            ("name", "Leuchtturm Westerheversand"),
            ("name_en", "Dornbusch Lighthouse"),
            ("name", "Faro de Cabo Mayor"),
            ("name", "Phare du Créac'h"),
            ("name", "観音埼灯台"),
            ("name:da", "Hornbæk Fyrtårn"),
        ] {
            assert!(lighthouse(&poi("attraction", key, name)), "{name}");
        }
        // Not other attractions, nor bus stops or boards named after a lighthouse, nor words
        // that merely contain one of the short words.
        assert!(!lighthouse(&poi("attraction", "name", "Schloss Thun")));
        assert!(!lighthouse(&poi("bus", "name", "Leuchtturm")));
        assert!(!lighthouse(&poi(
            "information",
            "name",
            "Leuchtturm Dornbusch"
        )));
        assert!(!lighthouse(&poi("attraction", "name", "Pharmacy Garden")));
        assert!(!lighthouse(&poi("attraction", "name", "Фыркино")));
    }

    #[test]
    fn commercial_and_public_land_are_told_from_industrial() {
        assert_eq!(
            land_cover("landuse", "industrial"),
            Some(LandCover::Industrial)
        );
        for class in ["commercial", "retail"] {
            assert_eq!(land_cover("landuse", class), Some(LandCover::Commercial));
        }
        for class in [
            "school",
            "college",
            "university",
            "kindergarten",
            "hospital",
        ] {
            assert_eq!(
                land_cover("landuse", class),
                Some(LandCover::Public),
                "{class}"
            );
        }
    }

    #[test]
    fn decodes_land_cover_roads_and_water() {
        let data = test_tile();

        assert!(data.areas.iter().any(|a| a.cover == LandCover::Forest));
        assert!(data.areas.iter().any(|a| a.cover == LandCover::Residential));
        assert!(data.roads.iter().any(|r| !r.major()));
        assert!(
            !data.waterways.is_empty() || data.areas.iter().any(|a| a.cover == LandCover::Water)
        );
        // The Gürbetal line runs through Wabern.
        assert_ne!(data.railways.len(), 0, "no railways");
    }

    #[test]
    fn buildings_merged_into_one_feature_each_get_their_own_id() {
        let data = test_tile();

        // The tile merges hundreds of untagged houses into one feature; they must still vary
        // (only outlines mapped twice share an id).
        let ids: std::collections::HashSet<i64> = data.buildings.iter().map(|b| b.id).collect();
        assert!(ids.len() * 100 > data.buildings.len() * 99, "{}", ids.len());
    }

    #[test]
    fn untagged_buildings_have_no_height() {
        let data = test_tile();

        // OpenMapTiles fills in 5 m where nothing is tagged; that is not a known height.
        let unknown = data.buildings.iter().filter(|b| b.height.is_none()).count();
        assert!(unknown > data.buildings.len() / 2, "{unknown}");
        assert!(data.buildings.iter().all(|b| b.height != Some(5.0)));
    }

    #[test]
    fn decodes_churches_shops_and_mapped_colours() {
        let data = test_tile();

        // Kirche Wabern and St. Michael; the Petruskirche in the tile's margin belongs to the
        // next tile.
        assert_eq!(data.churches.len(), 2);
        // Wabern's restaurants, supermarkets, bakeries, hairdressers and the like.
        assert!(data.shops.len() > 30, "{} shops", data.shops.len());
        assert!(data.buildings.iter().any(|b| b.color.is_some()));
    }

    #[test]
    fn streams_in_culverts_and_tunnels_are_left_out() {
        let projection = TileProjection {
            x: 8531.0,
            y: 5767.0,
            extent: 4096.0,
        };
        let stream = Geometry::LineString(LineString::from(vec![(100.0, 100.0), (200.0, 100.0)]));
        let tags = |brunnel: &str| {
            Tags(HashMap::from([
                ("class".to_owned(), Value::String("stream".to_owned())),
                ("brunnel".to_owned(), Value::String(brunnel.to_owned())),
            ]))
        };
        let mut data = MapData::default();

        add_waterway(&mut data, &tags("tunnel"), &stream, &projection);
        assert!(data.waterways.is_empty(), "culvert drawn");
        add_waterway(&mut data, &tags("bridge"), &stream, &projection);
        add_waterway(&mut data, &tags(""), &stream, &projection);
        assert_eq!(
            data.waterways.len(),
            2,
            "open streams and aqueducts are drawn"
        );
    }

    #[test]
    fn railways_are_kept_with_their_bridges_and_tunnels_but_not_trams_or_subways() {
        let projection = TileProjection {
            x: 8531.0,
            y: 5767.0,
            extent: 4096.0,
        };
        let line = Geometry::LineString(LineString::from(vec![(100.0, 100.0), (200.0, 100.0)]));
        let tags = |class: &str, subclass: &str, brunnel: &str| {
            Tags(HashMap::from([
                ("class".to_owned(), Value::String(class.to_owned())),
                ("subclass".to_owned(), Value::String(subclass.to_owned())),
                ("brunnel".to_owned(), Value::String(brunnel.to_owned())),
            ]))
        };
        let mut data = MapData::default();
        for (class, subclass, brunnel) in [
            ("rail", "rail", ""),
            ("rail", "narrow_gauge", "bridge"),
            ("rail", "rail", "tunnel"),
            ("rail", "funicular", ""),
            ("transit", "light_rail", ""),
        ] {
            add_road(
                &mut data,
                &tags(class, subclass, brunnel),
                &line,
                &projection,
            );
        }
        let structures: Vec<Option<StructureKind>> =
            data.railways.iter().map(|r| r.structure).collect();
        assert_eq!(
            structures,
            [
                None,
                Some(StructureKind::Bridge),
                Some(StructureKind::Tunnel),
                None,
                None
            ]
        );
        let funiculars: Vec<bool> = data.railways.iter().map(|r| r.funicular).collect();
        assert_eq!(funiculars, [false, false, false, true, false]);
        for (class, subclass, brunnel) in [
            ("transit", "tram", ""),
            ("transit", "subway", ""),
            ("minor", "", ""),
        ] {
            add_road(
                &mut data,
                &tags(class, subclass, brunnel),
                &line,
                &projection,
            );
        }
        assert_eq!(
            data.railways.len(),
            5,
            "trams, subways and roads are no railways"
        );
        assert_eq!(data.roads.len(), 1);
    }

    #[test]
    fn ways_crossing_a_tile_border_meet_there_end_to_end() {
        // #116, #117: the copies in the tiles' buffers overlapped, ending along each other.
        let here = TileProjection {
            x: 8531.0,
            y: 5767.0,
            extent: 4096.0,
        };
        let east = TileProjection { x: 8532.0, ..here };
        let tags = Tags(HashMap::from([(
            "class".to_owned(),
            Value::String("minor".to_owned()),
        )]));
        // One street in each tile's pixels, reaching into the other's buffer: x 4000–4150 here
        // is −96–54 there, crossing the border at 96 / 150 of the way.
        let street = |shift: f64| {
            Geometry::LineString(LineString::from(vec![
                (4000.0 - shift, 500.0),
                (4150.0 - shift, 520.0),
            ]))
        };
        let mut data = MapData::default();
        add_road(&mut data, &tags, &street(0.0), &here);
        add_road(&mut data, &tags, &street(4096.0), &east);

        assert_eq!(data.roads.len(), 2);
        let (western, eastern) = (&data.roads[0].line, &data.roads[1].line);
        let border = here.point(4096.0, 500.0 + 20.0 * 96.0 / 150.0);
        let close = |p: LatLon| (p.0 - border.0).abs() < 1e-9 && (p.1 - border.1).abs() < 1e-9;
        assert!(
            close(western[western.len() - 1]) && close(eastern[0]),
            "{western:?} {eastern:?}"
        );
        // Neither reaches past the border into the other tile.
        assert!(western.iter().all(|p| p.1 <= border.1 + 1e-9));
        assert!(eastern.iter().all(|p| p.1 >= border.1 - 1e-9));
        // A way only in the buffer is the neighbour's.
        let beyond = Geometry::LineString(LineString::from(vec![(4110.0, 500.0), (4150.0, 500.0)]));
        add_road(&mut data, &tags, &beyond, &here);
        assert_eq!(data.roads.len(), 2);
    }

    #[test]
    fn reads_colours_as_hex_or_names() {
        assert_eq!(
            color("#5a81a0"),
            Some([90.0 / 255.0, 129.0 / 255.0, 160.0 / 255.0])
        );
        assert_eq!(color("#fff"), Some([1.0; 3]));
        assert!(color("White").is_some_and(|c| c.iter().all(|&v| v > 0.9)));
        assert_eq!(color("sparkly"), None);
        assert_eq!(color("#12345"), None);
    }

    #[test]
    fn a_building_on_a_tile_border_is_drawn_by_one_tile() {
        let here = TileProjection {
            x: 8531.0,
            y: 5767.0,
            extent: 4096.0,
        };
        let east = TileProjection { x: 8532.0, ..here };
        // A house straddling the border, in each tile's pixels: x 4080–4120 here is −16–24
        // there.
        assert_ne!(here.owns((4100.0, 500.0)), east.owns((4.0, 500.0)));
        // A house just inside the eastern tile's buffer belongs to the eastern tile.
        assert!(!here.owns((4120.0, 500.0)) && east.owns((24.0, 500.0)));
        // A hall too large for the buffers is cut in both tiles (here at 4160, there at −64);
        // it may be kept twice but never dropped.
        assert!(here.owns((3580.0, 500.0)) || east.owns((68.0, 500.0)));
        // Both tiles agree on its id where the copies are identical.
        assert_eq!(here.id((4100.0, 500.0)), east.id((4.0, 500.0)));
    }
}
