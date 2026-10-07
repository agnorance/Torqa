//! Buildings from OpenStreetMap footprints (R45). The map rarely says more than the outline
//! and sometimes a height, so what a building is gets guessed from where it stands, what is in
//! it and its size: churches from places of worship, castles and lighthouses from theirs (#137),
//! hotels, offices and public buildings (schools, hospitals, town halls) from the points of
//! interest in them or the land they stand on, halls on industrial land, chalets in the
//! mountains, farmhouses as large buildings in the countryside, blocks from their height or
//! size in towns, sheds from their size, and houses otherwise — in the subtropics houses built
//! for the heat (#136). Each kind gets its own proportions, roof, materials and details.

mod models;
mod parts;
mod shape;

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use torqa_osm::{Building, MapData};
use torqa_routes::LocalProjection;

use crate::climate::Climate;
use crate::{BuildingCell, HeightGrid, MeshData, hash, palette};
use models::{Fit, Wanted};
pub(crate) use parts::Style;
use parts::{Builder, Paint, Pitch, RoofPaint};
pub(crate) use shape::{Point, centroid, contains, footprint, signed_area, triangulate};
use shape::{Rect, distance};

/// Height of a storey; rows of windows repeat at it.
const STOREY: f64 = 3.0;
/// Walls of whole storeys end this far above the last one, clear of its windows.
const EAVES_MARGIN: f64 = 0.4;
/// Walls reach this far below the lowest ground point, so slopes never show a gap.
const FOUNDATION: f64 = 1.0;
/// Footprints filling this much of the rectangle around them are built as that rectangle,
/// which can carry gable and hipped roofs; the difference does not show from the road.
const RECTANGULAR: f64 = 0.8;
/// Churches keep their nave shape even with a choir or porch, castles theirs with towers
/// standing out, and lighthouses theirs though round (a circle fills 79 % of its square).
const RECTANGULAR_CHURCH: f64 = 0.7;
/// Above this elevation chalets start to replace houses; above the second, all are chalets.
const CHALETS_FROM: f64 = 700.0;
const CHALETS_ONLY: f64 = 1100.0;
/// A church point mapped on church grounds rather than the church marks the largest
/// building this close.
const CHURCH_REACH: f64 = 30.0;
/// A castle point mapped on the castle's grounds rather than its keep marks the largest
/// building this close.
const CASTLE_REACH: f64 = 40.0;
/// A lighthouse point beside its tower marks the nearest building this close; with none, the
/// tower stands on its own, `LONE_LIGHTHOUSE` metres wide.
const LIGHTHOUSE_REACH: f64 = 10.0;
const LONE_LIGHTHOUSE: f64 = 6.0;
/// Churches smaller than this are chapels.
const CHAPEL_AREA: f64 = 150.0;
/// Public buildings, hotels and offices are at least this large; smaller ones are what their
/// size says (a kindergarten in a house, a guest house).
const PUBLIC_AREA: f64 = 250.0;
const HOTEL_AREA: f64 = 150.0;
const OFFICE_AREA: f64 = 150.0;
/// Larger low buildings on commercial land are stores, built as halls.
const STORE_AREA: f64 = 2500.0;
/// Models' walls reach 3 m below their ground floor; on plots falling more than this they
/// would float, so those buildings keep their shells. Castles and lighthouses, on hilltops and
/// rocks, reach 8 m down (`art/buildings`, `DEEP`).
const MODEL_BASEMENT: f64 = 2.8;
const DEEP_BASEMENT: f64 = 7.8;
/// Modelled buildings are grouped in cells this size, so that each cell switches between
/// models up close and shells far away by its own distance.
const CELL: f64 = 120.0;

/// Building colours from the palette (sRGB, ADR 0011), read once.
type Colours = LazyLock<Vec<[f32; 3]>>;

/// Plaster of houses: cream, sand, peach, dusty rose, pale ochre, pale sage.
static PLASTER: Colours = LazyLock::new(|| palette::list("buildings.walls"));
/// Plaster of light blocks, churches and masonry ground floors: creams and off-whites.
static LIGHT_PLASTER: Colours = LazyLock::new(|| palette::list("buildings.light_walls"));
/// Walls of offices, hotels and public buildings, and some blocks (#75): cream, pale yellow,
/// pale blue, pale sage, off-white, brick.
static MODERN: Colours = LazyLock::new(|| palette::list("buildings.modern_walls"));
/// Their trim, bands and canopies: coral, teal, blue, plum, yellow, sage.
static ACCENTS: Colours = LazyLock::new(|| palette::list("buildings.accents"));
/// Timber: browns of the palette.
static WOOD: Colours = LazyLock::new(|| palette::list("buildings.timber"));
/// Roof tiles: coral, terracotta, brick, then two slate greys.
static TILES: Colours = LazyLock::new(|| palette::list("buildings.tiles"));
/// Mountain roofs: slate greys, brown shingles.
static MOUNTAIN_ROOFS: Colours = LazyLock::new(|| palette::list("buildings.mountain_roofs"));
/// Metal cladding of halls: pale greys and creams, blue-grey.
static CLADDING: Colours = LazyLock::new(|| palette::list("buildings.cladding"));
/// Metal roofs: blue-greys.
static SHEET: Colours = LazyLock::new(|| palette::list("buildings.sheet"));
/// Flat roofs: gravel tones.
static FLAT: Colours = LazyLock::new(|| palette::list("buildings.flat_roofs"));
/// Castle walls: light stone and plaster.
static CASTLE_WALLS: Colours = LazyLock::new(|| palette::list("buildings.castle_walls"));
/// The bands and caps of lighthouses: coral red first, then teal and plum.
static BEACON: Colours = LazyLock::new(|| palette::list("buildings.beacon"));
/// Houses in the subtropics: whites and pale pastels.
static TROPICAL_WALLS: Colours = LazyLock::new(|| palette::list("buildings.tropical_walls"));
/// Spires: slate, copper green, coral tiles.
static SPIRES: Colours = LazyLock::new(|| palette::list("buildings.spires"));
/// Shop fronts' frames, and awnings.
static SHOPFRONTS: Colours = LazyLock::new(|| palette::list("buildings.shopfronts"));
static AWNINGS: Colours = LazyLock::new(|| palette::list("buildings.awnings"));
/// Accents: the shutters' sage, teal, dusty rose and plum, also the trim of light blocks.
static SHUTTERS: Colours = LazyLock::new(|| palette::list("buildings.shutters"));
/// Window frames; stone around church openings; chimneys and their caps.
static WHITE: LazyLock<[f32; 3]> = LazyLock::new(|| rgb("buildings.frame"));
static STONE: LazyLock<[f32; 3]> = LazyLock::new(|| rgb("buildings.stone"));
static METAL: LazyLock<[f32; 3]> = LazyLock::new(|| rgb("buildings.metal"));
static SOOT: LazyLock<[f32; 3]> = LazyLock::new(|| rgb("buildings.soot"));
static BRICK: LazyLock<[f32; 3]> = LazyLock::new(|| rgb("buildings.chimney"));

/// The palette colour nearest to a colour from the map: a mapped colour keeps its idea (red,
/// white, yellow) but fits the look.
fn nearest(mapped: [f32; 3], palette: &[[f32; 3]]) -> [f32; 3] {
    let distance = |c: &[f32; 3]| (0..3).map(|k| (c[k] - mapped[k]).powi(2)).sum::<f32>();
    palette
        .iter()
        .copied()
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
        .unwrap_or(mapped)
}

fn rgb(path: &str) -> [f32; 3] {
    let [r, g, b, _] = palette::srgb(path, 1.0);
    [r, g, b]
}

/// What a building is, as far as the map lets us tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A family house: plastered walls, a pitched roof.
    House,
    /// A mountain house: timber over a masonry ground floor, a shallow roof with deep eaves.
    Chalet,
    /// A large rural building under a big, steep roof reaching low.
    Farmhouse,
    /// Shed, garage or barn too small to live in: no windows.
    Shed,
    /// Apartments or offices: several storeys, mostly flat roofs.
    Block,
    /// Factory, warehouse or store: low, wide, clad in metal.
    Hall,
    /// A church with a tower, or a chapel with a turret on its roof.
    Church,
    /// Offices: bands of glass between plain spandrels over a glazed ground floor.
    Office,
    /// A school, hospital, town hall and the like: wide, symmetrical, with a marked entrance.
    Public,
    /// A hotel: balconies all along its fronts, an entrance under a canopy.
    Hotel,
    /// A castle: crenellated walls round a steep roof, round towers at the corners.
    Castle,
    /// A lighthouse: a round tower in bands of colour, a lantern on top.
    Lighthouse,
}

/// A building the map marks by a point of interest of its own, other than a church.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Landmark {
    Castle,
    Lighthouse,
}

/// Where a building stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Setting {
    /// In a residential area.
    Town,
    /// On industrial land.
    Industrial,
    /// On commercial or retail land.
    Commercial,
    /// On the grounds of a school, college, university or hospital.
    Public,
    /// Anywhere else.
    Countryside,
}

/// A building with what is known about it, ready to be built.
pub(crate) struct Plot<'a> {
    pub(crate) building: &'a Building,
    /// Footprint in metres east/north, counter-clockwise.
    pub(crate) footprint: Vec<Point>,
    pub(crate) setting: Setting,
    /// A church point lies in or by it.
    pub(crate) church: bool,
    /// A shop, café or the like is in it: the point of the street its front faces.
    pub(crate) shop: Option<Point>,
    /// What the points of interest in or by it say it is used for.
    pub(crate) purpose: Option<Purpose>,
    /// A castle or lighthouse point lies in or by it.
    pub(crate) landmark: Option<Landmark>,
    /// The climate of the region it stands in.
    pub(crate) climate: Climate,
}

/// What a building is used for, from the points of interest in or by it; later ones win over
/// earlier ones where a building has several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Purpose {
    Office,
    Hotel,
    Public,
}

/// Shop fronts are this high, their awnings reach this far out.
const SHOP_HEIGHT: f64 = 2.8;
const AWNING_REACH: f64 = 1.4;

/// Shop points this far from a building's outline still belong to it (mapped by the door).
const SHOP_REACH: f64 = 8.0;
/// So do hotel, office and public points (mapped by the entrance or on the grounds).
const PURPOSE_REACH: f64 = 12.0;
/// A shop's street lies at most this far from its building's middle.
const STREET_REACH: f64 = 40.0;
/// Index cell size of [`Frontage`].
const FRONTAGE_CELL: f64 = 25.0;
/// A street's middle this far inside a building's outline runs through it (#100); less is
/// taken for the map's inaccuracy.
const THROUGH_DEPTH: f64 = 1.0;

