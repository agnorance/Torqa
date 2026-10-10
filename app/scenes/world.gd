class_name RideWorld
extends Node3D
## The 3D world: terrain and road from Torqa, a rider following the ride state and cameras.

enum CameraMode { CHASE, FIRST_PERSON, DRONE }

# i18n-begin: time-of-day and weather names are shown in the setup screen.
## Sun elevation and azimuth (degrees, azimuth clockwise from north), sun and sky light energy
## per time of day, with the palette's names of its sun and sky colours. A low sun gets more sky
## light, so mornings and evenings stay bright and pastel.
const TIMES: Dictionary[String, Dictionary] = {
	"Morning":
	{
		"elevation": 14.0,
		"azimuth": 110.0,
		"energy": 0.75,
		"ambient": 0.68,
		"sun": "light.morning_sun",
		"top": "sky.morning_top",
		"horizon": "sky.morning_horizon",
	},
	"Midday":
	{
		"elevation": 55.0,
		"azimuth": 190.0,
		"energy": 0.8,
		"ambient": 0.55,
		"sun": "light.sun",
		"top": "sky.top",
		"horizon": "sky.horizon",
	},
	"Evening":
	{
		"elevation": 12.0,
		"azimuth": 265.0,
		"energy": 0.75,
		"ambient": 0.72,
		"sun": "light.evening_sun",
		"top": "sky.evening_top",
		"horizon": "sky.evening_horizon",
	},
}
const WEATHERS: Array[String] = ["Clear", "Cloudy", "Hazy", "Rain"]
# i18n-end

## Chunks are turned into meshes gradually, so building the world never stalls a frame.
const CHUNKS_PER_FRAME: int = 6
## Chunks further than this are hidden; fog hides the edge.
const VISIBILITY_RANGE: float = 4500.0
## The land beyond the corridor reaches 12 km from the route (`torqa_world::HORIZON`); the
## camera sees that far, the haze hides its end.
const HORIZON_RANGE: float = 14000.0
## Buildings switch between their models and their shells over this distance.
const MODEL_FADE: float = 40.0
## Trees and buildings are small; beyond this the land-cover colours carry the scene.
const DETAIL_RANGE: float = 1800.0
## Bushes and rocks are smaller still.
const SMALL_PLANT_RANGE: float = 600.0
const CAMERA_SMOOTHING: float = 6.0
## Leaning in and out of bends takes a moment (1 / seconds).
const LEAN_SMOOTHING: float = 3.0
## The free camera of simulated rides (#53): metres per second, radians per pixel of mouse and
## radians per second with Shift + arrows.
const FREE_SPEED: float = 25.0
const FREE_LOOK: float = 0.004
const FREE_TURN: float = 1.2
## A rider moving further than this between frames jumped: the camera follows at once.
const JUMP_M: float = 50.0
## Graphics presets (R43): what each turns on. Medium holds 60 fps on a base M1; `distance`
## scales how far terrain and details are drawn, `rain` is the number of raindrops. MSAA stays
## at 2×: at 4× a few distant pixels broke and the glow spread them into bright blobs. Flat
## colours gain nothing from global illumination (SSIL, SDFGI) or volumetric fog, which only
## greyed them (#103); what they cost goes to grass, models and shadows further out.
const QUALITY: Dictionary[String, Dictionary] = {
	"low":
	{
		"model_range": 200.0,
		"grass_range": 0.0,
		"grass_shadows": false,
		"shadow_atlas": 2048,
		"shadow_distance": 180.0,
		"shadow_splits": 2,
		"soft_shadows": 0.0,
		"ssao": false,
		"glow": false,
		"msaa": Viewport.MSAA_DISABLED,
		"fxaa": true,
		"render_scale": 0.77,
		"distance": 0.65,
		"rain": 1200,
	},
	"medium":
	{
		"model_range": 400.0,
		"grass_range": 60.0,
		"grass_shadows": false,
		"shadow_atlas": 4096,
		"shadow_distance": 300.0,
		"shadow_splits": 2,
		"soft_shadows": 0.0,
		"ssao": true,
		"glow": true,
		"msaa": Viewport.MSAA_2X,
		"fxaa": false,
		"render_scale": 1.0,
		"distance": 1.0,
		"rain": 2500,
	},
	"high":
	{
		"model_range": 600.0,
		"grass_range": 120.0,
		"grass_shadows": true,
		"shadow_atlas": 4096,
		"shadow_distance": 600.0,
		"shadow_splits": 4,
		"soft_shadows": 0.5,
		"ssao": true,
		"glow": true,
		"msaa": Viewport.MSAA_2X,
		"fxaa": false,
		"render_scale": 1.0,
		"distance": 1.4,
		"rain": 4000,
	},
	"ultra":
	{
		"model_range": 900.0,
		"grass_range": 180.0,
		"grass_shadows": true,
		"shadow_atlas": 8192,
		"shadow_distance": 900.0,
		"shadow_splits": 4,
		"soft_shadows": 0.7,
		"ssao": true,
		"glow": true,
		"msaa": Viewport.MSAA_2X,
		"fxaa": false,
		"render_scale": 1.0,
		"distance": 1.8,
		"rain": 6000,
	},
}
## Haze over the distance (density per metre) per weather: pastel, the colour of the horizon.
const HAZE: Dictionary[String, float] = {
	"Clear": 0.00035, "Cloudy": 0.0005, "Hazy": 0.0016, "Rain": 0.0012
}
## Low sun hazes the distance a little more: mornings most. Kept small, as more turned the whole
## view milky (#115).
const HAZE_BY_TIME: Dictionary[String, float] = {"Morning": 1.3, "Midday": 1.0, "Evening": 1.15}
## How much a low sun brightens the haze on its side.
const LOW_SUN_SCATTER: float = 0.08
## Fog lying in low ground (#103): how thick, per time of day and per weather, and how high it
## reaches over the route's lowest ground (metres).
const VALLEY_FOG_BY_TIME: Dictionary[String, float] = {
	"Morning": 1.0, "Midday": 0.1, "Evening": 0.4
}
const VALLEY_FOG_BY_WEATHER: Dictionary[String, float] = {
	"Clear": 0.5, "Cloudy": 0.7, "Hazy": 1.0, "Rain": 0.9
}
const VALLEY_FOG_DEPTH: float = 30.0
## Density gained per metre below the valley fog's top at full thickness.
const VALLEY_FOG_DENSITY: float = 0.01
## Godot's height fog hangs on height alone, not distance: whatever lies below its top is veiled
## alike, the road at the rider's wheel as much as the far shore. So its top stays this far
## below the rider: the valleys below fill with it, the rider's surroundings stay clear (#115).
const VALLEY_FOG_CLEARANCE: float = 8.0
## Raindrops fall around the camera from this far above it.
const RAIN_ABOVE: float = 9.0
## How fast the clouds drift, in cloud-layer units per second.
const CLOUD_DRIFT: Vector2 = Vector2(0.004, 0.0015)
# i18n-begin
const FREE_CAMERA: String = "Free"
# i18n-end

