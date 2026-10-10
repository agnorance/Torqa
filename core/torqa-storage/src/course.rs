//! Course files (`.tqc`, R32–R35, ADR 0007): one zip file holding a prepared course, so it can be
//! kept, shared and ridden offline on any machine.
//!
//! Layout:
//!
//! ```text
//! manifest.json   format and generator versions, name, key figures, attribution
//! route.gpx       the original track
//! data/…          the downloaded terrain and map files the course was built from,
//!                 under their paths relative to the download cache
//! ```
//!
//! Opening a course puts the data files back into the cache, after which the course is built
//! offline exactly as when it was prepared — always with the current world generator (R35).

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// File extension of course files.
pub const EXTENSION: &str = "tqc";
/// The course file format written by this version; newer formats are rejected.
pub const FORMAT_VERSION: u32 = 1;

const MANIFEST: &str = "manifest.json";
const ROUTE: &str = "route.gpx";
const DATA: &str = "data/";
/// The course's map picture, drawn by the app when the course was prepared (#192).
const PREVIEW: &str = "preview.png";

/// Reading or writing a course file failed.
#[derive(Debug, thiserror::Error)]
pub enum CourseError {
    /// File system error.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// The file is not a valid zip container.
    #[error("not a course file: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// The manifest is missing fields or malformed.
    #[error("invalid course manifest: {0}")]
    Manifest(#[from] serde_json::Error),
    /// The file was written by a newer Torqa.
    #[error(
        "course format {0} is newer than this Torqa supports ({FORMAT_VERSION}); please update"
    )]
    NewerFormat(u32),
    /// A data file to pack lies outside the data root.
    #[error("{} is not inside {}", .0.display(), .1.display())]
    OutsideRoot(PathBuf, PathBuf),
    /// An entry would be extracted outside the data root.
    #[error("unsafe path in course file: {0}")]
    UnsafePath(String),
}

/// Describes a course; stored as `manifest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Course file format version.
    pub format: u32,
    /// The program that prepared the course, e.g. `Torqa 0.1.0`.
    pub generator: String,
    /// Display name.
    pub name: String,
    /// Route length in metres.
    pub length_m: f64,
    /// Total climbing in metres.
    pub elevation_gain_m: f64,
    /// Steepest climbing gradient in percent.
    pub max_grade_percent: f64,
    /// When the course was prepared, in seconds since the Unix epoch.
    pub created_unix_s: u64,
    /// Credits the bundled data requires, to show wherever the course is used.
    pub attribution: Vec<String>,
    /// Fingerprint of the route, to find the rider's records and avoid duplicates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_key: Option<String>,
    /// Thinned track for course cards (R37): metres east/north of the start. Optional fields
    /// like this one are read as empty by older Torqa versions and need no format bump.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub track: Vec<[f32; 2]>,
    /// Thinned elevation profile for course cards: (distance m, elevation m).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile: Vec<[f32; 2]>,
    /// For video courses (R17): the video, which is referenced rather than embedded (R35).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoReference>,
}

/// The video a video course plays, found again by path, or by name next to the course file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoReference {
    /// Where the video was when the course was prepared.
    pub path: String,
    /// Its file name, to find it next to the course file after moving both.
    pub file_name: String,
    /// Its size in bytes, to tell it from another file of the same name.
    pub size: u64,
    /// Where the route's first point sits in the video, in seconds.
    pub offset_s: f64,
    /// Where the route's last point sits in the video, in seconds, for videos aligned by hand
    /// (no GPS): the video is spread evenly between the two. `None` when GPS or the GPX's
    /// timestamps pair them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_s: Option<f64>,
    /// All marks of a video aligned by hand, `[distance m, video s]` from the route's start to
    /// its end (the first and last are `offset_s` and `end_s`); empty for courses with only
    /// those two.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<[f64; 2]>,
    /// Whether the route is a real place; `false` for courses known only by distance and
    /// slope (Tacx RLV), which are ridden along their video only.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub located: bool,
}

fn yes() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes the field by reference
fn is_yes(value: &bool) -> bool {
    *value
}

impl VideoReference {
    /// The video file: at its recorded path, else next to the course file `course`; `None` if
    /// neither holds a file of the recorded size.
    #[must_use]
    pub fn locate(&self, course: &Path) -> Option<PathBuf> {
        let candidates = [
            PathBuf::from(&self.path),
            course
                .parent()
                .map(|dir| dir.join(&self.file_name))
                .unwrap_or_default(),
        ];
        candidates
            .into_iter()
            .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() == self.size))
    }
}