/// The paved streets on the ground and the road ridden, as points every few metres: shops face
/// them, and buildings keep off them.
pub(crate) struct Frontage {
    cells: HashMap<(i64, i64), Vec<Point>>,
}

impl Frontage {
    pub(crate) fn new(points: impl IntoIterator<Item = Point>) -> Self {
        let mut cells: HashMap<(i64, i64), Vec<Point>> = HashMap::new();
        for point in points {
            cells.entry(frontage_cell(point)).or_default().push(point);
        }
        Self { cells }
    }

    /// The nearest street point within `reach` of `point`.
    fn nearest(&self, point: Point, reach: f64) -> Option<Point> {
        #[allow(clippy::cast_possible_truncation)] // a few cells
        let span = (reach / FRONTAGE_CELL).ceil() as i64;
        let (ce, cn) = frontage_cell(point);
        let mut best: Option<(f64, Point)> = None;
        for x in ce - span..=ce + span {
            for y in cn - span..=cn + span {
                for &candidate in self.cells.get(&(x, y)).into_iter().flatten() {
                    let d = distance(point, candidate);
                    if d <= reach && best.is_none_or(|b| d < b.0) {
                        best = Some((d, candidate));
                    }
                }
            }
        }
        best.map(|b| b.1)
    }

    /// Whether a street runs through the building with this footprint.
    pub(crate) fn runs_through(&self, footprint: &[Point]) -> bool {
        let Some(&first) = footprint.first() else {
            return false;
        };
        let (low, high) = footprint.iter().fold((first, first), |(low, high), p| {
            (
                (low.0.min(p.0), low.1.min(p.1)),
                (high.0.max(p.0), high.1.max(p.1)),
            )
        });
        let ((low_e, low_n), (high_e, high_n)) = (frontage_cell(low), frontage_cell(high));
        (low_e..=high_e)
            .flat_map(|x| (low_n..=high_n).map(move |y| (x, y)))
            .flat_map(|cell| self.cells.get(&cell).into_iter().flatten())
            .any(|&point| {
                contains(footprint, point) && outline_distance(footprint, point) > THROUGH_DEPTH
            })
    }
}

#[allow(clippy::cast_possible_truncation)] // local metres stay far below 2^63 cells
fn frontage_cell(point: Point) -> (i64, i64) {
    (
        (point.0 / FRONTAGE_CELL).floor() as i64,
        (point.1 / FRONTAGE_CELL).floor() as i64,
    )
}

/// Gives the plots with a shop in or by them a front onto their street.
pub(crate) fn mark_shops(plots: &mut [Plot], shops: &[Point], frontage: &Frontage) {
    for &point in shops {
        let inside = plots.iter().position(|p| contains(&p.footprint, point));
        let nearby = || {
            plots
                .iter()
                .enumerate()
                .map(|(k, p)| (k, outline_distance(&p.footprint, point)))
                .filter(|&(_, d)| d < SHOP_REACH)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(k, _)| k)
        };
        let Some(index) = inside.or_else(nearby) else {
            continue;
        };
        let plot = &mut plots[index];
        if plot.church || plot.landmark.is_some() {
            continue;
        }
        plot.shop = frontage.nearest(centroid(&plot.footprint), STREET_REACH);
    }
}

/// Marks the plots the `points` (hotels, offices or public buildings) lie in or by as used for
/// `purpose`.
pub(crate) fn mark_purpose(plots: &mut [Plot], points: &[Point], purpose: Purpose) {
    for &point in points {
        let inside = plots.iter().position(|p| contains(&p.footprint, point));
        let nearby = || {
            plots
                .iter()
                .enumerate()
                .map(|(k, p)| (k, outline_distance(&p.footprint, point)))
                .filter(|&(_, d)| d < PURPOSE_REACH)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(k, _)| k)
        };
        if let Some(index) = inside.or_else(nearby) {
            let plot = &mut plots[index];
            plot.purpose = plot.purpose.max(Some(purpose));
        }
    }
}

/// Distance from `point` to the outline (a closed ring) of a footprint.
fn outline_distance(footprint: &[Point], point: Point) -> f64 {
    (0..footprint.len())
        .map(|k| {
            let (a, b) = (footprint[k], footprint[(k + 1) % footprint.len()]);
            let (de, dn) = (b.0 - a.0, b.1 - a.1);
            let length_squared = (de * de + dn * dn).max(1e-12);
            let t =
                (((point.0 - a.0) * de + (point.1 - a.1) * dn) / length_squared).clamp(0.0, 1.0);
            distance(point, (a.0 + de * t, a.1 + dn * t))
        })
        .fold(f64::INFINITY, f64::min)
}

/// Marks the plots that are churches: the one each church point lies in, or for points
/// mapped on church grounds, the largest building near it.
pub(crate) fn mark_churches(plots: &mut [Plot], churches: &[Point]) {
    for &point in churches {
        let inside = plots.iter().position(|p| contains(&p.footprint, point));
        let nearby = || {
            plots
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.footprint
                        .iter()
                        .any(|&corner| distance(corner, point) < CHURCH_REACH)
                })
                .max_by(|(_, a), (_, b)| {
                    signed_area(&a.footprint).total_cmp(&signed_area(&b.footprint))
                })
                .map(|(i, _)| i)
        };
        if let Some(i) = inside.or_else(nearby) {
            plots[i].church = true;
        }
    }
}

/// Marks the plots that are castles or lighthouses: the one each point lies in, or near it
/// for castles (points on the castle grounds) the largest building, for lighthouses (points
/// beside the tower) the nearest. Churches stay churches.
pub(crate) fn mark_landmarks(plots: &mut [Plot], points: &[Point], landmark: Landmark) {
    for &point in points {
        let inside = plots.iter().position(|p| contains(&p.footprint, point));
        let nearby = || {
            let near = plots.iter().enumerate().map(|(k, p)| {
                (
                    k,
                    outline_distance(&p.footprint, point),
                    signed_area(&p.footprint),
                )
            });
            let found = match landmark {
                Landmark::Castle => near
                    .filter(|&(_, d, _)| d < CASTLE_REACH)
                    .max_by(|a, b| a.2.total_cmp(&b.2)),
                Landmark::Lighthouse => near
                    .filter(|&(_, d, _)| d < LIGHTHOUSE_REACH)
                    .min_by(|a, b| a.1.total_cmp(&b.1)),
            };
            found.map(|(k, _, _)| k)
        };
        if let Some(i) = inside.or_else(nearby)
            && !plots[i].church
        {
            plots[i].landmark = Some(landmark);
        }
    }
}

/// Buildings for the lighthouses the map has as points only, with no building in or near
/// them: a round tower `LONE_LIGHTHOUSE` metres wide on each point.
pub(crate) fn lone_lighthouses(map: &MapData, projection: &LocalProjection) -> Vec<Building> {
    // Buildings starting farther off than this (about 500 m) cannot hold the point.
    const NEAR: f64 = 0.005;
    map.lighthouses
        .iter()
        .filter_map(|&(lat, lon)| {
            let point = projection.project(lat, lon);
            let wide = NEAR / lat.to_radians().cos().max(0.1);
            let housed = map
                .buildings
                .iter()
                .filter(|b| {
                    b.outline
                        .first()
                        .is_some_and(|&(a, o)| (a - lat).abs() < NEAR && (o - lon).abs() < wide)
                })
                .any(|b| {
                    let outline = footprint(b, projection);
                    outline.len() >= 3
                        && (contains(&outline, point)
                            || outline_distance(&outline, point) < LIGHTHOUSE_REACH)
                });
            (!housed).then(|| {
                let radius = LONE_LIGHTHOUSE / 2.0;
                let outline = (0..=8)
                    .map(|k| {
                        let angle = std::f64::consts::TAU * f64::from(k % 8) / 8.0;
                        projection.unproject(
                            point.0 + radius * angle.cos(),
                            point.1 + radius * angle.sin(),
                        )
                    })
                    .collect();
                // Negative, so apart from the map's buildings' ids, and stable for the place.
                #[allow(clippy::cast_possible_truncation)] // micro-degrees fit easily
                let id = i64::MIN
                    + ((lat + 90.0) * 1e6).round() as i64 * 400_000_000
                    + ((lon + 180.0) * 1e6).round() as i64;
                Building {
                    id,
                    outline,
                    height: None,
                    levels: None,
                    color: None,
                }
            })
        })
        .collect()
}

/// A chunk's buildings.
#[derive(Debug, Default)]
pub(crate) struct ChunkBuildings {
    /// Shells of the buildings no model fits.
    pub(crate) shells: MeshData,
    /// Buildings drawn as models, by cell.
    pub(crate) cells: BTreeMap<(i64, i64), BuildingCell>,
}

