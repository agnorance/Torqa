extends SceneTree
## Headless checks of UI behaviour that needs no ride: the HUD editor (R51).
## Run: godot --headless --path app -s res://tests/ui_smoke.gd

const MAIN_SCENE: String = "res://scenes/main.tscn"

var _failed: bool = false


func _initialize() -> void:
	_run.call_deferred()


func _run() -> void:
	_hud_editor()
	_ride_settings()
	_workout_settings()
	_workout_editor()
	_rider_drivetrain()
	_shifter_buttons()
	await _course_cards()
	_video_view()
	_video_alignment()
	_translations()
	await _free_camera()
	await _overlay()
	_overlay_size()
	_clouds()
	if not _failed:
		print("UI SMOKE TEST PASSED")
	quit(1 if _failed else 0)


func _hud_editor() -> void:
	var editor: HudEditor = HudEditor.new()
	root.add_child(editor)
	var changes: Array[PackedStringArray] = []
	editor.layout_changed.connect(func(layout: PackedStringArray) -> void: changes.append(layout))
	editor.edit(PackedStringArray(["power", "speed", "cadence"]), false)

	editor.place("cadence", 0)
	_expect(editor.layout(), ["cadence", "power", "speed"], "move to the large figure")
	editor.place("heart_rate", 1)
	_expect(editor.layout(), ["cadence", "heart_rate", "power", "speed"], "add from available")
	editor.remove("power")
	_expect(editor.layout(), ["cadence", "heart_rate", "speed"], "remove")

	# Drag "cadence" out of the HUD onto an available figure: removed.
	_available_chips(editor)[0].call("_drop_data", Vector2.ZERO, {HudEditor.DRAG_KEY: "cadence"})
	_expect(editor.layout(), ["heart_rate", "speed"], "drop on the available list")
	editor.place("speed", 0)

	# Directly in the HUD (R54): drop "power" on the lower half of the large figure: right after it.
	var preview: HudPanel = editor.find_children("*", "HudPanel", true, false)[0]
	var large: Control = preview.get_child(0)
	large.size = Vector2(200, 80)
	var accepts: bool = large.call("_can_drop_data", Vector2(10, 70), {HudPanel.DRAG_KEY: "power"})
	_check(accepts, "the HUD accepts figures")
	large.call("_drop_data", Vector2(10, 70), {HudPanel.DRAG_KEY: "power"})
	_expect(editor.layout(), ["speed", "power", "heart_rate"], "drop into the HUD")
	# Grid cells split left/right: dropping "speed" on the right half of "heart_rate" (the last
	# figure) moves it to the end.
	preview = editor.find_children("*", "HudPanel", true, false)[0]
	var grid: Node = preview.get_child(preview.get_child_count() - 1)
	var last: Control = grid.get_child(grid.get_child_count() - 1)
	last.size = Vector2(100, 40)
	last.call("_drop_data", Vector2(90, 10), {HudPanel.DRAG_KEY: "speed"})
	_expect(editor.layout(), ["power", "heart_rate", "speed"], "move within the HUD")
	# Free space in the HUD appends.
	preview = editor.find_children("*", "HudPanel", true, false)[0]
	preview.call("_drop_data", Vector2.ZERO, {HudPanel.DRAG_KEY: "cadence"})
	_expect(editor.layout(), ["power", "heart_rate", "speed", "cadence"], "drop on free space")
	editor.remove("power")
	editor.remove("cadence")

	for id: String in ["speed", "heart_rate"]:
		editor.remove(id)
	_expect(editor.layout(), ["heart_rate"], "the last figure stays")
	for i: int in range(20):
		editor.place(str(TorqaApp.hud_metrics()[i]["id"]), 99)
	_check(editor.layout().size() == TorqaApp.hud_max_metrics(), "at most the maximum figures")
	_check(not changes.is_empty(), "changes are reported")
	editor.free()