/// A course read back from its file.
#[derive(Debug, Clone, PartialEq)]
pub struct Unpacked {
    /// The manifest.
    pub manifest: Manifest,
    /// The original GPX track.
    pub gpx: String,
}

/// Writes a course file at `path`, bundling the `data` files, which must lie under `data_root`.
/// The file appears atomically: readers never see a half-written course.
///
/// # Errors
/// [`CourseError::OutsideRoot`] for a data file outside `data_root`, or an I/O or zip error.
pub fn write(
    path: &Path,
    manifest: &Manifest,
    gpx: &str,
    data_root: &Path,
    data: &[PathBuf],
) -> Result<(), CourseError> {
    let partial = path.with_extension(format!("{EXTENSION}.part"));
    let result = write_to(&partial, manifest, gpx, data_root, data)
        .and_then(|()| Ok(std::fs::rename(&partial, path)?));
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

fn write_to(
    path: &Path,
    manifest: &Manifest,
    gpx: &str,
    data_root: &Path,
    data: &[PathBuf],
) -> Result<(), CourseError> {
    let mut zip = ZipWriter::new(BufWriter::new(File::create(path)?));
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    // Images are compressed already; deflating them again only costs time.
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    zip.start_file(MANIFEST, deflated)?;
    serde_json::to_writer_pretty(&mut zip, manifest)?;
    zip.start_file(ROUTE, deflated)?;
    zip.write_all(gpx.as_bytes())?;
    for file in data {
        let relative = file
            .strip_prefix(data_root)
            .map_err(|_| CourseError::OutsideRoot(file.clone(), data_root.to_owned()))?;
        let name = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let is_image = matches!(
            relative.extension().and_then(|e| e.to_str()),
            Some("jpg" | "jpeg" | "png" | "webp")
        );
        zip.start_file(
            format!("{DATA}{name}"),
            if is_image { stored } else { deflated },
        )?;
        io::copy(&mut BufReader::new(File::open(file)?), &mut zip)?;
    }
    zip.finish()?.flush()?;
    Ok(())
}

/// Reads only the manifest, e.g. for listing courses.
///
/// # Errors
/// [`CourseError::NewerFormat`] for files from a newer Torqa, or an I/O, zip or manifest error.
pub fn read_manifest(path: &Path) -> Result<Manifest, CourseError> {
    let mut zip = ZipArchive::new(BufReader::new(File::open(path)?))?;
    manifest_of(&mut zip)
}

/// Reads a course and puts its data files under `data_root`. Files already there are kept: the
/// cache paths are versioned, so an existing file has the same content.
///
/// # Errors
/// [`CourseError::NewerFormat`] for files from a newer Torqa, [`CourseError::UnsafePath`] for
/// entries that would land outside `data_root`, or an I/O, zip or manifest error.
pub fn unpack(path: &Path, data_root: &Path) -> Result<Unpacked, CourseError> {
    let mut zip = ZipArchive::new(BufReader::new(File::open(path)?))?;
    let manifest = manifest_of(&mut zip)?;
    let mut gpx = String::new();
    zip.by_name(ROUTE)?.read_to_string(&mut gpx)?;

    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let Some(name) = entry.name().strip_prefix(DATA).map(ToOwned::to_owned) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let relative = Path::new(&name);
        if !relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(CourseError::UnsafePath(entry.name().to_owned()));
        }
        let target = data_root.join(relative);
        if target.exists() {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let partial = target.with_extension("part");
        io::copy(&mut entry, &mut File::create(&partial)?)?;
        std::fs::rename(&partial, &target)?;
    }
    Ok(Unpacked { manifest, gpx })
}

/// Replaces the manifest of the course at `path`, e.g. to rename it; the other entries are
/// copied unchanged. The file is replaced only once the new one is complete.
///
/// # Errors
/// [`CourseError::NewerFormat`] for files from a newer Torqa, or an I/O, zip or manifest error.
pub fn rewrite_manifest(path: &Path, manifest: &Manifest) -> Result<(), CourseError> {
    rewrite(path, MANIFEST, |out| {
        out.start_file(
            MANIFEST,
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )?;
        serde_json::to_writer_pretty(out, manifest)?;
        Ok(())
    })
}