var _torqa: TorqaApp
var _chunk_count: int = 0
var _next_chunk: int = 0
var _camera_mode: CameraMode = CameraMode.CHASE
var _sky: ShaderMaterial = ShaderMaterial.new()
var _cloud_offset: Vector2 = Vector2.ZERO
var _quality: Dictionary = QUALITY["medium"]
## The route's lowest ground (metres), where valley fog lies.
var _low_ground: float = 0.0
## Where the rider is (metres above sea level), which the valley fog stays below.
var _rider_elevation: float = INF
var _time_of_day: String = "Midday"
var _weather: String = "Clear"
## Draw-distance factor of the current preset.
var _distance: float = 1.0
## Flying freely instead of following the rider (simulated rides only).
var _free: bool = false
var _free_speed: float = 1.0
## How far the rider and the ghost lean into the bend (radians, positive to the right).
var _lean: float = 0.0
var _ghost_lean: float = 0.0
var _placed: bool = false
var _avatar: RiderAvatar = RiderAvatar.new()
var _ghost: RiderAvatar = RiderAvatar.new()
var _ghost_distance: float = 0.0
var _clouds: CloudLayer = CloudLayer.new()
## Whether the clouds are over the land of the current world yet.
var _clouds_settled: bool = false
## The land beyond the corridor: ground and lakes.
var _horizon_ground: MeshInstance3D = MeshInstance3D.new()
var _horizon_water: MeshInstance3D = MeshInstance3D.new()

var _terrain_material: ShaderMaterial = ShaderMaterial.new()
var _road_material: ShaderMaterial = ShaderMaterial.new()
var _water_material: ShaderMaterial = ShaderMaterial.new()
var _building_material: ShaderMaterial = ShaderMaterial.new()
var _structure_material: StandardMaterial3D = StandardMaterial3D.new()
var _grass_mesh: ArrayMesh = _plant_mesh(false)
var _flower_mesh: ArrayMesh = _plant_mesh(true)
var _plant_material: ShaderMaterial = ShaderMaterial.new()
## The wind's clock for the grass and tree shaders: stands still while the ride is paused, so
## nothing moves in the scene and the swaying grass's shadows stop flickering (#178).
var _wind_time: float = 0.0
var _paused: bool = false
## Other streets of the map (asphalt) and tracks and paths (gravel).
var _street_material: ShaderMaterial = ShaderMaterial.new()
var _track_material: ShaderMaterial = ShaderMaterial.new()
var _rail_material: ShaderMaterial = ShaderMaterial.new()
## The railways near the route, on their own lines like the road (#85).
var _railways: MeshInstance3D = MeshInstance3D.new()

@onready var _terrain: Node3D = $Terrain
@onready var _road: MeshInstance3D = $Road
@onready var _structures: MeshInstance3D = $Structures
@onready var _rider: Node3D = $Rider
@onready var _camera: Camera3D = $Camera
@onready var _sun: DirectionalLight3D = $Sun
@onready var _environment: Environment = ($Environment as WorldEnvironment).environment
@onready var _rain: GPUParticles3D = $Camera/Rain


func bind(torqa: TorqaApp) -> void:
	_torqa = torqa
	_torqa.world_ready.connect(_on_world_ready)
	# A switch of riders changes the avatar on the bike at once, in a world built already.
	_torqa.profile_changed.connect(_apply_rider)


## Whether the free camera flies (its arrows move it).
func is_free_camera() -> bool:
	return _free