## The ride options and the in-ride settings dialog (R48, R49).
func _ride_settings() -> void:
	var options: RideOptions = RideOptions.new()
	root.add_child(options)
	var wanted: Dictionary = {
		"camera": 2,
		"difficulty": 75.0,
		"flat_descents": true,
		"time": "Evening",
		"weather": "Rain",
		"video_sound": false,
	}
	options.set_options(wanted)
	_check(options.options() == wanted, "options round trip: %s" % options.options())
	# Video courses: only the trainer's options are shown; the others keep their values.
	options.show_option_groups(false, true)
	var shown: PackedStringArray = PackedStringArray()
	for i: int in range(0, options.get_child_count(), options.columns):
		var caption: Label = options.get_child(i) as Label
		if caption.visible:
			shown.append(caption.text)
	_check(
		shown == PackedStringArray(["Trainer difficulty", "Descents", "Sound"]),
		"video course options: %s" % shown
	)
	_check(options.options() == wanted, "hidden options keep their values")
	options.show_option_groups(true, false)
	options.free()

	var dialog: RideSettingsDialog = RideSettingsDialog.new()
	root.add_child(dialog)
	var events: Array[String] = []
	dialog.finish_requested.connect(func() -> void: events.append("finish"))
	dialog.abort_requested.connect(func() -> void: events.append("abort"))
	dialog.options_changed.connect(func(_options: Dictionary) -> void: events.append("options"))
	dialog.edit(wanted, PackedStringArray(["power", "speed"]), false)
	dialog.custom_action.emit(&"finish")
	_check(events == ["finish"], "finish at once: %s" % [events])
	# Aborting asks first; only the confirmation aborts.
	dialog.custom_action.emit(&"abort")
	_check(events == ["finish"], "abort needs a confirmation: %s" % [events])
	var confirm: ConfirmationDialog = (
		dialog.find_children("*", "ConfirmationDialog", true, false)[0]
	)
	confirm.confirmed.emit()
	_check(events == ["finish", "abort"], "abort once confirmed: %s" % [events])
	dialog.free()


