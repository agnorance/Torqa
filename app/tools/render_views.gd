extends SceneTree
## Renders Torqa's standard views — fixed shots on the fixture routes — into $OUT_DIR/<view>.png,
## so every visual change can be compared before and after against the same pictures (ADR 0011).
## One run covers the views of route $ROUTE; scripts/render-views.sh runs all routes, and $VIEWS
## (space-separated names) limits it to some. A $ROUTE ending in .gpx is any route's file, shot
## as $SHOTS says: space-separated name:distance:camera, optionally :right,up,back for a camera
## standing aside.

## View name → [route, distance along it in metres, camera (`RideWorld.CameraMode`), time of
## day, weather], and optionally where a camera standing aside looks at the rider from (metres
## right, up and back of the rider), e.g. to see a bridge from the side.
const VIEWS: Dictionary[String, Array] = {
	"village-chase": ["gurtenstrasse", 150.0, 0, "Midday", "Clear"],
	"climb-chase": ["gurtenstrasse", 1840.0, 0, "Midday", "Clear"],
	"hairpin-drone": ["gurtenstrasse", 1240.0, 2, "Midday", "Clear"],
	"hairpin-chase": ["gurtenstrasse", 1290.0, 0, "Midday", "Clear"],
	"village-evening": ["gurtenstrasse", 150.0, 0, "Evening", "Clear"],
	"railway-tunnel-drone": ["bielersee", 1900.0, 2, "Midday", "Clear"],
	"shops-side": ["bielersee", 590.0, 0, "Midday", "Clear", Vector3(6.0, 7.0, -75.0)],
	"lake-chase": ["bielersee", 3000.0, 0, "Midday", "Clear"],
	"lake-drone": ["bielersee", 5000.0, 2, "Midday", "Clear"],
	"lake-rain": ["bielersee", 3000.0, 0, "Midday", "Rain"],
	"lake-morning": ["bielersee", 3000.0, 0, "Morning", "Clear"],
	"climb-morning-drone": ["gurtenstrasse", 1840.0, 2, "Morning", "Hazy"],
	"valley-morning": ["gurtenstrasse", 1840.0, 0, "Morning", "Clear", Vector3(0.0, 14.0, -45.0)],
	"lakeside-village": ["bielersee", 5150.0, 0, "Midday", "Clear"],
	"block-chase": ["bielersee", 4960.0, 0, "Midday", "Clear"],
	"junction-chase": ["bielersee", 1500.0, 0, "Midday", "Clear"],
	"junction-drone": ["bielersee", 1380.0, 2, "Midday", "Clear"],
	"river-drone": ["kirchenfeldbruecke", 90.0, 2, "Midday", "Clear"],
	"oldtown-drone": ["kirchenfeldbruecke", 0.0, 2, "Midday", "Clear"],
	"bridge-chase": ["kirchenfeldbruecke", 200.0, 0, "Midday", "Clear"],
	"roundabout-drone": ["kirchenfeldbruecke", 170.0, 2, "Midday", "Clear"],
	"bridge-side": ["kirchenfeldbruecke", 380.0, 0, "Midday", "Clear", Vector3(140.0, -22.0, 0.0)],
	# The reference route (#167): a planner's file over a pass, its hard places.
	"oberalp-tunnel-chase": ["oberalp", 3700.0, 0, "Midday", "Clear"],
	"oberalp-railway-chase": ["oberalp", 4500.0, 0, "Midday", "Clear"],
	"oberalp-hairpins-drone": ["oberalp", 6100.0, 2, "Midday", "Clear"],
	"oberalp-lake-drone": ["oberalp", 10900.0, 2, "Midday", "Clear"],
	"oberalp-portal-drone": ["oberalp", 11600.0, 2, "Midday", "Clear"],
	"oberalp-village-chase": ["oberalp", 24000.0, 0, "Midday", "Clear"],
}
## Frames to let the world stream in around a new place; software rendering is slow.
const SETTLE_FRAMES: int = 240

var _main: Control


func _initialize() -> void:
	_main = (load("res://scenes/main.tscn") as PackedScene).instantiate()
	root.add_child(_main)
	_run.call_deferred()


func _run() -> void:
	var route: String = OS.get_environment("ROUTE")
	var out: String = OS.get_environment("OUT_DIR")
	DirAccess.make_dir_recursive_absolute(out)
	var wanted: PackedStringArray = OS.get_environment("VIEWS").split(" ", false)
	var specs: Dictionary[String, Array] = VIEWS.duplicate()
	var gpx: String = ProjectSettings.globalize_path("res://../core/fixtures/%s.gpx" % route)
	if route.ends_with(".gpx"):
		gpx = route
		specs.clear()
		for shot: String in OS.get_environment("SHOTS").split(" ", false):
			var parts: PackedStringArray = shot.split(":")
			specs[parts[0]] = [route, float(parts[1]), int(parts[2]), "Midday", "Clear"]
			if parts.size() > 3:
				var aside: PackedFloat64Array = parts[3].split_floats(",")
				specs[parts[0]].append(Vector3(aside[0], aside[1], aside[2]))
	var views: Array[String] = []
	for view: String in specs:
		var spec: Array = specs[view]
		if spec[0] == route and (wanted.is_empty() or wanted.has(view)):
			views.append(view)
	if views.is_empty():
		quit(0)
		return
	var torqa: TorqaApp = _main.get_node("Torqa")
	var start: StartPage = _main.get_node("StartPage")
	var world: RideWorld = _main.get_node("World")
	var quality: String = OS.get_environment("QUALITY")
	torqa.set_graphics_quality(quality if not quality.is_empty() else "medium")
	torqa.load_route(gpx, true)
	var built: Array[bool] = [false]
	torqa.world_ready.connect(
		func(_info: Variant = null) -> void: built[0] = true, CONNECT_ONE_SHOT
	)
	while not built[0]:
		await process_frame
	torqa.connect_fake_trainer(150.0, 85.0)
	torqa.start_ride(50.0, false, {"kind": "none"})
	start.ride_started.emit(start.ride_options())
	for view: String in views:
		var spec: Array = specs[view]
		var distance: float = spec[1]
		var camera: int = spec[2]
		var time: String = spec[3]
		var weather: String = spec[4]
		torqa.jump_to_distance(distance)
		world.set_camera(camera)
		world.apply_conditions(time, weather)
		if spec.size() > 5:
			# A camera aside stands where the rider was just after the jump and stays there
			# while the world settles; the rider rides on meanwhile.
			for frame: int in range(10):
				await process_frame
			var aside: Vector3 = spec[5]
			var rider: Node3D = world.get_node("Rider")
			var camera_node: Camera3D = world.get("_camera")
			world.set("_free", true)
			camera_node.look_at_from_position(
				rider.transform * aside, rider.position + Vector3.UP * 2.0, Vector3.UP
			)
		for frame: int in range(SETTLE_FRAMES):
			await process_frame
		root.get_texture().get_image().save_png(out.path_join(view + ".png"))
		print("saved %s" % view)
	quit(0)