/// Adds a building standing on `heights` to a chunk centred at `origin`: as a model where one
/// fits its footprint (with its shell for the distance), else as a shell.
pub(crate) fn add(chunk: &mut ChunkBuildings, plot: &Plot, heights: &HeightGrid, origin: [f64; 3]) {
    let footprint = &plot.footprint;
    if footprint.len() < 3 {
        return;
    }
    // Floors are level: the ground floor is at the highest ground, and on slopes the walls
    // go down to the lowest like a basement.
    let grounds: Vec<f64> = footprint.iter().map(|&(e, n)| heights.at(e, n)).collect();
    let ground = grounds.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let lowest = grounds.iter().copied().fold(f64::INFINITY, f64::min);
    let area = signed_area(footprint);
    let dice = Dice(plot.building.id);
    let kind = kind(plot, area, ground, &dice);
    let tropical = plot.climate.tropical_at(ground);
    let special = matches!(kind, Kind::Church | Kind::Castle | Kind::Lighthouse);
    let fill = if special {
        RECTANGULAR_CHURCH
    } else {
        RECTANGULAR
    };
    let rect = Rect::around(footprint).filter(|r| area / r.area() >= fill);
    let design = (!special).then(|| design(kind, plot.building, rect.is_some(), tropical, &dice));
    let basement = if matches!(kind, Kind::Castle | Kind::Lighthouse) {
        DEEP_BASEMENT
    } else {
        MODEL_BASEMENT
    };
    // A shop's front is drawn on the shell, which shows it up close too.
    let fit = rect
        .filter(|_| ground - lowest < basement && plot.shop.is_none())
        .and_then(|rect| {
            let wanted = Wanted {
                kind,
                tropical,
                chapel: area < CHAPEL_AREA,
                storeys: design
                    .as_ref()
                    .filter(|d| d.windows)
                    .map(|d| storeys_of(wall_height(plot.building, rise(d, &rect), d))),
                hipped: design.as_ref().is_some_and(|d| d.roof == Roof::Hipped),
            };
            models::fitting(&rect, wanted, &dice).map(|fit| (rect, fit))
        });
    let footing = lowest - FOUNDATION;
    let Some((rect, fit)) = fit else {
        let mut builder = Builder {
            mesh: &mut chunk.shells,
            origin,
            ground,
            footing,
        };
        shell(&mut builder, plot, area, rect, design.as_ref(), &dice, None);
        return;
    };

    #[allow(clippy::cast_possible_truncation)] // world coordinates are far below 2^63 cells
    let cell = (
        (rect.centre.0 / CELL).floor() as i64,
        (rect.centre.1 / CELL).floor() as i64,
    );
    let cell = chunk.cells.entry(cell).or_default();
    let (plaster, roof) = colours(plot, design.as_ref(), &dice);
    // Chalets and farmhouses show their front gable to the valley; churches have their choir
    // in the east; the rest face either way.
    let front = rect.point(rect.half_length, 0.0);
    let back = rect.point(-rect.half_length, 0.0);
    let turn = match kind {
        Kind::Chalet | Kind::Farmhouse => heights.at(front.0, front.1) > heights.at(back.0, back.1),
        Kind::Church => rect.axis.0 < 0.0,
        _ => dice.roll(51) < 0.5,
    };
    let instance = Instance {
        rect: &rect,
        fit,
        turn,
        ground,
        plaster,
        roof,
        variant: dice.roll(50),
    };
    instance.push(
        cell.models.entry(fit.model.name.clone()).or_default(),
        origin,
    );
    let mut builder = Builder {
        mesh: &mut cell.shells,
        origin,
        ground,
        footing,
    };
    shell(
        &mut builder,
        plot,
        area,
        Some(rect),
        design.as_ref(),
        &dice,
        Some(fit),
    );
}

/// A building of the map, built as a shell; `fit` makes it match the model drawn up close.
fn shell(
    b: &mut Builder,
    plot: &Plot,
    area: f64,
    rect: Option<Rect>,
    design: Option<&Design>,
    dice: &Dice,
    fit: Option<Fit>,
) {
    let Some(design) = design else {
        match landmark(plot) {
            Some(Landmark::Castle) => castle(b, plot, area, rect, dice, fit),
            Some(Landmark::Lighthouse) => lighthouse(b, plot, rect, dice, fit),
            None => church(b, plot, area, rect, dice),
        }
        return;
    };
    if let Some(fit) = fit {
        let mut matched = design.clone();
        matched.roof = match fit.model.roof.as_str() {
            "hipped" => Roof::Hipped,
            // Flat-roofed models end in a parapet as their designs do.
            "flat" => match design.roof {
                Roof::Flat { parapet } => Roof::Flat { parapet },
                Roof::Gable | Roof::Hipped => Roof::Flat { parapet: 0.7 },
            },
            _ => Roof::Gable,
        };
        if let Some(pitch) = fit.model.pitch {
            matched.pitch_degrees = pitch;
        }
        let walls = Some(fit.model.eaves);
        build(
            b,
            &plot.footprint,
            rect,
            &matched,
            plot.building,
            walls,
            dice,
        );
    } else {
        build(b, &plot.footprint, rect, design, plot.building, None, dice);
        if let Some(street) = plot.shop {
            let outline = rect.map_or_else(|| plot.footprint.clone(), |r| r.corners());
            shop_front(b, &outline, street, dice);
        }
    }
}

/// A shop front with an awning on the ground floor of the wall facing `street`, if one does.
fn shop_front(b: &mut Builder, outline: &[Point], street: Point, dice: &Dice) {
    let count = outline.len();
    let facing = (0..count)
        .filter_map(|k| {
            let (a, c) = (outline[k], outline[(k + 1) % count]);
            let length = distance(a, c);
            if length < 2.5 {
                return None;
            }
            // Counter-clockwise outline: outward is to the right of the way round.
            let out = ((c.1 - a.1) / length, (a.0 - c.0) / length);
            let middle = (f64::midpoint(a.0, c.0), f64::midpoint(a.1, c.1));
            let to_street = (street.0 - middle.0, street.1 - middle.1);
            let away = to_street.0.hypot(to_street.1).max(1e-9);
            let facing = (out.0 * to_street.0 + out.1 * to_street.1) / away;
            Some((facing, away, (a, c)))
        })
        .filter(|f| f.0 > 0.3)
        .max_by(|x, y| x.0.total_cmp(&y.0));
    let Some((_, away, edge)) = facing else {
        return;
    };
    let front = Paint::new(dice.pick(60, &SHOPFRONTS), Style::Shop);
    let awning = Paint::new(dice.pick(61, &AWNINGS), Style::Blank);
    // The awning keeps clear of the street.
    let reach = (away - 1.5).clamp(0.6, AWNING_REACH);
    b.shop_front(edge, (b.ground, SHOP_HEIGHT), reach, (front, awning));
}

/// One model instance and how it stands.
struct Instance<'a> {
    rect: &'a Rect,
    fit: Fit,
    /// Turned round: the model's front towards the rectangle's back.
    turn: bool,
    ground: f64,
    plaster: [f32; 3],
    roof: [f32; 3],
    variant: f64,
}

impl Instance<'_> {
    /// Appends the instance in Godot `MultiMesh` buffer layout: transform (row-major 3×4,
    /// relative to `origin`), instance colour (plaster) and custom data (roof colour, variant).
    #[allow(clippy::cast_possible_truncation)] // stored as f32 for the GPU
    fn push(&self, buffer: &mut Vec<f32>, origin: [f64; 3]) {
        let sign = if self.turn { -1.0 } else { 1.0 };
        let (east, north) = (self.rect.axis.0 * sign, self.rect.axis.1 * sign);
        let (along, across) = self.fit.scale;
        let position = [
            self.rect.centre.0 - origin[0],
            self.ground - origin[1],
            -self.rect.centre.1 - origin[2],
        ];
        // The model's x runs along the rectangle, its z (Blender's −y) across it.
        buffer.extend(
            [
                east * along,
                0.0,
                north * across,
                position[0],
                0.0,
                1.0,
                0.0,
                position[1],
                -north * along,
                0.0,
                east * across,
                position[2],
            ]
            .map(|v| v as f32),
        );
        buffer.extend([self.plaster[0], self.plaster[1], self.plaster[2], 1.0]);
        buffer.extend([
            self.roof[0],
            self.roof[1],
            self.roof[2],
            self.variant as f32,
        ]);
    }
}

/// The castle or lighthouse a plot is, unless it is a church.
fn landmark(plot: &Plot) -> Option<Landmark> {
    plot.landmark.filter(|_| !plot.church)
}

/// The plaster and roof colours of a building (sRGB), for its model. Flat-roofed offices,
/// hotels and public buildings have their accent colour in place of the roof's, lighthouses
/// the colour of their bands.
fn colours(plot: &Plot, design: Option<&Design>, dice: &Dice) -> ([f32; 3], [f32; 3]) {
    if let Some(design) = design
        && matches!(design.kind, Kind::Office | Kind::Public | Kind::Hotel)
    {
        let accent = match (design.roof, design.trim) {
            (Roof::Flat { .. }, Some(trim)) => trim.band.rgb,
            _ => design.roof_paint.top.rgb,
        };
        (design.wall.rgb, accent)
    } else if let Some(design) = design {
        (
            design.base.unwrap_or(design.wall).rgb,
            design.roof_paint.top.rgb,
        )
    } else {
        let (wall, roof) = match landmark(plot) {
            Some(Landmark::Castle) => castle_paints(plot.building, dice),
            Some(Landmark::Lighthouse) => lighthouse_paints(dice),
            None => church_paints(plot.building, dice),
        };
        (wall.rgb, roof.rgb)
    }
}

/// Storeys of walls this high.
fn storeys_of(walls: f64) -> u32 {
    // Small whole numbers.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let count = ((walls - EAVES_MARGIN) / STOREY).round().max(1.0) as u32;
    count
}

/// What a building is, from what is in it, its surroundings, size, mapped height and
/// elevation.
fn kind(plot: &Plot, area: f64, ground: f64, dice: &Dice) -> Kind {
    if plot.church {
        return Kind::Church;
    }
    match plot.landmark {
        Some(Landmark::Castle) => return Kind::Castle,
        Some(Landmark::Lighthouse) => return Kind::Lighthouse,
        None => {}
    }
    let height = mapped_height(plot.building);
    if area < 30.0 && height.is_none_or(|h| h < 5.0) {
        return Kind::Shed;
    }
    // Guest houses in a house, a doctor's office or a post office in a shop stay what they
    // look like.
    match plot.purpose {
        Some(Purpose::Public) if area >= PUBLIC_AREA => return Kind::Public,
        Some(Purpose::Hotel) if area >= HOTEL_AREA => return Kind::Hotel,
        Some(Purpose::Office) if area >= OFFICE_AREA => return Kind::Office,
        _ => {}
    }
    if height.is_some_and(|h| h >= 12.0) {
        return if plot.setting == Setting::Commercial {
            Kind::Office
        } else {
            Kind::Block
        };
    }
    // The subtropics have neither chalets nor Bernese farmhouses.
    let tropical = plot.climate.tropical_at(ground);
    let alpine = !tropical && dice.roll(0) < smoothstep(CHALETS_FROM, CHALETS_ONLY, ground);
    match plot.setting {
        Setting::Industrial | Setting::Commercial if area < 80.0 => Kind::Shed,
        Setting::Industrial => Kind::Hall,
        // Big low buildings on commercial land are stores, the rest offices.
        Setting::Commercial if area > STORE_AREA && height.is_none_or(|h| h < 10.0) => Kind::Hall,
        Setting::Commercial if area >= OFFICE_AREA => Kind::Office,
        Setting::Public if area >= PUBLIC_AREA => Kind::Public,
        Setting::Town if area > 400.0 => Kind::Block,
        Setting::Countryside if area > 1500.0 => Kind::Hall,
        _ if alpine => Kind::Chalet,
        Setting::Countryside if area > 220.0 && !tropical => Kind::Farmhouse,
        _ => Kind::House,
    }
}

/// Mapped heights reach the top of the roof.
fn mapped_height(building: &Building) -> Option<f64> {
    building.height.or_else(|| building.levels.map(storeys))
}