## Workouts are set the same way on the Workouts tab and during them (R56, R58).
func _workout_settings() -> void:
	var options: WorkoutOptions = WorkoutOptions.new()
	root.add_child(options)
	var zones: PackedVector2Array = PackedVector2Array(
		[
			Vector2(95, 114),
			Vector2(114, 133),
			Vector2(133, 152),
			Vector2(152, 171),
			Vector2(171, 190)
		]
	)
	options.configure(zones, 250.0)
	var fresh: Dictionary = options.workout()
	_check(
		(
			_number(fresh, "power_w") == 175.0
			and _number(fresh, "min_w") == 100.0
			and _number(fresh, "max_w") == 225.0
		),
		"starting values from the FTP: %s" % fresh
	)
	_check(_number(fresh, "bpm") == 124.0, "heart rate from zone 2: %s" % fresh)
	var wanted: Dictionary = {
		"kind": "zone",
		"power_w": 200.0,
		"zone": 3,
		"bpm": 140.0,
		"min_w": 120.0,
		"max_w": 220.0,
		"id": "",
		"test": "twenty_minutes",
	}
	options.set_workout(wanted)
	_check(options.workout() == wanted, "workout round trip: %s" % options.workout())
	_check(options.title() == "Heart-rate zone 3", "named for the history: %s" % options.title())
	_expect(
		_visible_captions(options),
		["Workout", "Zone", "Lowest power", "Highest power"],
		"zone rows"
	)
	options.set_workout({"kind": "power", "power_w": 210.0})
	_expect(_visible_captions(options), ["Workout", "Power"], "constant power rows")
	_check(options.title() == "Constant power 210 W", "power title: %s" % options.title())
	# Limits typed the wrong way round still make a range.
	options.set_workout({"kind": "bpm", "min_w": 230.0, "max_w": 110.0})
	var swapped: Dictionary = options.workout()
	_check(
		_number(swapped, "min_w") == 110.0 and _number(swapped, "max_w") == 230.0,
		"limits ordered: %s" % swapped
	)
	# Structured workouts (R21) come from the library; their name names the ride.
	var plans: Array = [
		{"id": "builtin:a", "name": "Sweet spot", "duration_s": 3600.0, "steps": []},
		{"id": "/w/b.zwo", "name": "Tempo", "duration_s": 1800.0, "steps": []},
	]
	options.set_plans(plans)
	options.set_workout({"kind": "plan", "id": "/w/b.zwo"})
	var chosen_id: String = options.workout()["id"]
	_check(chosen_id == "/w/b.zwo", "the chosen plan: %s" % options.workout())
	_check(options.title() == "Tempo", "named after the plan: %s" % options.title())
	_expect(_visible_captions(options), ["Workout", "Plan"], "plan rows")
	options.set_plans(plans, "builtin:a")
	var chosen_name: String = options.plan()["name"]
	_check(chosen_name == "Sweet spot", "an imported plan is chosen at once")
	options.set_workout({"kind": "bpm"})
	_check(options.plan().is_empty(), "no plan for other kinds")
	options.set_workout({"kind": "ftp_test"})
	_check(options.title() == "FTP test", "the FTP test (R22): %s" % options.title())
	var test: String = options.workout()["test"]
	_check(test == "ramp", "the ramp test unless chosen otherwise")
	_expect(_visible_captions(options), ["Workout", "Test"], "the FTP test: only which one")
	options.set_workout({"kind": "ftp_test", "test": "two_by_eight"})
	test = options.workout()["test"]
	_check(test == "two_by_eight", "another test (#125)")
	_check(options.title() == "FTP test · 2 × 8 minutes", "named after it: %s" % options.title())
	# The same rider keeps their values; another rider gets their own.
	options.configure(zones, 250.0)
	_check(_number(options.workout(), "max_w") == 230.0, "values kept for the same rider")
	options.configure(zones, 300.0)
	_check(_number(options.workout(), "max_w") == 270.0, "another rider's values")
	options.free()

	var dialog: RideSettingsDialog = RideSettingsDialog.new()
	root.add_child(dialog)
	dialog.configure_workout(zones, 250.0, [])
	var changes: Array[Dictionary] = []
	dialog.workout_changed.connect(func(workout: Dictionary) -> void: changes.append(workout))
	var tabs: TabContainer = dialog.find_children("*", "TabContainer", true, false)[0]
	var ride_options: Dictionary = {"difficulty": 50.0, "workout": wanted, "on_course": false}
	dialog.edit(ride_options, PackedStringArray(["power"]), false, false)
	_check(tabs.current_tab == 0 and not tabs.is_tab_hidden(0), "the workout shows first")
	_check(tabs.is_tab_hidden(1), "a workout on its own has no ride options")
	var in_dialog: WorkoutOptions = dialog.find_children("*", "WorkoutOptions", true, false)[0]
	_check(in_dialog.workout() == wanted, "the workout being ridden: %s" % in_dialog.workout())
	in_dialog.changed.emit()
	_check(
		changes.size() == 1 and _number(changes[0], "zone") == 3.0,
		"changes reach the ride: %s" % [changes]
	)
	ride_options["on_course"] = true
	dialog.edit(ride_options, PackedStringArray(["power"]), false)
	_check(not tabs.is_tab_hidden(1), "on a course, the world's options too")
	var ride: RideOptions = dialog.find_children("*", "RideOptions", true, false)[0]
	_check(not "Trainer difficulty" in _visible_captions(ride), "ERG leaves difficulty no part")
	dialog.edit({"difficulty": 50.0}, PackedStringArray(["power"]), false)
	_check(tabs.is_tab_hidden(0) and tabs.current_tab == 1, "a plain ride has no workout")
	_check("Trainer difficulty" in _visible_captions(ride), "a plain ride has difficulty")
	dialog.free()


## A number in a workout, typed for comparing.
static func _number(workout: Dictionary, key: String) -> float:
	var number: float = workout[key]
	return number