## Cycles chase → first person → drone (→ free in simulated rides) and returns the new
## mode's name.
func cycle_camera() -> String:
	if not _free and _camera_mode == CameraMode.DRONE and _torqa != null and _torqa.simulating():
		_free = true
		_avatar.show_rider(true)
		return FREE_CAMERA
	set_camera(CameraMode.CHASE if _free else (_camera_mode + 1) % CameraMode.size())
	return camera_names()[_camera_mode]


## The camera modes' names, in `CameraMode` order.
static func camera_names() -> PackedStringArray:
	var names: PackedStringArray = PackedStringArray()
	for key: String in CameraMode.keys():
		names.append(key.capitalize())
	return names


## The current camera mode (`CameraMode`).
func camera() -> int:
	return _camera_mode


## Switches to camera `mode` (`CameraMode`).
func set_camera(mode: int) -> void:
	_free = false
	_camera_mode = clampi(mode, 0, CameraMode.size() - 1) as CameraMode
	# From the rider's own eyes only the bike is visible.
	_avatar.show_rider(_camera_mode != CameraMode.FIRST_PERSON)


## Sets the light, sky, fog and precipitation for a time of day and a weather.
func apply_conditions(time_of_day: String, weather: String) -> void:
	var time: Dictionary = TIMES.get(time_of_day, TIMES["Midday"])
	var elevation: float = time["elevation"]
	var azimuth: float = time["azimuth"]
	# The light shines along its −z axis: from the sun's direction towards the ground.
	_sun.rotation = Vector3(deg_to_rad(-elevation), deg_to_rad(180.0 - azimuth), 0.0)
	var top_name: String = time["top"]
	var horizon_name: String = time["horizon"]
	var sun_name: String = time["sun"]
	var top: Color = Palette.color(top_name)
	var horizon: Color = Palette.color(horizon_name)
	var sun_color: Color = Palette.color(sun_name)
	var energy: float = time["energy"]
	var ambient: float = time["ambient"]
	_time_of_day = time_of_day
	_weather = weather
	var overcast: float = 0.0
	# Fair-weather clouds even on clear days: a bare gradient looked artificial.
	var cover: float = 0.3
	match weather:
		"Cloudy":
			overcast = 0.75
			cover = 0.8
		"Hazy":
			overcast = 0.3
			cover = 0.45
		"Rain":
			overcast = 1.0
			cover = 1.0
	var wind: Dictionary[String, float] = {"Clear": 0.08, "Cloudy": 0.14, "Hazy": 0.04, "Rain": 0.2}
	var wind_strength: float = wind.get(weather, 0.08)
	_plant_material.set_shader_parameter("wind_strength", wind_strength)
	VegetationModels.set_wind(wind_strength)
	# Grey weather stays pastel (ADR 0011): soft grey-blue instead of dull grey.
	var sky_top: Color = top.lerp(Palette.color("sky.overcast_top"), overcast)
	var sky_horizon: Color = horizon.lerp(Palette.color("sky.overcast_horizon"), overcast)
	_sky.set_shader_parameter("top_color", sky_top)
	_sky.set_shader_parameter("horizon_color", sky_horizon)
	# The sky paints a thin high layer; the low-poly clouds below carry the cover.
	_sky.set_shader_parameter("cloud_cover", cover * 0.3)
	_sky.set_shader_parameter("cloud_darkness", overcast)
	_clouds.cover = cover
	_sun.light_color = sun_color.lerp(Color.WHITE, overcast * 0.5)
	# The light shines along its −z axis: its +z points towards the sun.
	_clouds.light(_sun.global_transform.basis.z, _sun.light_color, sky_horizon, overcast)
	_sun.light_energy = energy * lerpf(1.0, 0.25, overcast)
	_sun.shadow_blur = lerpf(1.0, 4.0, overcast)
	_environment.ambient_light_energy = lerpf(ambient, 0.85, overcast)
	_environment.fog_density = (
		HAZE.get(weather, HAZE["Clear"]) * HAZE_BY_TIME.get(time_of_day, 1.0)
	)
	_environment.fog_light_color = sky_horizon
	# A low sun lights the haze from its side.
	_environment.fog_sun_scatter = LOW_SUN_SCATTER if elevation < 20.0 else 0.0
	_apply_valley_fog()
	var raining: float = 1.0 if weather == "Rain" else 0.0
	_rain.emitting = raining > 0.0
	for material: ShaderMaterial in [_road_material, _street_material, _track_material]:
		material.set_shader_parameter("wetness", raining)
		material.set_shader_parameter("puddles", raining)
		material.set_shader_parameter("puddle_color", sky_horizon)


