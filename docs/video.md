# Video courses

Ride a route along a real video (R17): the video plays as fast as you ride — climb slowly and
it slows down, sprint and it speeds up. The 3D world is not used on video courses.

## What you can import

**Import** on the Courses tab takes:

- **Your own videos with GPS** — GoPro recordings (`.mp4`, `.mov`; `.m4v` and `.mkv` are
  read too) carry their GPS track inside the file (GPMF). Torqa reads it, builds the route
  from it and pairs every moment of the video with a position on it.
- **Tacx Real Life Videos** — an `.rlv` file with its `.pgmf` course and the video (often
  `.avi`), side by side. Choose the **`.rlv` file**. The RLV says how far the camera moved per
  frame, the PGMF the slopes; Torqa pairs them. RLV courses know distance and slope but not
  *where* they are: they are ridden along their video only (no 3D world, no map position),
  with the slopes on the trainer as recorded. The video's name in the RLV may be an old
  Windows path; Torqa looks for that file name (or the RLV's own name) next to the `.rlv`.
- **Any other video** is added to the course of its GPX route — see below. Importing a video
  without GPS here shows these steps instead.
- **Route videos made for Incyclist** — a folder with a video, a `.gpx` route and an `.xml`
  control file. Choose the **`.xml` file**. Many free route videos are published in this format,
  for example the library by Van Gestel offered from within Incyclist. Keep the three files
  together; the GPX timestamps (and the start frame in the `.xml`) place the route in the video.

The route goes through the usual import (elevation correction when online) and the course is
added to your library, marked **Video** on its card.

## Videos without GPS

A video without GPS cannot tell where it was filmed, so it is added to a course:

1. Import the **GPX route** of the ride shown in the video (Courses tab).
2. Open that course and press **Add video…**, then choose the video.
3. **Align video with route** opens with two **sync points**: the route's **start** and
   **end**. Select one and set the moment of the video showing that place, with the slider
   and the ±1 s / ±0.1 s buttons, checking the frame above.
4. Where the footage stops or changes speed (a traffic light, a steep climb), **Add point**:
   set its place on the route (shown on the elevation profile) and its moment in the video.

The course is now ridden along the video. Between neighbouring points the video follows your
distance evenly; the GPX's own timestamps are not used (they come from another recording).
**Align video…** changes the points later; the course file keeps the change.

**Remove video** on any video course's page takes the video off: the course is then ridden
in 3D only. Courses made from a video with GPS follow their GPS and have
nothing to align.

## Riding a video course

Open the course and press **Ride**: Torqa asks whether to ride **along the video** or **in
3D** — a video course is a route like any other, so its 3D world is there too (built the
first time you choose it; online, it fetches the terrain and map data the video course does
not hold yet). Along the video, the video fills the screen, with your figures, map and
elevation profile on top. Where you are on the route
decides the moment of the video: it plays at the speed you ride, stands still when you stop,
and blends smoothly from frame to frame even when you crawl up a steep climb.

The video's own **sound** plays along at the same speed, without sounding higher or deeper:
it is stretched, not sped up like a tape. It fades out when you slow to a crawl or stop.
Switch it off under **Sound** in the ride options (course page or **Settings** while riding).

Videos are decoded on the processor for now, on all its cores. 1080p is the target; larger
videos are scaled down while playing, which may not keep up on a small machine — hardware
decoding is planned.

## The video stays beside the course

Videos are large, so a video course (`.tqc`) only **refers** to its video, it does not contain
it. When a course is prepared, Torqa keeps the course's own copy of the video next to the
course file in the [library](courses.md): a hard link where the library lies on the same disk
as the video, which takes no extra space, else a copy. The course rides on when the file it was
imported from is moved or deleted. Opening a course looks for the video where the course keeps
it, then next to the course file (by name and size). To move a video course to another
computer, copy the `.tqc` and the video into the same folder. If the video is missing, opening
the course says which file to put there. Deleting a course deletes its copy of the video too,
unless another course rides along the same file; a video anywhere else stays where it is.

## Licences

Route videos belong to whoever filmed them; the free ones are usually for personal use only
(e.g. CC BY-NC-SA). Torqa does not ship or redistribute any videos — share courses together
with their videos only where the video's licence allows it.

Video is decoded with FFmpeg (LGPL); AV1 videos (common for downloadable route videos) with
rav1d (BSD-2-Clause). See [ADR 0010](adr/0010-video-decoding.md).