/// Wall height of `count` storeys.
fn storeys(count: f64) -> f64 {
    count.max(1.0) * STOREY + EAVES_MARGIN
}

/// The wall height of whole storeys nearest to `height`, so windows never meet the eaves.
fn whole_storeys(height: f64) -> f64 {
    storeys(((height - EAVES_MARGIN) / STOREY).round())
}

fn smoothstep(from: f64, to: f64, x: f64) -> f64 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Pseudo-random but stable choices for one building.
struct Dice(i64);

impl Dice {
    /// A value in [0, 1); each `salt` gives an independent one.
    fn roll(&self, salt: i64) -> f64 {
        hash(self.0 ^ salt.wrapping_mul(0x5851_F42D_4C95_7F2D))
    }

    fn pick<T: Copy>(&self, salt: i64, options: &[T]) -> T {
        // `roll` lies in [0, 1), so the index is a small non-negative number.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        #[allow(clippy::cast_precision_loss)]
        let index = (self.roll(salt) * options.len() as f64) as usize;
        options[index.min(options.len() - 1)]
    }

    /// A colour varied a little, so neighbours painted alike still differ.
    fn tint(&self, rgb: [f32; 3]) -> [f32; 3] {
        #[allow(clippy::cast_possible_truncation)] // a factor near 1
        let scale = (0.94 + 0.12 * self.roll(99)) as f32;
        rgb.map(|c| (c * scale).min(1.0))
    }
}

/// Roof shapes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Roof {
    Gable,
    Hipped,
    Flat { parapet: f64 },
}

/// How a building is built.
#[derive(Debug, Clone)]
struct Design {
    kind: Kind,
    /// Wall height when the map gives none.
    walls: f64,
    /// Whether the walls have rows of windows, which then need whole storeys.
    windows: bool,
    /// Roof over rectangular footprints; others get a hipped band or a flat roof.
    roof: Roof,
    pitch_degrees: f64,
    /// Wide buildings get flatter roofs so they rise no more than this.
    max_rise: f64,
    overhang: f64,
    verge: f64,
    wall: Paint,
    /// A masonry ground floor below timber walls.
    base: Option<Paint>,
    roof_paint: RoofPaint,
    chimney: bool,
    /// Frame colour of windows in the gables.
    gable_windows: Option<[f32; 3]>,
    balcony: bool,
    /// Blocks: a cornice under the roof line (or round the top of the parapet), a string
    /// course over the ground storey, stacked balconies and units on a flat roof.
    trim: Option<Trim>,
}

/// The finish of a block, office, hotel or public building (#75): trim that stands out
/// against the walls, as in its references.
#[derive(Debug, Clone, Copy)]
struct Trim {
    /// Cornice and string course.
    band: Paint,
    /// Balconies on the long sides, if any.
    balconies: Option<Paint>,
    /// A band at every floor, not only over the ground storey.
    floors: bool,
    /// Balconies in every column of windows rather than every other.
    every_column: bool,
}

/// How a building of `kind` is built; houses in the subtropics (`tropical`) are built for the
/// heat.
fn design(
    kind: Kind,
    building: &Building,
    rectangular: bool,
    tropical: bool,
    dice: &Dice,
) -> Design {
    let recipe = Recipe {
        kind,
        building,
        rectangular,
        dice,
    };
    match kind {
        Kind::House if tropical => recipe.tropical_house(),
        Kind::House => recipe.house(),
        Kind::Chalet => recipe.chalet(),
        Kind::Farmhouse => recipe.farmhouse(),
        Kind::Shed => recipe.shed(),
        Kind::Block => recipe.block(),
        Kind::Office => recipe.office(),
        Kind::Public => recipe.public(),
        Kind::Hotel => recipe.hotel(),
        // `church`, `castle` and `lighthouse` build those; none get here.
        Kind::Hall | Kind::Church | Kind::Castle | Kind::Lighthouse => recipe.hall(),
    }
}

/// What goes into one building's design.
struct Recipe<'a> {
    kind: Kind,
    building: &'a Building,
    rectangular: bool,
    dice: &'a Dice,
}

impl Recipe<'_> {
    fn paint(&self, palette: &[[f32; 3]], salt: i64, style: Style) -> Paint {
        Paint::new(self.dice.tint(self.dice.pick(salt, palette)), style)
    }

    /// The main walls' paint: the mapped colour if there is one.
    fn facade(&self, palette: &[[f32; 3]], style: Style) -> Paint {
        let rgb = self.building.color.map_or_else(
            || self.dice.pick(1, palette),
            |mapped| nearest(mapped, palette),
        );
        Paint::new(self.dice.tint(rgb), style)
    }

    /// Overhang varies with each building, at the eaves and gables alike.
    fn overhang(&self, least: f64, spread: f64) -> f64 {
        least + spread * self.dice.roll(5)
    }

    fn house(&self) -> Design {
        let wall = self.facade(&PLASTER, Style::Plaster);
        let wood = self.paint(&WOOD, 2, Style::Boards);
        Design {
            kind: self.kind,
            walls: storeys(match self.dice.roll(3) {
                r if r < 0.25 => 1.0,
                r if r < 0.85 => 2.0,
                _ => 3.0,
            }),
            windows: true,
            roof: if self.dice.roll(4) < 0.75 {
                Roof::Gable
            } else {
                Roof::Hipped
            },
            pitch_degrees: 35.0 + 10.0 * self.dice.roll(6),
            max_rise: 6.0,
            overhang: self.overhang(0.5, 0.2),
            verge: self.overhang(0.4, 0.2),
            wall,
            base: None,
            roof_paint: RoofPaint {
                top: if self.dice.roll(7) < 0.85 {
                    self.paint(&TILES, 8, Style::Tiles)
                } else {
                    self.paint(&SHEET, 8, Style::Sheet)
                },
                under: wood,
                gables: if self.dice.roll(9) < 0.3 {
                    wood
                } else {
                    wall.with(Style::Blank)
                },
            },
            chimney: self.dice.roll(10) < 0.6,
            gable_windows: Some(*WHITE),
            balcony: false,
            trim: None,
        }
    }

    /// A house of the subtropics (#136), as on Okinawa and Ishigaki: light plastered concrete
    /// under a flat roof behind a low parapet, or under a low hipped roof of red tiles with
    /// wide eaves against sun and rain; no chimney.
    fn tropical_house(&self) -> Design {
        let wall = self.facade(&TROPICAL_WALLS, Style::Plaster);
        let flat = !self.rectangular || self.dice.roll(4) < 0.55;
        // Concrete houses often have two storeys, tiled ones mostly one.
        let one_storey = if flat { 0.4 } else { 0.8 };
        let count = if self.dice.roll(3) < one_storey {
            1.0
        } else {
            2.0
        };
        Design {
            kind: self.kind,
            walls: storeys(count),
            windows: true,
            roof: if flat {
                Roof::Flat { parapet: 0.5 }
            } else {
                Roof::Hipped
            },
            pitch_degrees: 22.0 + 5.0 * self.dice.roll(6),
            max_rise: 3.5,
            overhang: self.overhang(0.9, 0.4),
            verge: self.overhang(0.9, 0.4),
            wall,
            base: None,
            roof_paint: RoofPaint {
                top: if flat {
                    self.paint(&FLAT, 8, Style::Flat)
                } else {
                    self.paint(&TILES[..2], 8, Style::Tiles)
                },
                under: wall.with(Style::Blank),
                gables: wall.with(Style::Blank),
            },
            chimney: false,
            gable_windows: None,
            balcony: false,
            trim: None,
        }
    }

    fn chalet(&self) -> Design {
        let wood = self.facade(&WOOD, Style::Timber);
        Design {
            kind: self.kind,
            walls: storeys(if self.dice.roll(3) < 0.65 { 2.0 } else { 3.0 }),
            windows: true,
            roof: Roof::Gable,
            pitch_degrees: 20.0 + 6.0 * self.dice.roll(6),
            max_rise: 5.0,
            overhang: self.overhang(1.2, 0.4),
            verge: self.overhang(1.4, 0.4),
            wall: wood,
            base: Some(self.paint(&LIGHT_PLASTER, 2, Style::Plaster)),
            roof_paint: RoofPaint {
                top: if self.dice.roll(7) < 0.7 {
                    self.paint(&MOUNTAIN_ROOFS, 8, Style::Tiles)
                } else {
                    self.paint(&SHEET, 8, Style::Sheet)
                },
                under: wood.with(Style::Boards),
                gables: wood.with(Style::Boards),
            },
            chimney: self.dice.roll(10) < 0.5,
            gable_windows: Some(*WHITE),
            balcony: true,
            trim: None,
        }
    }

    fn farmhouse(&self) -> Design {
        let wood = self.facade(&WOOD, Style::Timber);
        Design {
            kind: self.kind,
            walls: storeys(if self.dice.roll(3) < 0.6 { 1.0 } else { 2.0 }),
            windows: true,
            roof: if self.dice.roll(4) < 0.6 {
                Roof::Hipped
            } else {
                Roof::Gable
            },
            pitch_degrees: 40.0 + 8.0 * self.dice.roll(6),
            max_rise: 9.0,
            overhang: self.overhang(0.9, 0.3),
            verge: self.overhang(0.9, 0.3),
            wall: wood,
            base: Some(self.paint(&LIGHT_PLASTER, 2, Style::Plaster)),
            roof_paint: RoofPaint {
                top: self.paint(&TILES[..3], 8, Style::Tiles),
                under: wood.with(Style::Boards),
                gables: wood.with(Style::Boards),
            },
            chimney: self.dice.roll(10) < 0.4,
            gable_windows: Some(*WHITE),
            balcony: false,
            trim: None,
        }
    }

    fn shed(&self) -> Design {
        let wall = if self.dice.roll(2) < 0.6 {
            self.facade(&WOOD, Style::Boards)
        } else {
            self.facade(&PLASTER, Style::Blank)
        };
        Design {
            kind: self.kind,
            walls: 2.6,
            windows: false,
            roof: if self.rectangular {
                Roof::Gable
            } else {
                Roof::Flat { parapet: 0.0 }
            },
            pitch_degrees: 18.0 + 10.0 * self.dice.roll(6),
            max_rise: 2.5,
            overhang: 0.3,
            verge: 0.3,
            wall,
            base: None,
            roof_paint: RoofPaint {
                top: if self.dice.roll(7) < 0.5 {
                    self.paint(&SHEET, 8, Style::Sheet)
                } else {
                    self.paint(&TILES, 8, Style::Tiles)
                },
                under: wall,
                gables: wall,
            },
            chimney: false,
            gable_windows: None,
            balcony: false,
            trim: None,
        }
    }

    fn block(&self) -> Design {
        // Coloured or light walls, and trim that stands out against them, as in the
        // references (#75): white on colour, an accent colour on light walls.
        let palette: &[[f32; 3]] = match self.dice.roll(13) {
            r if r < 0.45 => &PLASTER,
            r if r < 0.75 => &LIGHT_PLASTER,
            _ => &MODERN,
        };
        let wall = self.facade(palette, Style::Plaster);
        let light = luminance(wall.rgb) > 0.85;
        let accent = if light {
            self.paint(&SHUTTERS, 14, Style::Blank)
        } else {
            Paint::new(*WHITE, Style::Blank)
        };
        let base = if self.dice.roll(15) < 0.5 {
            Paint::new(*STONE, Style::Plaster)
        } else {
            accent.with(Style::Plaster)
        };
        let band = accent;
        let balconies = (self.dice.roll(16) < 0.7).then_some(accent);
        let walls = storeys(4.0 + (self.dice.roll(3) * 3.0).floor());
        // Tall blocks are modern and flat-roofed; lower ones are as often hipped.
        let tall = mapped_height(self.building).unwrap_or(walls) >= 15.0;
        let flat = tall || !self.rectangular || self.dice.roll(4) < 0.6;
        Design {
            kind: self.kind,
            walls,
            windows: true,
            roof: if flat {
                Roof::Flat { parapet: 0.8 }
            } else {
                Roof::Hipped
            },
            pitch_degrees: 22.0 + 6.0 * self.dice.roll(6),
            max_rise: 4.0,
            overhang: 0.6,
            verge: 0.6,
            wall,
            base: Some(base),
            roof_paint: RoofPaint {
                top: if flat {
                    self.paint(&FLAT, 8, Style::Flat)
                } else {
                    self.paint(&TILES, 8, Style::Tiles)
                },
                under: wall.with(Style::Blank),
                gables: wall.with(Style::Blank),
            },
            chimney: false,
            gable_windows: None,
            balcony: false,
            trim: Some(Trim {
                band,
                balconies,
                floors: false,
                every_column: false,
            }),
        }
    }

    /// Flat-roofed, with a cornice in an accent colour and machinery on the roof.
    fn modern(&self, wall: Paint, base: Paint, trim: Trim, walls: f64) -> Design {
        Design {
            kind: self.kind,
            walls,
            windows: true,
            roof: Roof::Flat { parapet: 0.7 },
            pitch_degrees: 25.0,
            max_rise: 4.0,
            overhang: 0.5,
            verge: 0.5,
            wall,
            base: Some(base),
            roof_paint: RoofPaint {
                top: self.paint(&FLAT, 8, Style::Flat),
                under: wall.with(Style::Blank),
                gables: wall.with(Style::Blank),
            },
            chimney: false,
            gable_windows: None,
            balcony: false,
            trim: Some(trim),
        }
    }

    fn office(&self) -> Design {
        // Bands of glass between plain spandrels, over a glazed ground floor framed in the
        // accent colour.
        let wall = self.facade(&MODERN[..5], Style::Ribbon);
        let accent = self.paint(&ACCENTS, 14, Style::Blank);
        let trim = Trim {
            band: accent,
            balconies: None,
            floors: false,
            every_column: false,
        };
        let walls = storeys(3.0 + (self.dice.roll(3) * 4.0).floor());
        self.modern(wall, accent.with(Style::Shop), trim, walls)
    }

    fn public(&self) -> Design {
        // Either classic — light plaster over a stone ground floor under a hipped roof, white
        // trim — or modern like most schools and hospitals: flat-roofed, a band of colour at
        // every floor.
        let classic = self.rectangular && self.dice.roll(4) < 0.45;
        let walls = storeys(2.0 + (self.dice.roll(3) * 3.0).floor());
        if !classic {
            let wall = self.facade(&MODERN[..5], Style::Plaster);
            let trim = Trim {
                band: self.paint(&ACCENTS, 14, Style::Blank),
                balconies: None,
                floors: true,
                every_column: false,
            };
            return self.modern(wall, wall, trim, walls);
        }
        let wall = self.facade(&LIGHT_PLASTER, Style::Plaster);
        Design {
            kind: self.kind,
            walls,
            windows: true,
            roof: Roof::Hipped,
            pitch_degrees: 26.0 + 6.0 * self.dice.roll(6),
            max_rise: 5.0,
            overhang: 0.6,
            verge: 0.6,
            wall,
            base: Some(Paint::new(*STONE, Style::Plaster)),
            roof_paint: RoofPaint {
                top: self.paint(&TILES, 8, Style::Tiles),
                under: wall.with(Style::Blank),
                gables: wall.with(Style::Blank),
            },
            chimney: false,
            gable_windows: None,
            balcony: false,
            trim: Some(Trim {
                band: Paint::new(*WHITE, Style::Blank),
                balconies: None,
                floors: false,
                every_column: false,
            }),
        }
    }

    fn hotel(&self) -> Design {
        // Balconies in every column in the accent colour, a glazed ground floor.
        let wall = self.facade(&MODERN, Style::Plaster);
        let accent = self.paint(&ACCENTS, 14, Style::Blank);
        let trim = Trim {
            band: accent,
            balconies: Some(accent),
            floors: false,
            every_column: true,
        };
        let walls = storeys(3.0 + (self.dice.roll(3) * 4.0).floor());
        self.modern(wall, accent.with(Style::Shop), trim, walls)
    }

    fn hall(&self) -> Design {
        let wall = self.facade(&CLADDING, Style::Cladding);
        Design {
            kind: self.kind,
            walls: 6.0 + 3.0 * self.dice.roll(3),
            windows: false,
            roof: if self.rectangular && self.dice.roll(4) < 0.45 {
                Roof::Gable
            } else {
                Roof::Flat { parapet: 0.5 }
            },
            pitch_degrees: 8.0 + 6.0 * self.dice.roll(6),
            max_rise: 3.0,
            overhang: 0.4,
            verge: 0.4,
            wall,
            base: None,
            roof_paint: RoofPaint {
                top: self.paint(&SHEET, 8, Style::Sheet),
                under: wall.with(Style::Blank),
                gables: wall,
            },
            chimney: false,
            gable_windows: None,
            balcony: false,
            trim: None,
        }
    }
}