## Applies a graphics preset (`QUALITY` key, as `TorqaApp.graphics_quality()` names it).
func apply_quality(name: String) -> void:
	_quality = QUALITY.get(name, QUALITY["medium"])
	var shadow_atlas: int = _quality["shadow_atlas"]
	RenderingServer.directional_shadow_atlas_set_size(shadow_atlas, true)
	_sun.directional_shadow_max_distance = _quality["shadow_distance"]
	var splits: int = _quality["shadow_splits"]
	_sun.directional_shadow_mode = (
		DirectionalLight3D.SHADOW_PARALLEL_4_SPLITS
		if splits == 4
		else DirectionalLight3D.SHADOW_PARALLEL_2_SPLITS
	)
	# A sun of real size casts soft shadows (PCSS): softer further from the caster.
	_sun.light_angular_distance = _quality["soft_shadows"]
	_environment.ssao_enabled = _quality["ssao"]
	_environment.glow_enabled = _quality["glow"]
	_rain.amount = _quality["rain"]
	var viewport: Viewport = get_viewport()
	viewport.msaa_3d = _quality["msaa"]
	viewport.screen_space_aa = (
		Viewport.SCREEN_SPACE_AA_FXAA if _quality["fxaa"] else Viewport.SCREEN_SPACE_AA_DISABLED
	)
	var render_scale: float = _quality["render_scale"]
	viewport.scaling_3d_mode = (
		Viewport.SCALING_3D_MODE_FSR if render_scale < 1.0 else Viewport.SCALING_3D_MODE_BILINEAR
	)
	viewport.scaling_3d_scale = render_scale
	# Lighter presets refresh the sky's light over several frames.
	_environment.sky.process_mode = (
		Sky.PROCESS_MODE_INCREMENTAL if name in ["low", "medium"] else Sky.PROCESS_MODE_AUTOMATIC
	)
	var distance: float = _quality["distance"]
	_set_distance(distance)


## Applies ride options (`RideOptions.options()`): camera, time of day and weather.
func apply_options(options: Dictionary) -> void:
	var time: String = options["time"]
	var weather: String = options["weather"]
	var camera_mode: int = options["camera"]
	apply_conditions(time, weather)
	set_camera(camera_mode)


## Snaps rider and camera to the start of a new ride instead of gliding there.
func reset_view() -> void:
	_placed = false


func _ready() -> void:
	# Flat palette colours everywhere (ADR 0011); the shaders facet what they draw.
	_terrain_material.shader = preload("res://shaders/terrain.gdshader")
	_terrain_material.set_shader_parameter("rock_color", Palette.color("ground.rock"))
	_terrain_material.set_shader_parameter("snow_color", Palette.color("ground.snow"))
	_road_material.shader = preload("res://shaders/road.gdshader")
	_road_material.set_shader_parameter("asphalt_color", Palette.color("road.asphalt"))
	_road_material.set_shader_parameter("marking_color", Palette.color("road.marking"))
	_road_material.set_shader_parameter("shoulder_color", Palette.color("road.shoulder"))
	_water_material.shader = preload("res://shaders/water.gdshader")
	_water_material.set_shader_parameter("deep_color", Palette.color("water.deep"))
	_water_material.set_shader_parameter("shallow_color", Palette.color("water.shallow"))
	_building_material.shader = preload("res://shaders/building.gdshader")
	_building_material.set_shader_parameter("glass", Palette.color("buildings.glass"))
	_building_material.set_shader_parameter("frame", Palette.color("buildings.frame"))
	_building_material.set_shader_parameter(
		"stained_glass", Palette.color("buildings.stained_glass")
	)
	_building_material.set_shader_parameter("stone", Palette.color("buildings.stone"))
	_structure_material.vertex_color_use_as_albedo = true
	_structure_material.vertex_color_is_srgb = true
	_structure_material.roughness = 0.9
	_sky.shader = preload("res://shaders/sky.gdshader")
	_sky.set_shader_parameter("ground_color", Palette.color("ground.meadow"))
	_sky.set_shader_parameter("cloud_color", Palette.color("sky.cloud"))
	_sky.set_shader_parameter("cloud_shade_color", Palette.color("sky.cloud_shade"))
	_environment.sky.sky_material = _sky
	# Tone mapping is linear (world.tscn) and the light balanced so a sunlit facet shows about its
	# palette colour; a warm white mixed into the sky's light keeps shadows bright, lightly tinted.
	_environment.ambient_light_color = Palette.color("light.ambient")
	# Haze towards distant terrain takes the sky's colour (R45).
	_environment.fog_aerial_perspective = 0.6
	_plant_material.shader = preload("res://shaders/plants.gdshader")
	_plant_material.set_shader_parameter("flower_colors", Palette.colors("plants.flowers"))
	for surface: Array in [
		[_street_material, "road.asphalt", 0.0], [_track_material, "road.gravel", 1.0]
	]:
		var material: ShaderMaterial = surface[0]
		var color: String = surface[1]
		material.shader = preload("res://shaders/street.gdshader")
		material.set_shader_parameter("surface_color", Palette.color(color))
		material.set_shader_parameter("grass_color", Palette.color("ground.meadow"))
		material.set_shader_parameter("middle_grass", surface[2])
	_rail_material.shader = preload("res://shaders/rail.gdshader")
	_rail_material.set_shader_parameter("ballast_color", Palette.color("road.ballast"))
	_rail_material.set_shader_parameter("sleeper_color", Palette.color("road.sleeper"))
	_rail_material.set_shader_parameter("rail_color", Palette.color("road.rail"))
	add_child(_clouds)
	add_child(_railways)
	for land: MeshInstance3D in [_horizon_ground, _horizon_water]:
		# Far away: its shadows would not show.
		land.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
		add_child(land)
	_rider.add_child(_avatar)
	_ghost.accent = UiTheme.GHOST_COLOR
	_ghost.ghostly = true
	_ghost.hide()
	add_child(_ghost)
	_make_rain()
	apply_conditions("Midday", "Clear")