## The workout editor (R21): steps from the library, new ones, intervals, times as m:ss.
func _workout_editor() -> void:
	_check(WorkoutEditor.seconds_of("12:30") == 750.0, "m:ss")
	_check(WorkoutEditor.seconds_of("45") == 45.0, "plain seconds")
	_check(
		WorkoutEditor.seconds_of("1:75") < 0.0 and WorkoutEditor.seconds_of("x") < 0.0, "no time"
	)
	_check(WorkoutEditor.clock(3725.0) == "62:05", "minutes and seconds")
	var editor: WorkoutEditor = WorkoutEditor.new()
	root.add_child(editor)
	var library_plan: Dictionary = {
		"id": "/w/tempo.zwo",
		"name": "Tempo",
		"description": "Steady",
		"builtin": false,
		"steps":
		[
			{"duration_s": 600.0, "from_pct": 50.0, "to_pct": 75.0, "cadence": null, "message": ""},
			{
				"duration_s": 120.0,
				"from_pct": null,
				"to_pct": null,
				"cadence": 95.0,
				"message": "Go"
			},
		],
	}
	editor.edit(library_plan, 250.0)
	var edited: Dictionary = editor.workout()
	var steps: Array = edited["steps"]
	var ramp: Dictionary = steps[0]
	var free: Dictionary = steps[1]
	var expected_ramp: Dictionary = {
		"duration_s": 600.0,
		"free": false,
		"from_pct": 50.0,
		"to_pct": 75.0,
		"cadence": 0.0,
		"message": "",
	}
	_check(ramp == expected_ramp, "a ramp: %s" % ramp)
	var expected_free: Dictionary = {
		"duration_s": 120.0,
		"free": true,
		"from_pct": 0.0,
		"to_pct": 0.0,
		"cadence": 95.0,
		"message": "Go",
	}
	_check(free == expected_free, "free: %s" % free)
	editor.add_intervals(3, 60.0, 120.0, 30.0, 50.0)
	steps = editor.workout()["steps"]
	_check(steps.size() == 8, "three intervals add six steps: %d" % steps.size())
	var last: Dictionary = steps[7]
	var expected_rest: Dictionary = {
		"duration_s": 30.0,
		"free": false,
		"from_pct": 50.0,
		"to_pct": 50.0,
		"cadence": 0.0,
		"message": "",
	}
	_check(last == expected_rest, "the last rest: %s" % last)
	editor.edit({}, 250.0)
	var fresh: Array = editor.workout()["steps"]
	_check(fresh.size() == 3, "a new workout starts with a warm-up, a block and a cool-down")
	editor.free()


## The rider's drivetrain (R9): chainring and cog only for a single cog.
func _rider_drivetrain() -> void:
	var dialog: ProfileDialog = ProfileDialog.new()
	root.add_child(dialog)
	var confirmed: Array[Dictionary] = []
	dialog.profile_confirmed.connect(
		func(_id: String, profile: Dictionary, _hud: PackedStringArray) -> void:
			confirmed.append(profile)
	)
	dialog.edit(
		{"id": "r", "name": "R", "drivetrain": "single_cog", "chainring": 46, "cog": 14},
		PackedStringArray(["power"])
	)
	# The cog is the last of the teeth fields; the zones tab has spin boxes of its own.
	var teeth: Array[Node] = dialog.find_children("*", "SpinBox", true, false).filter(
		func(node: Node) -> bool: return (node as SpinBox).suffix == " T"
	)
	var cog: SpinBox = teeth[teeth.size() - 1]
	_check(cog.visible, "chainring and cog for a single cog")
	dialog.confirmed.emit()
	var saved: Dictionary = confirmed[0]
	var drivetrain: String = saved["drivetrain"]
	var chainring: float = saved["chainring"]
	_check(drivetrain == "single_cog" and chainring == 46.0, "saved: %s" % saved)
	dialog.edit({"id": "r", "name": "R"}, PackedStringArray(["power"]))
	_check(not cog.visible, "a cassette needs no teeth")
	dialog.free()