/// The course's map picture, if it has one: the PNG the app drew when it was prepared (#192).
#[must_use]
pub fn preview(path: &Path) -> Option<Vec<u8>> {
    let mut zip = ZipArchive::new(BufReader::new(File::open(path).ok()?)).ok()?;
    let mut entry = zip.by_name(PREVIEW).ok()?;
    let mut png = Vec::new();
    entry.read_to_end(&mut png).ok()?;
    Some(png)
}

/// Puts the course's map picture `png` into its file, in place of an earlier one (#192).
///
/// # Errors
/// If the course cannot be read or written.
pub fn set_preview(path: &Path, png: &[u8]) -> Result<(), CourseError> {
    rewrite(path, PREVIEW, |out| {
        // A PNG is compressed already.
        out.start_file(
            PREVIEW,
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )?;
        out.write_all(png)?;
        Ok(())
    })
}

/// Writes the course again with the entry `replaced` written by `write` and everything else
/// copied as it is; the file appears whole or not at all.
fn rewrite(
    path: &Path,
    replaced: &str,
    write: impl FnOnce(&mut ZipWriter<BufWriter<File>>) -> Result<(), CourseError>,
) -> Result<(), CourseError> {
    let mut zip = ZipArchive::new(BufReader::new(File::open(path)?))?;
    manifest_of(&mut zip)?;
    let partial = path.with_extension("part");
    let mut out = ZipWriter::new(BufWriter::new(File::create(&partial)?));
    let written = (|| {
        write(&mut out)?;
        for index in 0..zip.len() {
            let entry = zip.by_index_raw(index)?;
            if entry.name() != replaced {
                out.raw_copy_file(entry)?;
            }
        }
        out.finish()?.flush()?;
        Ok::<(), CourseError>(())
    })();
    match written {
        Ok(()) => Ok(std::fs::rename(&partial, path)?),
        Err(error) => {
            let _ = std::fs::remove_file(&partial);
            Err(error)
        }
    }
}