/// Walls, roof and details of every kind but churches; `walls`, if given, is the height of the
/// walls instead of what the map and the design say.
fn build(
    b: &mut Builder,
    footprint: &[Point],
    rect: Option<Rect>,
    design: &Design,
    building: &Building,
    walls: Option<f64>,
    dice: &Dice,
) {
    let outline = rect.map_or_else(|| footprint.to_vec(), |r| r.corners());
    let parapet = match design.roof {
        Roof::Flat { parapet } => Some(parapet),
        Roof::Gable | Roof::Hipped => None,
    };
    let (angle, rise) = match (parapet, rect) {
        (Some(_), _) => (design.pitch_degrees.to_radians(), 0.0),
        (None, Some(rect)) => pitch_over(design, &rect),
        (None, None) => (design.pitch_degrees.to_radians(), 2.0),
    };
    let walls = walls.unwrap_or_else(|| wall_height(building, rise, design));
    let ground = b.ground;
    let eaves = ground + walls;
    let wall_top = eaves + parapet.unwrap_or(0.0);
    let bottom = b.footing;
    match design.base {
        Some(base) if ground + STOREY < wall_top => {
            b.walls(&outline, bottom, ground + STOREY, base);
            b.walls(&outline, ground + STOREY, wall_top, design.wall);
        }
        Some(base) => b.walls(&outline, bottom, wall_top, base),
        None => b.walls(&outline, bottom, wall_top, design.wall),
    }

    let pitch = Pitch {
        eaves,
        angle,
        overhang: design.overhang,
        verge: design.verge,
    };
    let paint = design.roof_paint;
    let top = match (design.roof, rect) {
        (Roof::Flat { parapet }, _) => {
            b.flat_roof(&outline, eaves, parapet, paint.top, paint.gables);
            eaves
        }
        (Roof::Gable, Some(rect)) => b.gable_roof(&rect, pitch, paint),
        (Roof::Hipped, Some(rect)) => b.hipped_roof(&rect, pitch, paint),
        (_, None) => b.hip_band_roof(&outline, pitch, paint).unwrap_or_else(|| {
            b.flat_roof(
                &outline,
                eaves,
                0.0,
                paint.top.with(Style::Flat),
                paint.gables,
            );
            eaves
        }),
    };

    if let Some(trim) = design.trim {
        finish(
            b,
            &outline,
            rect.as_ref(),
            (ground, eaves, wall_top),
            design.base.is_some(),
            trim,
            dice,
        );
    }

    let Some(rect) = rect else {
        return;
    };
    if design.chimney && parapet.is_none() {
        chimney(b, &rect, design, pitch, top, dice);
    }
    if design.roof == Roof::Gable {
        if let Some(frame) = design.gable_windows {
            gable_windows(b, &rect, pitch, frame);
        }
        if design.balcony {
            balcony(b, &rect, walls, design.wall.rgb, dice);
        }
    }
    let tall_kind = matches!(
        design.kind,
        Kind::Block | Kind::Office | Kind::Public | Kind::Hotel
    );
    if tall_kind && parapet.is_some() && dice.roll(11) < 0.6 {
        // Stairs and lift machinery on the roof.
        let housing = Rect {
            centre: rect.point((dice.roll(12) - 0.5) * rect.half_length, 0.0),
            half_length: (rect.half_length * 0.3).min(2.0),
            half_width: (rect.half_width * 0.4).min(1.5),
            ..rect
        };
        b.cuboid(
            &housing,
            (eaves, eaves + 2.6),
            design.wall.with(Style::Blank),
            Paint::new(FLAT[1], Style::Flat),
            false,
        );
    }
}