## Every action the core offers for a Di2's buttons can be chosen by its label (#139).
func _shifter_buttons() -> void:
	var actions: PackedStringArray = TorqaApp.button_actions()
	for action: String in actions:
		_check(DevicesTab.ACTION_LABELS.has(action), "a label for %s" % action)
	_check(actions.size() == DevicesTab.ACTION_LABELS.size(), "no label for a gone action")
	var tab: DevicesTab = DevicesTab.new()
	root.add_child(tab)
	var choices: Array[Node] = tab.find_children("*", "OptionButton", true, false)
	# Trainer, heart rate and shifter, a press, a hold and a double press of four channels, and
	# the graphics quality.
	_check(
		choices.size() == 3 + 4 * 3 + 1, "a choice per press of each channel: %d" % choices.size()
	)
	var press: OptionButton = choices[3]
	_check(press.item_count == 1 + actions.size(), "nothing or any action")
	tab.free()


## The captions of a three-column options grid whose rows show.
static func _visible_captions(grid: GridContainer) -> PackedStringArray:
	var shown: PackedStringArray = PackedStringArray()
	for i: int in range(0, grid.get_child_count(), grid.columns):
		var caption: Label = grid.get_child(i) as Label
		if caption.visible:
			shown.append(caption.text)
	return shown


## The video view blends from frame to frame by video time (R17).
func _video_view() -> void:
	var view: VideoView = VideoView.new()
	root.add_child(view)
	var dark: Image = Image.create_empty(8, 6, false, Image.FORMAT_RGBA8)
	var light: Image = Image.create_empty(8, 6, false, Image.FORMAT_RGBA8)
	light.fill(Color.WHITE)
	view.take(dark, 1.0)
	_check(view.blend_at(1.0) == 1.0, "the first frame shows as it is")
	view.take(light, 1.1)
	_check(is_equal_approx(view.blend_at(1.05), 0.5), "halfway between frames, half of each")
	_check(view.blend_at(2.0) == 1.0, "past the newest frame, that frame")
	var first: Texture2D = view.texture
	view.take(dark, 1.2)
	_check(view.texture != first, "the newest frame gets its own texture")
	view.free()


## Placing a video without GPS on a route (R17): start and end, the end after the start.
func _video_alignment() -> void:
	var dialog: VideoAlignDialog = VideoAlignDialog.new()
	root.add_child(dialog)
	var results: Array[PackedVector2Array] = []
	dialog.aligned.connect(func(marks: PackedVector2Array) -> void: results.append(marks))
	var profile: PackedVector2Array = [Vector2(0, 500), Vector2(2000, 600)]
	dialog.edit(null, "", 60.0, 2000.0, profile, PackedVector2Array())
	_check(
		dialog.marks() == PackedVector2Array([Vector2(0, 0), Vector2(2000, 60)]),
		"start and end span the whole video and route: %s" % [dialog.marks()]
	)
	_check(not dialog.get_ok_button().disabled, "the whole video is a valid alignment")
	var sliders: Array[Node] = dialog.find_children("*", "HSlider", true, false)
	var time_slider: HSlider = sliders[0]
	var distance_slider: HSlider = sliders[1]
	_check(not distance_slider.editable, "the start stays at the route's start")
	dialog.add_point()
	_check(
		dialog.marks()[1] == Vector2(1000, 30), "a point halfway in between: %s" % [dialog.marks()]
	)
	_check(distance_slider.editable, "points in between move along the route")
	distance_slider.value = 500.0
	time_slider.value = 40.0
	_check(dialog.marks()[1] == Vector2(500, 40), "the point moved: %s" % [dialog.marks()])
	dialog.select(2)
	time_slider.value = 35.0
	_check(dialog.get_ok_button().disabled, "the end before a point cannot be confirmed")
	time_slider.value = 55.0
	_check(not dialog.get_ok_button().disabled, "valid again once in order")
	dialog.confirmed.emit()
	_check(
		results == [PackedVector2Array([Vector2(0, 0), Vector2(500, 40), Vector2(2000, 55)])],
		"aligned with the chosen points: %s" % [results]
	)
	dialog.select(1)
	dialog.remove_point()
	_check(dialog.marks().size() == 2, "a point removed: %s" % [dialog.marks()])
	dialog.select(0)
	dialog.remove_point()
	_check(dialog.marks().size() == 2, "the start stays")
	dialog.free()
	# Importing a video without GPS explains how to add it to its course instead.
	var courses: CoursesTab = CoursesTab.new()
	var steps: String = courses.no_gps_steps("ride.mp4")
	_check(
		steps.begins_with("ride.mp4 has no GPS") and steps.contains("Add video…"),
		"steps for a video without GPS: %s" % steps
	)
	courses.free()