func _process(delta: float) -> void:
	if not _paused:
		_wind_time += delta
		RenderingServer.global_shader_parameter_set("wind_time", _wind_time)
	_build_some_chunks()
	if _rain.emitting:
		# Round the camera, falling straight whichever way it looks.
		_rain.global_position = _camera.global_position + Vector3.UP * RAIN_ABOVE
	if visible:
		_cloud_offset += CLOUD_DRIFT * delta
		_sky.set_shader_parameter("cloud_offset", _cloud_offset)
	if _free and visible:
		_fly(delta)
	if not visible or _torqa == null:
		return
	var state: Dictionary = _torqa.ride_state()
	if state.is_empty():
		return
	_follow_ride(state, delta)


## The rider's own avatar (R46), on a new world and whenever the active rider changes; the
## ghost rides the same one.
func _apply_rider() -> void:
	var avatar: String = _torqa.profile().get("avatar", RiderAvatar.RIDERS[0])
	_avatar.rider = avatar
	_ghost.rider = avatar


func _on_world_ready(_info: Dictionary) -> void:
	_clouds_settled = false
	_find_low_ground()
	_apply_rider()
	for chunk: Node in _terrain.get_children():
		chunk.queue_free()
	_chunk_count = _torqa.world_chunk_count()
	_next_chunk = 0
	_road.mesh = _mesh_from(_torqa.road_mesh())
	_road.material_override = _road_material
	var land: Dictionary = _torqa.horizon_meshes()
	var ground: Dictionary = land["ground"]
	var lakes: Dictionary = land["water"]
	_horizon_ground.mesh = _mesh_from(ground)
	_horizon_ground.material_override = _terrain_material
	_horizon_water.mesh = _mesh_from(lakes)
	_horizon_water.material_override = _water_material
	_railways.mesh = _mesh_from(_torqa.railways_mesh())
	_railways.material_override = _rail_material
	_structures.mesh = _mesh_from(_torqa.structures_mesh())
	_structures.material_override = _structure_material


func _build_some_chunks() -> void:
	var built: int = 0
	while _next_chunk < _chunk_count and built < CHUNKS_PER_FRAME:
		var chunk: Dictionary = _torqa.world_chunk(_next_chunk)
		# Another course was opened meanwhile: its world replaces this one when ready.
		if chunk.is_empty():
			_chunk_count = 0
			return
		var center: Vector3 = chunk["center"]
		var node: Node3D = Node3D.new()
		node.position = center
		_terrain.add_child(node)
		var terrain_arrays: Dictionary = chunk["terrain"]
		var building_arrays: Dictionary = chunk["buildings"]
		var ground: MeshInstance3D = _mesh_instance(terrain_arrays, _terrain_material)
		ground.visibility_range_end = VISIBILITY_RANGE * _distance
		node.add_child(ground)
		for surface: Array in [
			["streets", _street_material], ["tracks", _track_material], ["water", _water_material]
		]:
			var arrays: Dictionary = chunk.get(surface[0], {})
			var vertices: PackedVector3Array = arrays.get("vertices", PackedVector3Array())
			if not vertices.is_empty():
				var material: ShaderMaterial = surface[1]
				var way: MeshInstance3D = _mesh_instance(arrays, material)
				way.visibility_range_end = VISIBILITY_RANGE * _distance
				node.add_child(way)
		var buildings: MeshInstance3D = _mesh_instance(building_arrays, _building_material)
		buildings.visibility_range_end = DETAIL_RANGE * _distance
		node.add_child(buildings)
		var modelled: Array = chunk.get("modelled", [])
		for cell: Dictionary in modelled:
			_add_modelled(node, cell)
		var plants: Dictionary = chunk.get("plants", {})
		for model: String in plants:
			var buffer: PackedFloat32Array = plants[model]
			node.add_child(_vegetation(model, buffer))
		var grass_range: float = _quality["grass_range"]
		if grass_range > 0.0:
			for kind: String in ["grass", "flowers"]:
				var tufts: PackedFloat32Array = chunk.get(kind, PackedFloat32Array())
				if not tufts.is_empty():
					node.add_child(_plants(tufts, kind == "flowers", grass_range))
		_next_chunk += 1
		built += 1


## Buildings drawn as Blender-made models up close and as shells beyond `model_range`, the two
## crossfading.
func _add_modelled(node: Node3D, cell: Dictionary) -> void:
	var model_range: float = _quality["model_range"]
	var models: Dictionary = cell["models"]
	for model: String in models:
		var multimesh: MultiMesh = MultiMesh.new()
		multimesh.transform_format = MultiMesh.TRANSFORM_3D
		multimesh.use_colors = true
		multimesh.use_custom_data = true
		multimesh.mesh = BuildingModels.mesh(model)
		var buffer: PackedFloat32Array = models[model]
		multimesh.instance_count = buffer.size() / 20
		multimesh.buffer = buffer
		var instance: MultiMeshInstance3D = MultiMeshInstance3D.new()
		instance.multimesh = multimesh
		instance.visibility_range_end = model_range
		instance.visibility_range_end_margin = MODEL_FADE
		instance.visibility_range_fade_mode = GeometryInstance3D.VISIBILITY_RANGE_FADE_SELF
		node.add_child(instance)
	var shell_arrays: Dictionary = cell["shells"]
	var shells: MeshInstance3D = _mesh_instance(shell_arrays, _building_material)
	shells.visibility_range_begin = model_range
	shells.visibility_range_begin_margin = MODEL_FADE
	shells.visibility_range_end = DETAIL_RANGE * _distance
	node.add_child(shells)