/// A block's trim (#75): a string course over a ground storey of its own (`base`), a cornice
/// round the top of its walls, and on rectangular blocks balconies and, on a flat roof, units
/// for ventilation and cooling.
fn finish(
    b: &mut Builder,
    outline: &[Point],
    rect: Option<&Rect>,
    (ground, eaves, wall_top): (f64, f64, f64),
    base: bool,
    trim: Trim,
    dice: &Dice,
) {
    if trim.floors {
        // A band at every floor up to the top storey's.
        let mut floor = ground + STOREY;
        while floor < eaves - STOREY / 2.0 {
            b.band(outline, (floor - 0.2, floor + 0.2), 0.1, trim.band);
            floor += STOREY;
        }
    } else if base && ground + STOREY < eaves {
        let floor = ground + STOREY;
        b.band(outline, (floor - 0.12, floor + 0.12), 0.08, trim.band);
    }
    b.band(
        outline,
        (wall_top - CORNICE_DEPTH, wall_top + CORNICE_RISE),
        CORNICE_OUT,
        trim.band,
    );
    let Some(rect) = rect else {
        return;
    };
    if let Some(paint) = trim.balconies {
        balconies(b, rect, (ground, eaves), (paint, trim.every_column), dice);
    }
    if wall_top > eaves {
        let units = if dice.roll(17) < 0.4 {
            1
        } else if dice.roll(17) < 0.8 {
            2
        } else {
            3
        };
        let metal = Paint::new(*METAL, Style::Blank);
        for k in 0..units {
            let unit = Rect {
                centre: rect.point(
                    (dice.roll(18 + k) - 0.5) * 1.4 * rect.half_length,
                    (dice.roll(22 + k) - 0.5) * rect.half_width,
                ),
                half_length: 0.6,
                half_width: 0.4,
                ..*rect
            };
            b.cuboid(&unit, (eaves, eaves + 0.7), metal, metal, false);
        }
    }
}

/// The cornice: how far it reaches down the walls, rises over their top (or the parapet's),
/// capping them, and stands out of them.
const CORNICE_DEPTH: f64 = 0.6;
const CORNICE_RISE: f64 = 0.08;
const CORNICE_OUT: f64 = 0.25;

/// Relative luminance of an sRGB colour (roughly: its channels weighted as the eye does).
fn luminance([r, g, b]: [f32; 3]) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Balconies stand this far out of the wall.
const BALCONY_DEPTH: f64 = 1.2;

/// Stacked balconies on a block's long sides, one storey over another from the first floor up,
/// in every other column of windows or `every` one (never the outermost), each a solid box:
/// slab and balustrade in one, below its window.
fn balconies(
    b: &mut Builder,
    rect: &Rect,
    (ground, eaves): (f64, f64),
    (paint, every): (Paint, bool),
    dice: &Dice,
) {
    let length = 2.0 * rect.half_length;
    // The windows' columns, as the walls lay them out (whole windows, `walls_facing`).
    let columns = (length / parts::WINDOW_SPACING).round();
    let floors = ((eaves - ground) / STOREY).round() - 1.0;
    if columns < 3.0 || floors < 2.0 {
        return;
    }
    let spacing = length / columns;
    let parity = i64::from(dice.roll(19) < 0.5);
    #[allow(clippy::cast_possible_truncation)] // a few dozen columns and storeys
    for column in 1..columns as i64 - 1 {
        if !every && (column + parity) % 2 != 0 {
            continue;
        }
        #[allow(clippy::cast_precision_loss)] // small numbers
        let along = -rect.half_length + (column as f64 + 0.5) * spacing;
        for side in [-1.0, 1.0] {
            let balcony = Rect {
                centre: rect.point(along, side * (rect.half_width + BALCONY_DEPTH / 2.0)),
                half_length: spacing * 0.42,
                half_width: BALCONY_DEPTH / 2.0,
                ..*rect
            };
            for floor in 1..=floors as i64 {
                #[allow(clippy::cast_precision_loss)] // a few storeys
                let level = ground + floor as f64 * STOREY;
                b.cuboid(&balcony, (level - 0.15, level + 1.0), paint, paint, true);
            }
        }
    }
}

/// The slope of a design's roof over `rect`, flatter on wide buildings so it rises no more
/// than the design allows, and how high it rises.
fn pitch_over(design: &Design, rect: &Rect) -> (f64, f64) {
    let angle = design
        .pitch_degrees
        .to_radians()
        .min((design.max_rise / rect.half_width).atan());
    (angle, rect.half_width * angle.tan())
}

/// How high a design's roof rises over `rect`.
fn rise(design: &Design, rect: &Rect) -> f64 {
    match design.roof {
        Roof::Flat { .. } => 0.0,
        Roof::Gable | Roof::Hipped => pitch_over(design, rect).1,
    }
}

/// A church's walls (the mapped colour if there is one) and roof.
fn church_paints(building: &Building, dice: &Dice) -> (Paint, Paint) {
    let wall = Paint::new(
        dice.tint(building.color.map_or_else(
            || dice.pick(1, &LIGHT_PLASTER),
            |mapped| nearest(mapped, &LIGHT_PLASTER),
        )),
        Style::Church,
    );
    let roof = Paint::new(
        dice.pick(2, &[TILES[3], TILES[4], TILES[2], TILES[1]]),
        Style::Tiles,
    );
    (wall, roof)
}

/// A castle's walls (the mapped colour if there is one) and roofs.
fn castle_paints(building: &Building, dice: &Dice) -> (Paint, Paint) {
    let wall = Paint::new(
        dice.tint(building.color.map_or_else(
            || dice.pick(1, &CASTLE_WALLS),
            |mapped| nearest(mapped, &CASTLE_WALLS),
        )),
        Style::Blank,
    );
    let roof = Paint::new(dice.pick(2, &[TILES[0], TILES[1], TILES[3]]), Style::Tiles);
    (wall, roof)
}

/// A lighthouse's white, and the colour of its bands and cap: mostly red, now and then
/// another.
fn lighthouse_paints(dice: &Dice) -> (Paint, Paint) {
    let band = if dice.roll(2) < 0.75 {
        BEACON[0]
    } else {
        dice.pick(3, &BEACON[1..])
    };
    (
        Paint::new(*WHITE, Style::Blank),
        Paint::new(band, Style::Blank),
    )
}

/// A castle (#137): walls rising to a parapet round a steep hipped roof and, on a
/// rectangular keep, round towers under pointed roofs at its corners, as the models have.
fn castle(
    b: &mut Builder,
    plot: &Plot,
    area: f64,
    rect: Option<Rect>,
    dice: &Dice,
    fit: Option<Fit>,
) {
    let (wall, roof) = castle_paints(plot.building, dice);
    let outline = rect.map_or_else(|| plot.footprint.clone(), |r| r.corners());
    let walls = fit.map_or_else(
        || {
            plot.building
                .height
                .map_or(0.5 * area.sqrt(), |height| height * 0.6)
                .clamp(9.0, 16.0)
        },
        |fit| fit.model.eaves,
    );
    let eaves = b.ground + walls;
    let parapet = 1.0;
    b.walls(&outline, b.footing, eaves + parapet, wall);
    b.flat_roof(
        &outline,
        eaves,
        parapet,
        Paint::new(FLAT[0], Style::Flat),
        wall,
    );
    let Some(rect) = rect else {
        return;
    };
    let paint = RoofPaint {
        top: roof,
        under: wall,
        gables: wall,
    };
    let keep = Rect {
        half_length: rect.half_length - 0.8,
        half_width: rect.half_width - 0.8,
        ..rect
    };
    if keep.half_width > 1.0 {
        let pitch = Pitch {
            eaves: eaves + 0.2,
            angle: 48f64.to_radians().min((10.0 / keep.half_width).atan()),
            overhang: 0.0,
            verge: 0.0,
        };
        b.hipped_roof(&keep, pitch, paint);
    }
    let radius = (rect.half_width * 0.3).clamp(2.0, 4.0);
    let tower_top = eaves + 6.0;
    for (along, across) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let centre = rect.point(along * rect.half_length, across * rect.half_width);
        b.walls(
            &octagon(centre, radius, rect.axis),
            b.footing,
            tower_top,
            wall,
        );
        let cap = Rect::square(centre, rect.axis, radius + 0.3);
        let spire = Pitch {
            eaves: tower_top,
            angle: 2.2f64.atan(),
            overhang: 0.0,
            verge: 0.0,
        };
        b.hipped_roof(&cap, spire, paint);
    }
}

/// An eight-sided outline round `centre`, counter-clockwise, turned with `axis`.
fn octagon(centre: Point, radius: f64, axis: Point) -> Vec<Point> {
    (0..8)
        .map(|k| {
            let angle = std::f64::consts::TAU * (f64::from(k) + 0.5) / 8.0;
            let (sin, cos) = angle.sin_cos();
            (
                centre.0 + radius * (axis.0 * cos - axis.1 * sin),
                centre.1 + radius * (axis.1 * cos + axis.0 * sin),
            )
        })
        .collect()
}

/// A lighthouse (#137): its outline (round as mapped) rising in bands of white and colour from
/// a stone plinth to a gallery, a glazed lantern under a pointed cap on top; as high as
/// mapped, as its model, or by its width.
fn lighthouse(b: &mut Builder, plot: &Plot, rect: Option<Rect>, dice: &Dice, fit: Option<Fit>) {
    let outline = &plot.footprint;
    let Some(around) = rect.or_else(|| Rect::around(outline)) else {
        return;
    };
    let (white, band) = lighthouse_paints(dice);
    let stone = Paint::new(*STONE, Style::Blank);
    let gallery = b.ground
        + fit.map_or_else(
            || {
                plot.building
                    .height
                    .map_or(around.half_width * 6.8, |height| height - 4.0)
                    .clamp(10.0, 40.0)
            },
            |fit| fit.model.eaves,
        );
    let plinth = b.ground + 1.0;
    b.walls(outline, b.footing, plinth, stone);
    let bands = 7;
    let step = (gallery - plinth) / f64::from(bands);
    for k in 0..bands {
        let paint = if k % 2 == 1 { band } else { white };
        let low = plinth + f64::from(k) * step;
        b.walls(outline, low, low + step, paint);
    }
    let deck = gallery + 0.35;
    b.band(outline, (gallery, deck), 0.9, stone);
    b.flat_roof(outline, deck, 0.0, stone, stone);
    let lantern = Rect::square(
        around.centre,
        around.axis,
        (around.half_width * 0.5).max(1.0),
    );
    let glazed = deck + 2.4;
    b.cuboid(
        &lantern,
        (deck, glazed),
        Paint::new(*METAL, Style::Window),
        Paint::new(*METAL, Style::Blank),
        false,
    );
    let cap = Pitch {
        eaves: glazed,
        angle: 1.2f64.atan(),
        overhang: 0.2,
        verge: 0.0,
    };
    let paint = RoofPaint {
        top: band.with(Style::Sheet),
        under: Paint::new(*METAL, Style::Blank),
        gables: band,
    };
    b.hipped_roof(&lantern, cap, paint);
}