## Course cards (R37, R39) from a course preview: figures one per row, no elevation strip.
func _course_cards() -> void:
	var course: Dictionary = {
		"path": "/tmp/x.tqc",
		"name": "Gurten",
		"length_m": 2400.0,
		"elevation_gain_m": 149.4,
		"max_grade": 17.6,
		"track": PackedVector2Array([Vector2(0, 0), Vector2(500, 800), Vector2(900, 1200)]),
		"profile": PackedVector2Array([Vector2(0, 560), Vector2(2400, 709)]),
	}
	var figures: Array[PackedStringArray] = CourseCard.figures(course, false)
	var expected: Array[PackedStringArray] = [
		PackedStringArray(["Length", "2.4 km"]),
		PackedStringArray(["Climbing", "149 m"]),
		PackedStringArray(["Steepest", "18 %"]),
	]
	_check(figures == expected, "card figures: %s" % [figures])
	var imperial: Array[PackedStringArray] = CourseCard.figures(course, true)
	_check(
		imperial[0][1] == "1.5 mi" and imperial[1][1] == "490 ft",
		"imperial card figures: %s" % [imperial]
	)
	var card: CourseCard = CourseCard.new(course, false)
	root.add_child(card)
	var opened: Array[bool] = [false]
	card.pressed.connect(func() -> void: opened[0] = true)
	var click: InputEventMouseButton = InputEventMouseButton.new()
	click.button_index = MOUSE_BUTTON_LEFT
	click.pressed = true
	card.call("_gui_input", click)
	_check(opened[0], "a click opens the course")
	await process_frame
	await process_frame
	_check(card.name_tooltip().is_empty(), "a short name needs no tooltip")
	var long_name: String = "Rennradfahrt - Alpenbrevet 2026 Gold, Andermatt und zurück"
	var long_course: Dictionary = course.duplicate()
	long_course["name"] = long_name
	var long_card: CourseCard = CourseCard.new(long_course, false)
	root.add_child(long_card)
	await process_frame
	await process_frame
	_check(
		long_card.name_tooltip() == long_name,
		"a cut-off name shows in full on hover: %s" % long_card.name_tooltip()
	)
	_check(not _has_badge(card), "a GPX course has no video badge")
	card.free()
	course["video"] = "Gurten.MP4"
	var video_card: CourseCard = CourseCard.new(course, false)
	_check(_has_badge(video_card), "a video course is marked as one")
	video_card.free()


func _has_badge(node: Node) -> bool:
	for child: Node in node.get_children():
		var label: Label = child as Label
		if (label != null and label.text == "Video") or _has_badge(child):
			return true
	return false


func _translations() -> void:
	var before: String = TranslationServer.get_locale()
	TranslationServer.set_locale("de")
	_check(TranslationServer.translate("Power") == "Leistung", "German texts load")
	_check(
		TranslationServer.translate("Climb done in %s") % "4:12" == "Anstieg geschafft in 4:12",
		"formatted texts translate"
	)
	TranslationServer.set_locale("en")
	_check(TranslationServer.translate("Power") == "Power", "English is the source language")
	TranslationServer.set_locale(before)


## The available figures of the editor (its second column).
func _available_chips(editor: HudEditor) -> Array[Node]:
	return editor.get_child(1).get_child(1).get_child(0).get_children()


func _expect(actual: PackedStringArray, expected: Array, what: String) -> void:
	_check(actual == PackedStringArray(expected), "%s: %s" % [what, actual])


func _check(condition: bool, what: String) -> void:
	if not condition:
		push_error("UI SMOKE TEST FAILED: " + what)
		_failed = true