func _mesh_instance(arrays: Dictionary, material: Material) -> MeshInstance3D:
	var instance: MeshInstance3D = MeshInstance3D.new()
	instance.mesh = _mesh_from(arrays)
	instance.material_override = material
	instance.visibility_range_end_margin = 300.0
	instance.visibility_range_fade_mode = GeometryInstance3D.VISIBILITY_RANGE_FADE_SELF
	return instance


## Trees, bushes or rocks of one model in a chunk (`world_chunk()["plants"]`: transform and
## colour per plant). Bushes and rocks are small: they are drawn less far than trees.
func _vegetation(model: String, buffer: PackedFloat32Array) -> MultiMeshInstance3D:
	var multimesh: MultiMesh = MultiMesh.new()
	multimesh.transform_format = MultiMesh.TRANSFORM_3D
	multimesh.use_colors = true
	multimesh.mesh = VegetationModels.mesh(model)
	multimesh.instance_count = buffer.size() / 16
	multimesh.buffer = buffer
	var instance: MultiMeshInstance3D = MultiMeshInstance3D.new()
	instance.multimesh = multimesh
	var small: bool = model.begins_with("bush") or model.begins_with("rock")
	instance.visibility_range_end = (SMALL_PLANT_RANGE if small else DETAIL_RANGE) * _distance
	instance.visibility_range_end_margin = 100.0 if small else 300.0
	instance.visibility_range_fade_mode = GeometryInstance3D.VISIBILITY_RANGE_FADE_SELF
	return instance


## Instances of `mesh` from a transform buffer (12 floats each): grass tufts or flower clumps.
func _scatter(buffer: PackedFloat32Array, mesh: ArrayMesh) -> MultiMeshInstance3D:
	var multimesh: MultiMesh = MultiMesh.new()
	multimesh.transform_format = MultiMesh.TRANSFORM_3D
	multimesh.mesh = mesh
	multimesh.instance_count = buffer.size() / 12
	multimesh.buffer = buffer
	var instance: MultiMeshInstance3D = MultiMeshInstance3D.new()
	instance.multimesh = multimesh
	instance.visibility_range_end = DETAIL_RANGE * _distance
	instance.visibility_range_end_margin = 300.0
	instance.visibility_range_fade_mode = GeometryInstance3D.VISIBILITY_RANGE_FADE_SELF
	return instance


## Grass or flowers of a chunk, drawn near the camera only.
func _plants(buffer: PackedFloat32Array, flowers: bool, range_m: float) -> MultiMeshInstance3D:
	var instance: MultiMeshInstance3D = _scatter(buffer, _flower_mesh if flowers else _grass_mesh)
	instance.material_override = _plant_material
	instance.visibility_range_end = range_m
	instance.visibility_range_end_margin = range_m * 0.25
	var shadows: bool = _quality["grass_shadows"]
	instance.cast_shadow = (
		GeometryInstance3D.SHADOW_CASTING_SETTING_ON
		if shadows
		else GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	)
	return instance


## A grass tuft (blades leaning out from the root) or a flower clump (stems with heads),
## about half a metre high. UV.y runs from root to tip for the wind; normals point up so the
## plants light like the ground; flower heads have vertex alpha 0 for the shader to colour.
static func _plant_mesh(flower: bool) -> ArrayMesh:
	var tool: SurfaceTool = SurfaceTool.new()
	tool.begin(Mesh.PRIMITIVE_TRIANGLES)
	tool.set_normal(Vector3.UP)
	var blades: int = 3 if flower else 12
	for i: int in range(blades):
		var angle: float = TAU * float(i) / float(blades) + fmod(float(i) * 2.39, 1.0)
		var out: Vector3 = Vector3(cos(angle), 0.0, sin(angle))
		var side: Vector3 = Vector3(-out.z, 0.0, out.x)
		var height: float = (0.55 if flower else 0.32) + fmod(float(i) * 0.137, 0.2)
		var width: float = 0.012 if flower else 0.035
		var root: Vector3 = out * (0.04 if flower else 0.06 + fmod(float(i) * 0.31, 0.08))
		var tip: Vector3 = root + out * height * 0.35 + Vector3.UP * height
		# The meadow's tones, so tufts blend into the ground they grow from.
		var base_color: Color = Palette.color("plants.grass_base")
		var tip_color: Color = Palette.color("plants.grass_tip")
		_add_blade(tool, root, tip, side * width, base_color, tip_color)
		if flower:
			# A head: two crossed petals' quads at the tip, marked by alpha 0.
			var head: Color = Color(1, 1, 1, 0)
			for axis: Vector3 in [side, out]:
				_add_quad(tool, tip - axis * 0.045, tip + axis * 0.045, Vector3.UP * 0.05, head)
	return tool.commit()


static func _add_blade(
	tool: SurfaceTool, root: Vector3, tip: Vector3, half: Vector3, base: Color, top: Color
) -> void:
	for vertex: Array in [
		[root - half, base, 0.0],
		[root + half, base, 0.0],
		[tip, top, 1.0],
	]:
		var color: Color = vertex[1]
		var along: float = vertex[2]
		tool.set_color(color)
		tool.set_uv(Vector2(0.5, along))
		var position: Vector3 = vertex[0]
		tool.add_vertex(position)


static func _add_quad(tool: SurfaceTool, a: Vector3, b: Vector3, up: Vector3, color: Color) -> void:
	for position: Vector3 in [a, b, b + up, a, b + up, a + up]:
		tool.set_color(color)
		tool.set_uv(Vector2(0.5, 1.0))
		tool.add_vertex(position)