/// Height of the walls above the ground, from the map if it knows (counting half of a
/// roof rising `rise` into a mapped height), else the design's.
fn wall_height(building: &Building, rise: f64, design: &Design) -> f64 {
    let walls = match (building.levels, building.height) {
        (Some(levels), _) => storeys(levels),
        (None, Some(height)) => height - rise / 2.0,
        (None, None) => design.walls,
    };
    if design.windows {
        whole_storeys(walls)
    } else {
        walls.max(2.2)
    }
}

fn chimney(b: &mut Builder, rect: &Rect, design: &Design, pitch: Pitch, top: f64, dice: &Dice) {
    let half = 0.3;
    let along = (dice.roll(20) - 0.5) * rect.half_length;
    let across = if dice.roll(21) < 0.5 { -0.4 } else { 0.4 } * rect.half_width;
    // How far in from the eaves the chimney stands, which sets the roof's height there.
    let inset = match design.roof {
        Roof::Hipped => (rect.half_width - across.abs()).min(rect.half_length - along.abs()),
        Roof::Gable | Roof::Flat { .. } => rect.half_width - across.abs(),
    };
    let slope = pitch.angle.tan();
    let roof = pitch.eaves + (inset - half) * slope;
    let stack = Rect::square(rect.point(along, across), rect.axis, half);
    let sides = if dice.roll(22) < 0.3 {
        Paint::new(*BRICK, Style::Blank)
    } else {
        design.wall.with(Style::Blank)
    };
    b.cuboid(
        &stack,
        (roof - 0.1, top + 0.6),
        sides,
        Paint::new(*SOOT, Style::Flat),
        false,
    );
}

/// One window, or two in wide gables, where they fit under the roof.
fn gable_windows(b: &mut Builder, rect: &Rect, pitch: Pitch, frame: [f32; 3]) {
    let (sill, lintel) = (0.5, 1.6);
    let rise = rect.half_width * pitch.angle.tan();
    if rise < lintel + 0.4 {
        return;
    }
    // Half the gable's width at the windows' top.
    let room = rect.half_width * (1.0 - lintel / rise);
    let positions: &[f64] = if room >= 2.0 {
        &[-1.2, 1.2]
    } else if room >= 0.75 {
        &[0.0]
    } else {
        &[]
    };
    for end in [-1.0, 1.0] {
        let out = (rect.axis.0 * end, rect.axis.1 * end);
        for &across in positions {
            b.window(
                rect.point(end * rect.half_length, across),
                out,
                0.45,
                (pitch.eaves + sill, pitch.eaves + lintel),
                frame,
            );
        }
    }
}

/// A wooden balcony along one gable, at the floor of the top storey.
fn balcony(b: &mut Builder, rect: &Rect, walls: f64, wood: [f32; 3], dice: &Dice) {
    let span = rect.half_width - 0.5;
    if span < 2.0 {
        return;
    }
    let top_storey = ((walls - EAVES_MARGIN) / STOREY).round() - 1.0;
    if top_storey < 1.0 {
        return;
    }
    let floor = b.ground + top_storey * STOREY;
    let end = if dice.roll(30) < 0.5 { -1.0 } else { 1.0 };
    let depth = 1.2;
    let slab = Rect {
        centre: rect.point(end * (rect.half_length + depth / 2.0), 0.0),
        axis: rect.across(),
        half_length: span,
        half_width: depth / 2.0,
    };
    let railing = Rect {
        centre: rect.point(end * (rect.half_length + depth - 0.04), 0.0),
        half_width: 0.04,
        ..slab
    };
    let boards = Paint::new(wood, Style::Boards);
    b.cuboid(&slab, (floor - 0.2, floor), boards, boards, true);
    b.cuboid(&railing, (floor, floor + 1.0), boards, boards, false);
}

/// A church: a high nave under a steep roof, and a tower with a spire at one end, or a
/// turret on the roof of a chapel.
fn church(b: &mut Builder, plot: &Plot, area: f64, rect: Option<Rect>, dice: &Dice) {
    let building = plot.building;
    let chapel = area < CHAPEL_AREA;
    let (wall, roof) = church_paints(building, dice);
    let blank = wall.with(Style::Blank);
    let paint = RoofPaint {
        top: roof,
        under: blank,
        gables: blank,
    };
    let outline = rect.map_or_else(|| plot.footprint.clone(), |r| r.corners());
    let mut angle = (48.0 + 7.0 * dice.roll(3)).to_radians();
    let rise = match rect {
        Some(rect) => {
            angle = angle.min((12.0 / rect.half_width).atan());
            rect.half_width * angle.tan()
        }
        None => 2.0,
    };
    // Mapped heights of churches are mostly the tower's; only low ones can be the nave's.
    let walls = match building.height {
        Some(height) if height < 20.0 => height - rise / 2.0,
        _ => 0.45 * area.sqrt(),
    }
    .clamp(if chapel { 6.5 } else { 7.5 }, 14.0);
    let ground = b.ground;
    let eaves = ground + walls;
    b.walls(&outline, b.footing, eaves, wall);
    let pitch = Pitch {
        eaves,
        angle,
        overhang: 0.4,
        verge: 0.3,
    };
    let ridge = match rect {
        Some(rect) => b.gable_roof(&rect, pitch, paint),
        None => b.hip_band_roof(&outline, pitch, paint).unwrap_or_else(|| {
            b.flat_roof(&outline, eaves, 0.0, roof.with(Style::Flat), blank);
            eaves
        }),
    };

    // Towers stand at one end of the nave, also when the outline is irregular.
    let Some(nave) = rect.or_else(|| Rect::around(&plot.footprint)) else {
        return;
    };
    let steeple = Steeple {
        nave,
        end: if dice.roll(4) < 0.5 { -1.0 } else { 1.0 },
        ridge,
        wall,
        paint: RoofPaint {
            top: Paint::new(dice.pick(5, &SPIRES), Style::Tiles),
            under: blank,
            gables: blank,
        },
    };
    if chapel {
        steeple.turret(b, angle, roof);
    } else {
        steeple.tower(b, building.height, dice);
    }
}

/// Where a church's tower or turret goes and how it looks.
struct Steeple {
    nave: Rect,
    /// Which end of the nave: −1 or 1 along its axis.
    end: f64,
    /// Height of the nave's ridge.
    ridge: f64,
    wall: Paint,
    paint: RoofPaint,
}

impl Steeple {
    /// A small turret with a needle on the ridge of a chapel whose roof slopes at `angle`.
    fn turret(&self, b: &mut Builder, angle: f64, roof: Paint) {
        let half = 0.7;
        let turret = Rect::square(
            self.nave
                .point(self.end * (self.nave.half_length - 1.6).max(0.0), 0.0),
            self.nave.axis,
            half,
        );
        let base = self.ridge - (half + 0.3) * angle.tan();
        let top = self.ridge + 1.8;
        b.cuboid(&turret, (base, top), self.paint.under, roof, false);
        let needle = Pitch {
            eaves: top,
            angle: (2.6 / half).atan(),
            overhang: 0.05,
            verge: 0.0,
        };
        b.hipped_roof(&turret, needle, self.paint);
    }

    /// A bell tower at the end of the nave, or beside it there, under a needle spire, a
    /// saddle roof or a pyramid; as high as mapped if the map knows.
    fn tower(&self, b: &mut Builder, mapped: Option<f64>, dice: &Dice) {
        let nave = &self.nave;
        let half = (nave.half_width * 0.55).clamp(2.25, 4.0);
        let side = dice.roll(6);
        let across = if side < 0.7 {
            0.0
        } else {
            (if side < 0.85 { -1.0 } else { 1.0 }) * (nave.half_width - half).max(0.0)
        };
        let tower = Rect::square(
            nave.point(self.end * (nave.half_length - half).max(0.0), across),
            nave.axis,
            half,
        );
        let style = dice.roll(7);
        let (spire_angle, spire_rise) = if style < 0.6 {
            // A needle.
            let rise = half * (3.6 + 1.6 * dice.roll(8));
            ((rise / half).atan(), rise)
        } else if style < 0.85 {
            // A saddle roof across the nave.
            (60f64.to_radians(), half * 60f64.to_radians().tan())
        } else {
            // A pyramid.
            (55f64.to_radians(), half * 55f64.to_radians().tan())
        };
        let top = match mapped {
            Some(height) if height >= 20.0 => b.ground + height - spire_rise,
            _ => self.ridge + 2.0 * half + 5.0 * dice.roll(9),
        }
        .max(self.ridge + 3.0);
        b.walls(&tower.corners(), b.footing, top, self.wall);
        belfry(b, &tower, top);
        let pitch = Pitch {
            eaves: top,
            angle: spire_angle,
            overhang: if style < 0.6 { 0.05 } else { 0.3 },
            verge: 0.3,
        };
        if (0.6..0.85).contains(&style) {
            b.gable_roof(&tower.turned(), pitch, self.paint);
        } else {
            b.hipped_roof(&tower, pitch, self.paint);
        }
    }
}

/// Sound openings on every side of a bell tower, below its roof.
fn belfry(b: &mut Builder, tower: &Rect, top: f64) {
    let half = tower.half_length;
    let positions: &[f64] = if half >= 2.6 { &[-0.8, 0.8] } else { &[0.0] };
    let across = tower.across();
    for out in [
        tower.axis,
        across,
        (-tower.axis.0, -tower.axis.1),
        (-across.0, -across.1),
    ] {
        let along = (-out.1, out.0);
        for &offset in positions {
            let centre = (
                tower.centre.0 + out.0 * half + along.0 * offset,
                tower.centre.1 + out.1 * half + along.1 * offset,
            );
            b.window(centre, out, 0.32, (top - 3.4, top - 1.0), *STONE);
        }
    }
}