## The free camera turns with the mouse while Shift is held, and not without (#66).
## The free camera (#66, #78) with input as it arrives in the running app: through the
## interface, which must let the mouse through to the world.
func _free_camera() -> void:
	var main: Control = (load(MAIN_SCENE) as PackedScene).instantiate()
	root.add_child(main)
	await process_frame
	(main.get_node("StartPage") as Control).hide()
	(main.get_node("RideScreen") as Control).show()
	var world: RideWorld = main.get_node("World")
	world.show()
	world.set("_free", true)
	var camera: Camera3D = world.get("_camera")
	var middle: Vector2 = Vector2(root.size) / 2.0

	var hovering: InputEventMouseMotion = InputEventMouseMotion.new()
	hovering.position = middle
	hovering.relative = Vector2(50.0, 20.0)
	var before: Vector3 = camera.rotation
	root.push_input(hovering)
	_check(camera.rotation.is_equal_approx(before), "plain mouse moves leave the view")

	var looking: InputEventMouseMotion = hovering.duplicate()
	looking.shift_pressed = true
	root.push_input(looking)
	_check(
		camera.rotation.y < before.y and camera.rotation.x < before.x,
		"Shift + mouse over the ride screen turns the view"
	)

	before = camera.rotation
	var place: Vector3 = camera.position
	await _hold([KEY_SHIFT, KEY_LEFT])
	_check(camera.rotation.y > before.y, "Shift + left turns the view left")
	_check(camera.position.is_equal_approx(place), "Shift + arrows only look")

	before = camera.rotation
	await _hold([KEY_LEFT])
	var moved: Vector3 = camera.position - place
	_check(moved.dot(camera.basis.x) < -0.01, "left alone moves left: %s" % moved)
	_check(camera.rotation.is_equal_approx(before), "arrows alone do not turn")
	main.free()


## The overlay (R55, R57): the window turns into the HUD alone, on top, and back as it was.
func _overlay() -> void:
	var main: Control = (load(MAIN_SCENE) as PackedScene).instantiate()
	root.add_child(main)
	await process_frame
	(main.get_node("StartPage") as Control).hide()
	var ride: RideScreen = main.get_node("RideScreen")
	ride.begin({"workout": {"kind": "zone", "zone": 3}, "on_course": false})
	ride.show()
	var metrics: Control = main.get_node("RideScreen/MetricsPanel")
	var window: Window = root
	var scale_before: Vector2i = window.content_scale_size

	ride.overlay_requested.emit(true)
	for i: int in range(3):
		await process_frame
	_check(ride.is_overlay() and not metrics.visible, "only the overlay shows")
	# Headless, the window has no flags of its own: they are checked on a real display.
	var flags: bool = window.borderless and window.always_on_top and window.transparent
	_check(
		window.disable_3d and (flags or DisplayServer.get_name() == "headless"),
		"a borderless window on top, see-through, no world drawn"
	)
	var base: Vector2 = ride.overlay_content_size()
	_check(
		base.x >= OverlayHud.HUD_WIDTH and window.content_scale_size == Vector2i(base.ceil()),
		"the content scales with the window: %s, %s" % [base, window.content_scale_size]
	)
	_check(window.mouse_passthrough_polygon.size() == 4, "clicks beside it go through")

	# Larger and smaller from the keyboard and the bar (#124).
	var entered: Vector2 = Vector2(window.size)
	_press(KEY_EQUAL)
	_check(window.size.x > entered.x and window.size.y > entered.y, "+ makes it larger")
	var smaller: Button = _button_with_text(ride, "A−")
	for i: int in range(2):
		smaller.pressed.emit()
	_check(window.size.x < entered.x and window.size.y < entered.y, "A− makes it smaller")
	var overlay: OverlayWindow = main.get("_overlay")
	var chosen: float = overlay.scale

	_press(KEY_O)
	await process_frame
	_check(not ride.is_overlay() and metrics.visible, "O brings the whole screen back")
	_check(
		not window.borderless and not window.always_on_top and not window.disable_3d,
		"the window as it was"
	)
	_check(window.content_scale_size == scale_before, "the scale as it was")
	# Leaving resizes the window; that must not bring the overlay's outline back (#123).
	_check(window.mouse_passthrough_polygon.is_empty(), "every click is the app's again")
	var torqa: TorqaApp = main.get_node("Torqa")
	_check(torqa.overlay_window().size != Vector2i.ZERO, "the overlay's place is remembered")
	_check(is_equal_approx(torqa.overlay_scale(), chosen), "and the size the rider chose")
	main.free()