func _follow_ride(state: Dictionary, delta: float) -> void:
	var east: float = state["x"]
	var north: float = state["y"]
	var elevation: float = state["elevation_m"]
	var heading: float = state["heading"]
	var grade: float = state["grade"]
	var speed_kmh: float = state["speed_kmh"]
	var cadence: Variant = state["cadence"]
	var cadence_rpm: float = cadence if cadence != null else 0.0
	# Paused, the rider waits too: the state keeps its last speed, and the trainer's cadence
	# is live, but the bike goes nowhere (#178).
	_paused = state["paused"]
	if _paused:
		speed_kmh = 0.0
		cadence_rpm = 0.0
	_avatar.animate(delta, cadence_rpm, speed_kmh)
	var curvature: float = state["curvature"]
	_lean = _leaning(_lean, TorqaApp.lean_angle(speed_kmh, curvature), delta)
	# Bike and rider lean about where the tyres touch the road; the camera stays level.
	_avatar.transform = Transform3D(Basis(Vector3.FORWARD, _lean), Vector3.ZERO)

	# Godot looks along −z (north); headings run clockwise from north, rotations anticlockwise.
	# The heading is the road's own direction, so the rider turns with the bend.
	var yaw: Basis = Basis(Vector3.UP, -heading)
	var pitch: Basis = Basis(Vector3.RIGHT, atan(grade / 100.0))
	var position: Vector3 = Vector3(east, elevation, -north)
	if absf(elevation - _rider_elevation) > 0.5:
		_rider_elevation = elevation
		_apply_valley_fog()
	if not _clouds_settled:
		_clouds.settle(elevation)
		_clouds_settled = true
	# A jump (simulated rides): no gliding across the whole way.
	if _rider.position.distance_to(position) > JUMP_M:
		_placed = false
	_rider.transform = Transform3D(yaw * pitch, position)
	_place_ghost(state["ghost"], delta)

	if _free:
		return
	var target: Transform3D = _camera_target(_rider.transform)
	# First person is fixed to the head; smoothing its position would trail behind the rider.
	if _placed and _camera_mode != CameraMode.FIRST_PERSON:
		var camera_weight: float = 1.0 - exp(-delta * CAMERA_SMOOTHING)
		_camera.transform = _camera.transform.interpolate_with(target, camera_weight)
	else:
		_camera.transform = target
		_placed = true


## Free camera: arrow keys move, R/F rise and sink, the mouse wheel sets the speed; Shift +
## arrows look around, as does moving the mouse or trackpad with Shift held (or dragging with the
## right button) (#66, #78). The interface lets the mouse through to here (main.tscn).
func _unhandled_input(event: InputEvent) -> void:
	if not _free or not visible:
		return
	var motion: InputEventMouseMotion = event as InputEventMouseMotion
	if motion != null and (motion.shift_pressed or motion.button_mask & MOUSE_BUTTON_MASK_RIGHT):
		var turned: Vector3 = _camera.rotation
		turned.y -= motion.relative.x * FREE_LOOK
		turned.x = clampf(turned.x - motion.relative.y * FREE_LOOK, -1.5, 1.5)
		turned.z = 0.0
		_camera.rotation = turned
		get_viewport().set_input_as_handled()
	var wheel: InputEventMouseButton = event as InputEventMouseButton
	if wheel != null and wheel.pressed:
		if wheel.button_index == MOUSE_BUTTON_WHEEL_UP:
			_free_speed = minf(_free_speed * 1.25, 20.0)
		elif wheel.button_index == MOUSE_BUTTON_WHEEL_DOWN:
			_free_speed = maxf(_free_speed / 1.25, 0.1)


func _fly(delta: float) -> void:
	var looking: bool = Input.is_key_pressed(KEY_SHIFT)
	if looking:
		# Look around: left and right turn, up and down tilt.
		var turned: Vector3 = _camera.rotation
		turned.y += (_held(KEY_LEFT) - _held(KEY_RIGHT)) * FREE_TURN * delta
		turned.x = clampf(
			turned.x + (_held(KEY_UP) - _held(KEY_DOWN)) * FREE_TURN * delta, -1.5, 1.5
		)
		turned.z = 0.0
		_camera.rotation = turned
	var move: Vector3 = Vector3(0.0, _held(KEY_R) - _held(KEY_F), 0.0)
	if not looking:
		move.x = _held(KEY_RIGHT) - _held(KEY_LEFT)
		move.z = _held(KEY_DOWN) - _held(KEY_UP)
	if move == Vector3.ZERO:
		return
	_camera.position += _camera.basis * move.normalized() * FREE_SPEED * _free_speed * delta


## 1 while `key` is held, else 0.
static func _held(key: Key) -> float:
	return 1.0 if Input.is_physical_key_pressed(key) else 0.0


## Faceted raindrops (#103): slim four-sided diamonds in the pale sky colour; the emitter
## stands free of the camera's turn so the rain falls straight down.
func _make_rain() -> void:
	var drop: SphereMesh = SphereMesh.new()
	drop.radius = 0.022
	drop.height = 0.36
	drop.radial_segments = 4
	drop.rings = 2
	var material: ShaderMaterial = ShaderMaterial.new()
	material.shader = preload("res://shaders/rain.gdshader")
	material.set_shader_parameter("color", Palette.color("sky.rain"))
	drop.material = material
	_rain.draw_pass_1 = drop
	_rain.top_level = true