/// The outline a building is drawn on, for keeping the road clear: models and rectangular
/// shells stand on the rectangle fitted around its footprint, which can reach beyond the
/// footprint itself (a clipped corner drawn whole), and holds all of it (#138).
pub(crate) fn drawn_outline(footprint: &[Point]) -> Vec<Point> {
    Rect::around(footprint).map_or_else(|| footprint.to_vec(), |rect| rect.corners())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plot(building: &Building, setting: Setting, church: bool) -> Plot<'_> {
        Plot {
            building,
            footprint: Vec::new(),
            setting,
            church,
            shop: None,
            purpose: None,
            landmark: None,
            climate: Climate::Temperate,
        }
    }

    fn untagged(id: i64) -> Building {
        Building {
            id,
            outline: Vec::new(),
            height: None,
            levels: None,
            color: None,
        }
    }

    /// Kinds of 200 buildings with different ids.
    fn kinds(setting: Setting, area: f64, ground: f64) -> Vec<Kind> {
        (0..200)
            .map(|id| {
                let building = untagged(id);
                kind(&plot(&building, setting, false), area, ground, &Dice(id))
            })
            .collect()
    }

    #[test]
    fn mountain_houses_are_chalets() {
        assert!(
            kinds(Setting::Countryside, 120.0, 1300.0)
                .iter()
                .all(|&k| k == Kind::Chalet)
        );
        assert!(
            kinds(Setting::Countryside, 120.0, 400.0)
                .iter()
                .all(|&k| k == Kind::House)
        );
        // In between, both.
        let mixed = kinds(Setting::Countryside, 120.0, 900.0);
        assert!(mixed.contains(&Kind::Chalet) && mixed.contains(&Kind::House));
    }

    #[test]
    fn size_and_surroundings_set_the_kind() {
        let all = |kinds: Vec<Kind>, expected: Kind| kinds.iter().all(|&k| k == expected);

        assert!(all(
            kinds(Setting::Countryside, 600.0, 450.0),
            Kind::Farmhouse
        ));
        assert!(all(kinds(Setting::Countryside, 20.0, 450.0), Kind::Shed));
        assert!(all(kinds(Setting::Industrial, 900.0, 450.0), Kind::Hall));
        assert!(all(kinds(Setting::Town, 900.0, 450.0), Kind::Block));
        assert!(all(kinds(Setting::Town, 140.0, 450.0), Kind::House));
    }

    #[test]
    fn tall_buildings_are_blocks_and_marked_ones_churches() {
        let mut tall = untagged(1);
        tall.height = Some(24.0);
        let house = untagged(2);

        let dice = Dice(1);
        assert_eq!(
            kind(
                &plot(&tall, Setting::Countryside, false),
                300.0,
                450.0,
                &dice
            ),
            Kind::Block
        );
        assert_eq!(
            kind(&plot(&house, Setting::Town, true), 300.0, 450.0, &dice),
            Kind::Church
        );
    }

    #[test]
    fn what_is_in_a_building_sets_its_kind() {
        let building = untagged(4);
        let with = |purpose, area| {
            let plot = Plot {
                purpose: Some(purpose),
                ..plot(&building, Setting::Town, false)
            };
            kind(&plot, area, 450.0, &Dice(4))
        };

        assert_eq!(with(Purpose::Hotel, 400.0), Kind::Hotel);
        assert_eq!(with(Purpose::Office, 400.0), Kind::Office);
        assert_eq!(with(Purpose::Public, 600.0), Kind::Public);
        // A guest house or a doctor's office in a family house stays a house.
        assert_eq!(with(Purpose::Hotel, 110.0), Kind::House);
        assert_eq!(with(Purpose::Office, 110.0), Kind::House);
        // Churches stay churches, whatever else is mapped in them.
        let church = Plot {
            purpose: Some(Purpose::Public),
            ..plot(&building, Setting::Town, true)
        };
        assert_eq!(kind(&church, 600.0, 450.0, &Dice(4)), Kind::Church);
    }

    #[test]
    fn commercial_land_has_offices_and_stores_and_public_land_public_buildings() {
        let all = |kinds: Vec<Kind>, expected: Kind| kinds.iter().all(|&k| k == expected);

        assert!(all(kinds(Setting::Commercial, 600.0, 450.0), Kind::Office));
        assert!(all(kinds(Setting::Commercial, 4000.0, 450.0), Kind::Hall));
        assert!(all(kinds(Setting::Commercial, 50.0, 450.0), Kind::Shed));
        assert!(all(kinds(Setting::Public, 800.0, 450.0), Kind::Public));
        // A caretaker's house on the school grounds.
        assert!(all(kinds(Setting::Public, 120.0, 450.0), Kind::House));
    }

    #[test]
    fn points_mark_the_building_they_lie_in_or_beside() {
        let (a, b) = (untagged(1), untagged(2));
        let square = |east: f64| {
            vec![
                (east, 0.0),
                (east + 10.0, 0.0),
                (east + 10.0, 10.0),
                (east, 10.0),
            ]
        };
        let mut plots = vec![
            Plot {
                footprint: square(0.0),
                ..plot(&a, Setting::Town, false)
            },
            Plot {
                footprint: square(50.0),
                ..plot(&b, Setting::Town, false)
            },
        ];

        // An office inside the first, a hotel at the second's door; nothing for one far off.
        mark_purpose(&mut plots, &[(5.0, 5.0)], Purpose::Office);
        mark_purpose(&mut plots, &[(55.0, -4.0), (55.0, -60.0)], Purpose::Hotel);
        assert_eq!(plots[0].purpose, Some(Purpose::Office));
        assert_eq!(plots[1].purpose, Some(Purpose::Hotel));
        // A public building wins over an office in the same building, whichever comes first.
        mark_purpose(&mut plots, &[(5.0, 5.0)], Purpose::Public);
        mark_purpose(&mut plots, &[(5.0, 5.0)], Purpose::Office);
        assert_eq!(plots[0].purpose, Some(Purpose::Public));
    }

    #[test]
    fn mapped_heights_are_rounded_to_whole_storeys() {
        // A 2-storey house mapped 8 m high: 6.4 m of walls under a 3.2 m roof.
        assert!((whole_storeys(8.0 - 3.2 / 2.0) - 6.4).abs() < 1e-9);
        // Too low for a storey still gets one.
        assert!((whole_storeys(1.5) - 3.4).abs() < 1e-9);
    }

    #[test]
    fn church_points_mark_the_building_they_lie_in_or_the_largest_nearby() {
        let square = |e: f64, n: f64, half: f64| {
            vec![
                (e - half, n - half),
                (e + half, n - half),
                (e + half, n + half),
                (e - half, n + half),
            ]
        };
        let (shed, church, house) = (untagged(1), untagged(2), untagged(3));
        let mut plots = vec![
            Plot {
                footprint: square(0.0, 0.0, 2.0),
                ..plot(&shed, Setting::Town, false)
            },
            Plot {
                footprint: square(20.0, 0.0, 10.0),
                ..plot(&church, Setting::Town, false)
            },
            Plot {
                footprint: square(200.0, 0.0, 5.0),
                ..plot(&house, Setting::Town, false)
            },
        ];

        // In the churchyard, between the shed and the church.
        mark_churches(&mut plots, &[(5.0, 3.0)]);
        assert!(!plots[0].church && plots[1].church && !plots[2].church);

        // On the house itself.
        mark_churches(&mut plots, &[(201.0, 1.0)]);
        assert!(plots[2].church);
    }

    #[test]
    fn castle_and_lighthouse_points_mark_their_buildings() {
        let square = |e: f64, n: f64, half: f64| {
            vec![
                (e - half, n - half),
                (e + half, n - half),
                (e + half, n + half),
                (e - half, n + half),
            ]
        };
        let buildings: Vec<Building> = (1..=5).map(untagged).collect();
        let mut plots: Vec<Plot> = [
            // A castle's stable and its keep.
            (0.0, 0.0, 4.0, false),
            (30.0, 0.0, 12.0, false),
            // A lighthouse and the keeper's larger house beside it.
            (200.0, 0.0, 3.0, false),
            (212.0, 0.0, 6.0, false),
            // A church.
            (400.0, 0.0, 8.0, true),
        ]
        .iter()
        .zip(&buildings)
        .map(|(&(e, n, half, church), building)| Plot {
            footprint: square(e, n, half),
            ..plot(building, Setting::Countryside, church)
        })
        .collect();

        // On the castle grounds between the two; beside the tower; on the church.
        mark_landmarks(&mut plots, &[(9.0, 5.0)], Landmark::Castle);
        mark_landmarks(&mut plots, &[(200.0, 4.5)], Landmark::Lighthouse);
        mark_landmarks(&mut plots, &[(400.0, 0.0)], Landmark::Castle);

        let marks: Vec<Option<Landmark>> = plots.iter().map(|p| p.landmark).collect();
        assert_eq!(
            marks,
            [
                None,
                Some(Landmark::Castle),
                Some(Landmark::Lighthouse),
                None,
                None
            ]
        );
        assert_eq!(kind(&plots[1], 576.0, 450.0, &Dice(2)), Kind::Castle);
        // A lighthouse is no shed, however small.
        assert_eq!(kind(&plots[2], 36.0, 5.0, &Dice(3)), Kind::Lighthouse);
        assert_eq!(kind(&plots[4], 256.0, 450.0, &Dice(5)), Kind::Church);
    }

    #[test]
    fn the_subtropics_have_houses_built_for_the_heat() {
        let tropical = |id: i64, area: f64, ground: f64| {
            let building = untagged(id);
            let plot = Plot {
                climate: Climate::Tropical,
                ..plot(&building, Setting::Countryside, false)
            };
            kind(&plot, area, ground, &Dice(id))
        };
        // Neither chalets on the hills nor Bernese farmhouses: houses.
        for id in 0..200 {
            assert_eq!(tropical(id, 120.0, 900.0), Kind::House);
            assert_eq!(tropical(id, 600.0, 20.0), Kind::House);
        }
        // Up in the highlands the tropics are temperate.
        assert!((0..200).all(|id| tropical(id, 120.0, 1500.0) == Kind::Chalet));

        let designs: Vec<Design> = (0..100)
            .map(|id| design(Kind::House, &untagged(id), true, true, &Dice(id)))
            .collect();
        let flat = designs
            .iter()
            .filter(|d| matches!(d.roof, Roof::Flat { .. }))
            .count();
        assert!((20..80).contains(&flat), "{flat} flat roofs");
        for d in &designs {
            assert!(!d.chimney);
            assert!(luminance(d.wall.rgb) > 0.7, "light walls: {:?}", d.wall.rgb);
            if d.roof == Roof::Hipped {
                assert!(d.pitch_degrees < 28.0 && d.overhang >= 0.9);
            } else {
                assert!(matches!(d.roof, Roof::Flat { .. }), "{:?}", d.roof);
            }
        }
        // Outside the tropics the same houses keep pitched roofs and chimneys.
        let temperate: Vec<Design> = (0..100)
            .map(|id| design(Kind::House, &untagged(id), true, false, &Dice(id)))
            .collect();
        assert!(
            temperate
                .iter()
                .all(|d| d.roof != Roof::Flat { parapet: 0.5 })
        );
        assert!(temperate.iter().any(|d| d.chimney));
    }
}