fn manifest_of<R: io::Read + io::Seek>(zip: &mut ZipArchive<R>) -> Result<Manifest, CourseError> {
    let mut json = String::new();
    zip.by_name(MANIFEST)?.read_to_string(&mut json)?;
    // Check the version before the fields: a newer format may have changed them.
    let format = serde_json::from_str::<serde_json::Value>(&json)?["format"]
        .as_u64()
        .and_then(|f| u32::try_from(f).ok())
        .unwrap_or(0);
    if format > FORMAT_VERSION {
        return Err(CourseError::NewerFormat(format));
    }
    Ok(serde_json::from_str(&json)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("torqa-course-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest() -> Manifest {
        Manifest {
            format: FORMAT_VERSION,
            generator: "Torqa test".to_owned(),
            name: "Lake Biel".to_owned(),
            length_m: 12_345.0,
            elevation_gain_m: 120.0,
            max_grade_percent: 6.5,
            created_unix_s: 1_790_000_000,
            attribution: vec!["Terrain: Mapterhorn (CC BY 4.0)".to_owned()],
            route_key: Some("0123456789abcdef".to_owned()),
            track: vec![[0.0, 0.0], [10.0, 250.5]],
            profile: vec![[0.0, 500.0], [12_345.0, 620.0]],
            video: None,
        }
    }

    #[test]
    fn a_course_keeps_its_map_picture() {
        let dir = temp_dir("preview");
        let path = dir.join("lake.tqc");
        write(&path, &manifest(), "<gpx/>", &dir, &[]).unwrap();
        assert_eq!(preview(&path), None);

        set_preview(&path, b"PNG one").unwrap();
        assert_eq!(preview(&path).as_deref(), Some(&b"PNG one"[..]));
        // Drawn again (a newer map): replaced, the rest of the course as it was.
        set_preview(&path, b"PNG two").unwrap();

        assert_eq!(preview(&path).as_deref(), Some(&b"PNG two"[..]));
        let read = read_manifest(&path).unwrap();
        assert_eq!(read.name, manifest().name);
        assert_eq!(unpack(&path, &dir.join("out")).unwrap().gpx, "<gpx/>");
    }

    #[test]
    fn renaming_keeps_the_route_and_data() {
        let dir = temp_dir("rename");
        let cache = dir.join("cache");
        let tile = cache.join("terrain/t.webp");
        std::fs::create_dir_all(tile.parent().unwrap()).unwrap();
        std::fs::write(&tile, b"tile").unwrap();
        let course = dir.join("c.tqc");
        write(
            &course,
            &manifest(),
            "<gpx/>",
            &cache,
            std::slice::from_ref(&tile),
        )
        .unwrap();

        let mut renamed = read_manifest(&course).unwrap();
        renamed.name = "Bielersee".to_owned();
        rewrite_manifest(&course, &renamed).unwrap();

        assert_eq!(read_manifest(&course).unwrap(), renamed);
        let unpacked = unpack(&course, &dir.join("other")).unwrap();
        assert_eq!(unpacked.gpx, "<gpx/>");
        assert_eq!(
            std::fs::read(dir.join("other/terrain/t.webp")).unwrap(),
            b"tile"
        );
        assert!(!dir.join("c.part").exists());
    }

    #[test]
    fn a_course_moves_its_data_to_another_machine() {
        let dir = temp_dir("roundtrip");
        let prepared = dir.join("cache-a");
        let tile = prepared.join("terrain/mapterhorn/15/1/2.webp");
        let map = prepared.join("osm/v2/14/8531/5770.pbf");
        for (file, content) in [(&tile, b"tile".as_slice()), (&map, b"map".as_slice())] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, content).unwrap();
        }
        let course = dir.join("lake-biel.tqc");

        write(
            &course,
            &manifest(),
            "<gpx/>",
            &prepared,
            &[tile.clone(), map.clone()],
        )
        .unwrap();
        let other = dir.join("cache-b");
        let unpacked = unpack(&course, &other).unwrap();

        assert_eq!(unpacked.manifest, manifest());
        assert_eq!(unpacked.gpx, "<gpx/>");
        assert_eq!(read_manifest(&course).unwrap(), manifest());
        assert_eq!(
            std::fs::read(other.join("terrain/mapterhorn/15/1/2.webp")).unwrap(),
            b"tile"
        );
        assert_eq!(
            std::fs::read(other.join("osm/v2/14/8531/5770.pbf")).unwrap(),
            b"map"
        );
        assert!(!dir.join("lake-biel.tqc.part").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn video_courses_find_their_video_after_moving() {
        let dir = temp_dir("video");
        let video = dir.join("ride.mp4");
        std::fs::write(&video, b"0123456789").unwrap();
        let reference = VideoReference {
            path: dir.join("gone/ride.mp4").display().to_string(),
            file_name: "ride.mp4".to_owned(),
            size: 10,
            offset_s: 0.0,
            end_s: None,
            marks: Vec::new(),
            located: true,
        };

        assert_eq!(reference.locate(&dir.join("ride.tqc")), Some(video.clone()));
        let other_size = VideoReference {
            size: 11,
            ..reference
        };
        assert_eq!(other_size.locate(&dir.join("ride.tqc")), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_courses_from_a_newer_torqa() {
        let dir = temp_dir("newer");
        let course = dir.join("future.tqc");
        let future = Manifest {
            format: FORMAT_VERSION + 1,
            ..manifest()
        };
        write(&course, &future, "<gpx/>", &dir, &[]).unwrap();

        let error = read_manifest(&course).unwrap_err();

        assert!(matches!(error, CourseError::NewerFormat(f) if f == FORMAT_VERSION + 1));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn never_extracts_outside_the_data_root() {
        let dir = temp_dir("unsafe");
        let course = dir.join("evil.tqc");
        let mut zip = ZipWriter::new(File::create(&course).unwrap());
        let options = SimpleFileOptions::default();
        zip.start_file(MANIFEST, options).unwrap();
        serde_json::to_writer(&mut zip, &manifest()).unwrap();
        zip.start_file(ROUTE, options).unwrap();
        zip.start_file("data/../escaped.txt", options).unwrap();
        zip.write_all(b"x").unwrap();
        zip.finish().unwrap();

        let error = unpack(&course, &dir.join("cache")).unwrap_err();

        assert!(matches!(error, CourseError::UnsafePath(_)), "{error}");
        assert!(!dir.join("escaped.txt").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refuses_to_pack_files_outside_the_data_root() {
        let dir = temp_dir("outside");
        let stray = dir.join("stray.bin");
        std::fs::write(&stray, b"x").unwrap();

        let error = write(
            &dir.join("c.tqc"),
            &manifest(),
            "",
            &dir.join("cache"),
            &[stray],
        )
        .unwrap_err();

        assert!(matches!(error, CourseError::OutsideRoot(..)));
        assert!(!dir.join("c.tqc").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