## Fog lying in the low ground of the route (#103): thick on mornings, a trace at midday, and
## thicker in haze and rain; always below the rider, who sees it lie in the valleys below.
func _apply_valley_fog() -> void:
	var thickness: float = (
		VALLEY_FOG_BY_TIME.get(_time_of_day, 0.0) * VALLEY_FOG_BY_WEATHER.get(_weather, 0.0)
	)
	_environment.fog_height = minf(
		_low_ground + VALLEY_FOG_DEPTH, _rider_elevation - VALLEY_FOG_CLEARANCE
	)
	_environment.fog_height_density = VALLEY_FOG_DENSITY * thickness


## The route's lowest ground, from its elevation profile.
func _find_low_ground() -> void:
	var profile: PackedVector2Array = _torqa.elevation_profile(256)
	if profile.is_empty():
		return
	_low_ground = INF
	for point: Vector2 in profile:
		_low_ground = minf(_low_ground, point.y)
	_apply_valley_fog()


## Scales how far terrain and details are drawn, for the chunks built already too.
func _set_distance(factor: float) -> void:
	var change: float = factor / _distance
	_distance = factor
	for node: Node in _terrain.find_children("*", "GeometryInstance3D", true, false):
		var geometry: GeometryInstance3D = node as GeometryInstance3D
		if geometry.visibility_range_end > 0.0:
			geometry.visibility_range_end *= change
	_camera.far = maxf(VISIBILITY_RANGE * factor * 1.6, HORIZON_RANGE)


## Puts the ghost rider (`ride_state()["ghost"]`) on the road, a little to the left so it never
## merges with the rider when both are side by side.
func _place_ghost(ghost: Variant, delta: float) -> void:
	if ghost == null:
		_ghost.hide()
		return
	var info: Dictionary = ghost
	var east: float = info["x"]
	var north: float = info["y"]
	var elevation: float = info["elevation_m"]
	var heading: float = info["heading"]
	var grade: float = info["grade"]
	var distance_m: float = info["distance_m"]
	var speed_kmh: float = (
		maxf(distance_m - _ghost_distance, 0.0) / maxf(delta, 0.001) * 3.6
		if _ghost.visible
		else 0.0
	)
	_ghost_distance = distance_m
	var curvature: float = info["curvature"]
	_ghost_lean = _leaning(_ghost_lean, TorqaApp.lean_angle(speed_kmh, curvature), delta)
	var yaw: Basis = Basis(Vector3.UP, -heading)
	var pitch: Basis = Basis(Vector3.RIGHT, atan(grade / 100.0))
	var roll: Basis = Basis(Vector3.FORWARD, _ghost_lean)
	var left: Vector3 = yaw * Vector3.LEFT
	_ghost.transform = Transform3D(
		yaw * pitch * roll, Vector3(east, elevation, -north) + left * 1.1
	)
	_ghost.animate(delta, 85.0 if speed_kmh > 1.0 else 0.0, speed_kmh)
	_ghost.show()


## Eases a lean towards `target` over a moment, at once after a jump.
func _leaning(lean: float, target: float, delta: float) -> float:
	if not _placed:
		return target
	return lerpf(lean, target, 1.0 - exp(-delta * LEAN_SMOOTHING))


func _camera_target(rider: Transform3D) -> Transform3D:
	var forward: Vector3 = -rider.basis.z
	var flat_forward: Vector3 = Vector3(forward.x, 0.0, forward.z).normalized()
	var origin: Vector3 = rider.origin
	var eye: Vector3
	var look_at: Vector3
	match _camera_mode:
		CameraMode.FIRST_PERSON:
			# The rider's eyes, slightly ahead of the helmet, looking down the road.
			eye = rider * Vector3(0, 1.56, -0.5)
			look_at = eye + forward * 20.0 - Vector3.UP * 1.5
		CameraMode.DRONE:
			eye = origin - flat_forward * 28.0 + Vector3.UP * 20.0
			look_at = origin + flat_forward * 10.0
		_:
			eye = origin - flat_forward * 6.5 + Vector3.UP * 2.6
			look_at = origin + flat_forward * 10.0 + Vector3.UP * 1.0
	return Transform3D(Basis.IDENTITY, eye).looking_at(look_at, Vector3.UP)


func _mesh_from(arrays: Dictionary) -> ArrayMesh:
	var surface: Array = []
	surface.resize(Mesh.ARRAY_MAX)
	var vertices: PackedVector3Array = arrays["vertices"]
	var normals: PackedVector3Array = arrays["normals"]
	var uvs: PackedVector2Array = arrays["uvs"]
	var colors: PackedColorArray = arrays["colors"]
	var indices: PackedInt32Array = arrays["indices"]
	surface[Mesh.ARRAY_VERTEX] = vertices
	surface[Mesh.ARRAY_NORMAL] = normals
	surface[Mesh.ARRAY_TEX_UV] = uvs
	if not colors.is_empty():
		surface[Mesh.ARRAY_COLOR] = colors
	surface[Mesh.ARRAY_INDEX] = indices
	var mesh: ArrayMesh = ArrayMesh.new()
	if not vertices.is_empty():
		mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES, surface)
	return mesh