## The overlay's size (#124): large enough to read from the saddle at first, then as the rider
## makes it, also with the grip.
func _overlay_size() -> void:
	var overlay: OverlayWindow = OverlayWindow.new(root)
	var content: Vector2 = Vector2(300.0, 400.0)
	var screen_scale: float = DisplayServer.screen_get_scale(root.current_screen)
	overlay.enter(content, Rect2i(), 0.0)
	_check(overlay.scale >= OverlayWindow.DEFAULT_SCALE, "larger than the ride screen's HUD")
	_check(
		Vector2(root.size).is_equal_approx(content * overlay.scale * screen_scale),
		"the window fits its content at that size: %s" % root.size
	)
	var first: float = overlay.scale
	overlay.zoom(1)
	_check(is_equal_approx(overlay.scale, first * OverlayWindow.SCALE_STEP), "one step larger")
	for i: int in range(30):
		overlay.zoom(-1)
	_check(is_equal_approx(overlay.scale, OverlayWindow.MIN_SCALE), "not smaller than legible")
	root.size = Vector2i(content * 2.0 * screen_scale)
	overlay.window_resized()
	_check(is_equal_approx(overlay.scale, 2.0), "the grip sets the size: %.2f" % overlay.scale)
	overlay.leave()

	overlay.enter(content, Rect2i(), 1.6)
	_check(is_equal_approx(overlay.scale, 1.6), "the size chosen before comes back")
	overlay.leave()


## The low-poly clouds (ADR 0011) follow the weather: more and bigger ones as it clouds over,
## always high over the land and round the camera.
func _clouds() -> void:
	var eye: Vector3 = Vector3(5000.0, 600.0, -3000.0)
	var layer: CloudLayer = CloudLayer.new()
	root.add_child(layer)
	layer.settle(500.0)
	var counts: Array[int] = []
	var sizes: Array[float] = []
	for cover: float in [0.0, 0.3, 1.0]:
		layer.cover = cover
		var count: int = 0
		var size: float = 0.0
		for transforms: Array in layer.placements(eye):
			for placed: Transform3D in transforms:
				_check(
					placed.origin.y > 500.0 + 400.0,
					"clouds high over the land: %.0f m at cover %.1f" % [placed.origin.y, cover]
				)
				var from_eye: Vector2 = Vector2(placed.origin.x - eye.x, placed.origin.z - eye.z)
				_check(
					absf(from_eye.x) <= CloudLayer.SPREAD and absf(from_eye.y) <= CloudLayer.SPREAD,
					"clouds round the camera: %s" % from_eye
				)
				size += placed.basis.get_scale().x
				count += 1
		counts.append(count)
		sizes.append(size / maxf(count, 1.0))
	_check(counts[0] == 0, "no clouds on a clear day: %d" % counts[0])
	_check(counts[1] < counts[2], "more clouds as it clouds over: %s" % [counts])
	_check(sizes[1] < sizes[2], "bigger clouds as it clouds over: %s" % [sizes])
	layer.free()


## Presses and releases `key` as the rider would.
func _press(key: Key) -> void:
	for pressed: bool in [true, false]:
		var event: InputEventKey = InputEventKey.new()
		event.keycode = key
		event.pressed = pressed
		root.push_input(event)


## The first button under `node` showing `text`.
func _button_with_text(node: Node, text: String) -> Button:
	for button: Node in node.find_children("*", "Button", true, false):
		if (button as Button).text == text:
			return button
	return null


## Holds `keys` for a few frames, then lets them go.
func _hold(keys: Array[Key]) -> void:
	for pressed: bool in [true, false]:
		for key: Key in keys:
			var event: InputEventKey = InputEventKey.new()
			event.keycode = key
			event.physical_keycode = key
			event.pressed = pressed
			event.shift_pressed = pressed and keys.has(KEY_SHIFT)
			Input.parse_input_event(event)
		for frame: int in range(5):
			await process_frame
