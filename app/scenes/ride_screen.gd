class_name RideScreen
extends Control
## The live ride HUD: a compact metrics column, the map with the elevation profile below it,
## status toasts and one settings button (R48) — the rest of the screen is the ride. A workout
## (R56) adds its targets; on its own (R58) it has no map or world, but a chart of power and
## heart rate over its own backdrop.

signal closed
## The ride was saved to `path`; show its summary (R42).
signal summary_requested(path: String)
## The rider wants the overlay (R55), or the whole screen back.
signal overlay_requested(on: bool)
## The rider wants the overlay `steps` sizes larger, or smaller if negative (#124).
signal overlay_zoom_requested(steps: int)

## Keys during the ride: C camera; M play/pause music, "." next and "," previous track.
# i18n-begin
const MUSIC_KEYS: Dictionary[Key, Array] = {
	KEY_M: ["play_pause", "Music: play / pause"],
	KEY_PERIOD: ["next", "Music: next track"],
	KEY_COMMA: ["previous", "Music: previous track"],
}
# i18n-end
const TOAST_SECONDS: float = 4.0
## The frame-time budget of every preset (R43): 60 fps, with some slack; held below it this
## long, a lower preset is suggested once per ride.
const FRAME_BUDGET_S: float = 1.0 / 60.0 * 1.15
const SLOW_FOR_S: float = 10.0
## Speeds of a simulated ride (#53).
const TIME_SCALES: Array[float] = [1.0, 2.0, 5.0, 10.0, 20.0]
const KM_PER_MILE: float = 1.609344
const METERS_PER_FOOT: float = 0.3048
## How often the workout chart is redrawn: samples come once a second.
const CHART_EVERY_S: float = 1.0
## The workout chart spans at least this long at first, so its start is not stretched.
const CHART_MIN_S: float = 600.0

var _torqa: TorqaApp
var _world: RideWorld
var _settings_dialog: RideSettingsDialog = RideSettingsDialog.new()
## The ride options in effect (`RideOptions.options()`).
var _options: Dictionary = {}
## Finished from the settings: go to the summary as soon as the ride is saved.
var _summary_when_saved: bool = false
var _saved_path: String = ""
var _imperial: bool = false
var _climb_panel: PanelContainer = PanelContainer.new()
var _climb_title: Label = UiTheme.caption("")
var _climb_left: Label = UiTheme.value(20)
var _climb_time: Label = Label.new()
var _ghost_panel: PanelContainer = PanelContainer.new()
var _ghost_name: Label = UiTheme.caption("")
var _ghost_gap: Label = UiTheme.value(20)
var _finished: bool = false
var _saved: bool = false
var _toast_left: float = 0.0
## Simulated rides (fake trainer): speed, jumps on map and profile, free camera (#53).
var _simulation: PanelContainer = PanelContainer.new()
var _speed_buttons: Array[Button] = []
## The workout being ridden (`WorkoutsTab.workout()`), empty on a plain ride.
var _workout: Dictionary = {}
var _workout_panel: WorkoutPanel = WorkoutPanel.new()
## A workout on its own: no world behind the screen, a chart instead of map and profile.
var _backdrop: ColorRect = ColorRect.new()
var _chart_panel: PanelContainer = PanelContainer.new()
var _chart: RideChart = RideChart.new()
var _chart_left: float = 0.0
## A structured workout's steps above the chart, the part done dimmed.
var _plan_chart: WorkoutChart = WorkoutChart.new()
## The overlay (R55): only the HUD and the workout, the rest of the screen hidden meanwhile.
var _overlay_hud: OverlayHud = OverlayHud.new()
var _overlay_button: Button = Button.new()
var _pause_button: Button = Button.new()
var _hidden_by_overlay: Array[Control] = []
var _time_scale: float = 1.0
## The virtual gear shown last (R9), to tell the rider of a shift; 0 before the first.
var _gear: int = 0
var _frame_time: float = 0.0
var _slow_for: float = 0.0
var _budget_noted: bool = false

@onready var _hud: HudPanel = %Metrics
@onready var _minimap: Minimap = %Minimap
@onready var _profile: ElevationProfile = %Profile
@onready var _profile_info: Label = %ProfileInfo
@onready var _toast: PanelContainer = %Toast
@onready var _toast_label: Label = %ToastLabel
@onready var _settings_button: Button = %SettingsButton
@onready var _finish_button: Button = %FinishButton


func bind(torqa: TorqaApp, world: RideWorld) -> void:
	_torqa = torqa
	_world = world
	_torqa.device_connected.connect(_on_device_connected)
	_torqa.device_disconnected.connect(_on_device_disconnected)
	# Deferred: the handler calls back into Torqa, which is still busy emitting the signal.
	_torqa.ride_finished.connect(_on_ride_finished, CONNECT_DEFERRED)
	_torqa.ride_saved.connect(_on_ride_saved)
	_torqa.climb_completed.connect(_on_climb_completed)
	_torqa.route_completed.connect(_on_route_completed)
	_torqa.failed.connect(_on_failed)
	_torqa.control_requested.connect(_on_control_requested)


## Prepares the screen for a new ride on the loaded route with `options` in effect.
func begin(options: Dictionary) -> void:
	_options = options
	_finished = false
	_saved = false
	_gear = 0
	_summary_when_saved = false
	_settings_button.show()
	_finish_button.hide()
	_pause_button.show()
	_show_paused(false)
	_workout = options.get("workout", {})
	var on_its_own: bool = not _workout.is_empty() and not options.get("on_course", false)
	_backdrop.visible = on_its_own
	_chart_panel.visible = on_its_own
	_chart.set_series(PackedVector2Array(), PackedVector2Array(), PackedVector2Array())
	_chart_left = 0.0
	# Heart-rate zones span half the maximum to the maximum: the chart shows all of them.
	var max_heart_rate: float = _torqa.profile().get("max_heart_rate_bpm", 185.0)
	_chart.heart_rate_range = Vector2(max_heart_rate * 0.5, max_heart_rate)
	(get_node("Attribution") as Control).visible = not on_its_own
	for panel: Control in [$RightColumn/MapPanel, $RightColumn/ProfilePanel]:
		panel.visible = not on_its_own
	_workout_panel.visible = not _workout.is_empty()
	_workout_panel.show_workout(_workout)
	var rider: Dictionary = _torqa.profile()
	var ftp: float = rider.get("ftp_w", 200.0)
	_settings_dialog.configure_workout(_torqa.heart_rate_zones(), ftp, _torqa.workouts())
	_show_plan(ftp)
	_minimap.set_track(_torqa.track(2000))
	_minimap.set_map(_torqa.minimap_mesh())
	_profile.set_profile(_torqa.elevation_profile(600))
	var climb_info: Dictionary = _torqa.climbs()
	var climbs: Array = climb_info.get("climbs", [])
	_profile.set_climbs(climbs)
	_climb_panel.hide()
	var profile: Dictionary = _torqa.profile()
	_imperial = profile.get("units", "metric") == "imperial"
	_hud.imperial = _imperial
	_hud.show_layout(_torqa.hud_layout())
	if not _torqa.trainer_connected():
		_show_toast(tr("Waiting for the trainer…"))
	# Workouts are not sped up: a simulated heart beats in real time.
	var simulating: bool = _torqa.simulating() and _workout.is_empty()
	_simulation.visible = simulating
	_minimap.jumpable = simulating
	_profile.jumpable = simulating
	_set_time_scale(1.0)
	_frame_time = 0.0
	_slow_for = 0.0
	_budget_noted = false


func _ready() -> void:
	# The map panel clips the map to its rounded shape; it must be opaque, as the clip mask
	# also takes its transparency.
	var map_panel: Panel = _minimap.get_parent() as Panel
	var opaque: StyleBoxFlat = UiTheme.panel()
	opaque.bg_color = Color(0.1, 0.11, 0.12)
	map_panel.add_theme_stylebox_override("panel", opaque)
	_build_backdrop()
	_build_workout_panel()
	_build_climb_panel()
	_build_ghost_panel()
	_build_simulation_panel()
	_minimap.jump_requested.connect(
		func(position_m: Vector2) -> void: _torqa.jump_near(position_m.x, position_m.y)
	)
	_profile.jump_requested.connect(
		func(distance_m: float) -> void: _torqa.jump_to_distance(distance_m)
	)
	add_child(_settings_dialog)
	_settings_dialog.options_changed.connect(_on_options_changed)
	_settings_dialog.workout_changed.connect(_on_workout_changed)
	_settings_dialog.hud_changed.connect(_on_hud_changed)
	_settings_dialog.finish_requested.connect(_on_finish_requested)
	_settings_dialog.abort_requested.connect(_on_abort_requested)
	# Over the 3D scene, the light default buttons let road markings shine through the text.
	for button: Button in [_settings_button, _finish_button]:
		button.add_theme_stylebox_override("normal", UiTheme.hud_button())
	_settings_button.pressed.connect(_open_settings)
	_finish_button.pressed.connect(_on_finish_pressed)
	_overlay_button.text = tr("Overlay")
	_overlay_button.tooltip_text = tr(
		"Only the HUD, on top of other windows, e.g. over a video (O)"
	)
	_overlay_button.focus_mode = Control.FOCUS_NONE
	_overlay_button.add_theme_stylebox_override("normal", UiTheme.hud_button())
	_overlay_button.pressed.connect(func() -> void: overlay_requested.emit(true))
	_settings_button.add_sibling(_overlay_button)
	_pause_button.text = tr("Pause")
	_pause_button.tooltip_text = tr("Pause the ride: the clock and the trainer wait (P)")
	_pause_button.focus_mode = Control.FOCUS_NONE
	_pause_button.add_theme_stylebox_override("normal", UiTheme.hud_button())
	_pause_button.pressed.connect(_toggle_pause)
	_overlay_button.add_sibling(_pause_button)
	_overlay_hud.pause_requested.connect(_toggle_pause)
	_overlay_hud.hide()
	_overlay_hud.leave_requested.connect(func() -> void: overlay_requested.emit(false))
	_overlay_hud.zoom_requested.connect(overlay_zoom_requested.emit)
	add_child(_overlay_hud)


func _unhandled_input(event: InputEvent) -> void:
	var key: InputEventKey = event as InputEventKey
	if not visible or key == null or not key.pressed or key.echo:
		return
	if is_overlay():
		# The overlay is too small for dialogs; it only goes back or changes its size.
		if key.keycode in [KEY_O, KEY_ESCAPE]:
			overlay_requested.emit(false)
		elif key.keycode in [KEY_EQUAL, KEY_PLUS, KEY_KP_ADD]:
			overlay_zoom_requested.emit(1)
		elif key.keycode in [KEY_MINUS, KEY_KP_SUBTRACT]:
			overlay_zoom_requested.emit(-1)
		elif key.keycode in [KEY_P, KEY_SPACE]:
			_toggle_pause()
		else:
			return
		get_viewport().set_input_as_handled()
		return
	if key.keycode in [KEY_UP, KEY_DOWN] and not _world.is_free_camera():
		# Virtual gears (R9); with a cassette the rider shifts on the bike.
		_torqa.shift(1 if key.keycode == KEY_UP else -1)
	elif key.keycode == KEY_O and not _finished:
		overlay_requested.emit(true)
	elif key.keycode in [KEY_P, KEY_SPACE] and not _finished:
		_toggle_pause()
	elif key.keycode == KEY_C:
		_cycle_camera()
	elif key.keycode == KEY_S and not _finished:
		_open_settings()
	elif _simulation.visible and key.keycode in [KEY_EQUAL, KEY_PLUS, KEY_KP_ADD]:
		_step_time_scale(1)
	elif _simulation.visible and key.keycode in [KEY_MINUS, KEY_KP_SUBTRACT]:
		_step_time_scale(-1)
	elif MUSIC_KEYS.has(key.keycode):
		_control_music(MUSIC_KEYS[key.keycode])
	else:
		return
	get_viewport().set_input_as_handled()


func _process(delta: float) -> void:
	if _toast_left > 0.0:
		_toast_left -= delta
		if _toast_left <= 0.0:
			_toast.hide()
	if not visible or _finished:
		return
	_watch_frame_time(delta)
	var state: Dictionary = _torqa.ride_state()
	if state.is_empty():
		return
	if is_overlay():
		_overlay_hud.show_state(state)
	_show_gear(state["gear"])
	var metrics: Dictionary = state["metrics"]
	_hud.show_values(metrics, state["watts_per_kg"], state["power_zone"])
	_chart.heart_rate_target = _workout_panel.show_state(state["workout"], state["heart_rate"])
	if _chart_panel.visible:
		_chart_left -= delta
		if _chart_left <= 0.0:
			_chart_left = CHART_EVERY_S
			_plan_chart.progress_s = state["elapsed_s"]
			var chart: Dictionary = _torqa.ride_chart(600)
			var power: PackedVector2Array = chart["power"]
			var heart_rate: PackedVector2Array = chart["heart_rate"]
			_chart.set_series(PackedVector2Array(), power, heart_rate)
	if not state.has("x"):
		return
	var grade: float = state["grade"]
	var elevation: float = state["elevation_m"]
	var distance_m: float = state["distance_m"]
	var x_m: float = state["x"]
	var y_m: float = state["y"]
	var heading: float = state["heading"]
	if _imperial:
		_profile_info.text = "%d ft  ·  %+.1f %%" % [roundi(elevation / METERS_PER_FOOT), grade]
	else:
		_profile_info.text = "%d m  ·  %+.1f %%" % [roundi(elevation), grade]
	_minimap.set_rider(Vector2(x_m, y_m), heading)
	_show_climb(state["climb"])
	_show_ghost(state["ghost"])
	_profile.set_rider_distance(distance_m)


## Whether the screen shows as the overlay.
func is_overlay() -> bool:
	return _overlay_hud.visible


## Shows only the HUD and the workout, for the overlay window (R55), or the whole screen again.
func set_overlay(on: bool) -> void:
	if on == is_overlay():
		return
	if on:
		_overlay_hud.begin(_torqa.hud_layout(), _imperial, _workout)
		_hidden_by_overlay.clear()
		for child: Node in get_children():
			var control: Control = child as Control
			if control != null and control != _overlay_hud and control.visible:
				control.hide()
				_hidden_by_overlay.append(control)
		_overlay_hud.show()
	else:
		_overlay_hud.hide()
		for control: Control in _hidden_by_overlay:
			# A message whose time ran out meanwhile stays gone.
			if control != _toast or _toast_left > 0.0:
				control.show()
		_hidden_by_overlay.clear()


## The size the overlay's content needs, in interface units.
func overlay_content_size() -> Vector2:
	return _overlay_hud.content_size()


## The outline of the overlay's content in window pixels.
func overlay_outline() -> PackedVector2Array:
	return _overlay_hud.clickable_outline()


## Tells the rider of a shift (R9).
func _show_gear(gear: Variant) -> void:
	if gear == null:
		return
	var info: Dictionary = gear
	var number: int = info["number"]
	var of: int = info["of"]
	if _gear != 0 and number != _gear:
		_show_toast(tr("Gear %d of %d") % [number, of])
	_gear = number


func _open_settings() -> void:
	_options["camera"] = _world.camera()
	var world: bool = not _torqa.riding_along_video()
	_settings_dialog.edit(_options, _torqa.hud_layout(), _imperial, world)


## Applies changed ride options at once: camera, conditions and sound in the world, difficulty
## and descents on the trainer.
func _on_options_changed(options: Dictionary) -> void:
	for key: String in ["workout", "on_course"]:
		if _options.has(key):
			options[key] = _options[key]
	_options = options
	_world.apply_options(options)
	var difficulty: float = options["difficulty"]
	var flat_descents: bool = options["flat_descents"]
	_torqa.adjust_ride(difficulty, flat_descents)
	var video_sound: bool = options.get("video_sound", true)
	_torqa.set_video_sound(video_sound)


## A changed workout reaches the trainer at once; the ride keeps the name it started with.
func _on_workout_changed(workout: Dictionary) -> void:
	workout["name"] = _workout.get("name", "")
	_workout = workout
	_options["workout"] = workout
	_torqa.change_workout(workout)
	_workout_panel.show_workout(workout)
	var ftp: float = _torqa.profile().get("ftp_w", 200.0)
	_show_plan(ftp)


## A structured workout's steps over the chart, its time axis as long as the workout.
func _show_plan(ftp: float) -> void:
	var plan: Dictionary = _workout.get("plan", {})
	_plan_chart.visible = not plan.is_empty()
	_chart.min_duration = CHART_MIN_S
	_chart.power_range = Vector2.ZERO
	if plan.is_empty():
		return
	var steps: Array = plan["steps"]
	_plan_chart.set_steps(steps, ftp)
	var duration: float = plan["duration_s"]
	_chart.min_duration = duration
	_chart.power_range = Vector2(0.0, _plan_chart.top_w())


func _on_hud_changed(layout: PackedStringArray) -> void:
	var saved: PackedStringArray = _torqa.set_hud_layout(layout)
	if not saved.is_empty():
		_hud.show_layout(saved)


func _on_finish_requested() -> void:
	_summary_when_saved = true
	_finish_ride()


func _on_abort_requested() -> void:
	_torqa.abort_ride()
	_finished = true
	_pause_button.hide()
	closed.emit()


func _build_backdrop() -> void:
	_backdrop.color = Color(0.07, 0.08, 0.1)
	_backdrop.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	_backdrop.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_backdrop.hide()
	add_child(_backdrop)
	move_child(_backdrop, 0)
	# Between the HUD and the workout panel, from the top down to the buttons.
	_chart_panel.anchor_right = 1.0
	_chart_panel.anchor_bottom = 1.0
	_chart_panel.offset_left = 308.0
	_chart_panel.offset_right = -348.0
	_chart_panel.offset_top = 24.0
	_chart_panel.offset_bottom = -92.0
	_chart.min_duration = CHART_MIN_S
	_chart_panel.mouse_filter = Control.MOUSE_FILTER_IGNORE
	var rows: VBoxContainer = VBoxContainer.new()
	var legend: HBoxContainer = HBoxContainer.new()
	legend.add_theme_constant_override("separation", 16)
	# i18n-begin
	for entry: Array in [["Power", UiTheme.POWER_COLOR], ["Heart rate", UiTheme.HEART_RATE_COLOR]]:
		# i18n-end
		var text: String = entry[0]
		var color: Color = entry[1]
		var caption: Label = UiTheme.caption(text)
		caption.add_theme_color_override("font_color", color)
		legend.add_child(caption)
	rows.add_child(legend)
	# A structured workout's steps lie behind the power and heart rate, on the same scales.
	var area: Control = Control.new()
	area.size_flags_vertical = Control.SIZE_EXPAND_FILL
	for chart: Control in [_plan_chart, _chart]:
		chart.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
		area.add_child(chart)
	_plan_chart.modulate = Color(1, 1, 1, 0.4)
	_plan_chart.hide()
	rows.add_child(area)
	_chart_panel.add_child(rows)
	_chart_panel.hide()
	add_child(_chart_panel)
	move_child(_chart_panel, 1)


func _build_workout_panel() -> void:
	_workout_panel.hide()
	var column: VBoxContainer = $RightColumn
	column.add_child(_workout_panel)
	column.move_child(_workout_panel, 0)


func _build_ghost_panel() -> void:
	var rows: VBoxContainer = VBoxContainer.new()
	rows.add_theme_constant_override("separation", 2)
	_ghost_name.add_theme_color_override("font_color", UiTheme.GHOST_COLOR)
	rows.add_child(_ghost_name)
	rows.add_child(_ghost_gap)
	_ghost_panel.add_child(rows)
	_ghost_panel.hide()
	($RightColumn as VBoxContainer).add_child(_ghost_panel)


## The ghost (`ride_state()["ghost"]`) on the map and profile, and the time gap to it.
func _show_ghost(ghost: Variant) -> void:
	if ghost == null:
		_ghost_panel.hide()
		_minimap.set_ghost(Vector2.ZERO, false)
		_profile.set_ghost_distance(-1.0)
		return
	var info: Dictionary = ghost
	var x_m: float = info["x"]
	var y_m: float = info["y"]
	var distance_m: float = info["distance_m"]
	var ghost_name: String = info["name"]
	_minimap.set_ghost(Vector2(x_m, y_m), true)
	_profile.set_ghost_distance(distance_m)
	_ghost_name.text = ghost_name.to_upper()
	if info["gap_s"] == null:
		_ghost_gap.text = tr("Finished")
		_ghost_gap.remove_theme_color_override("font_color")
	else:
		var gap: float = info["gap_s"]
		var behind: bool = gap > 0.0
		_ghost_gap.text = (
			(tr("%s behind") if behind else tr("%s ahead")) % UiTheme.duration(absf(gap))
		)
		_ghost_gap.add_theme_color_override(
			"font_color", UiTheme.HEART_RATE_COLOR if behind else UiTheme.CLIMB_COLORS["Cat 4"]
		)
	_ghost_panel.show()


func _build_climb_panel() -> void:
	var rows: VBoxContainer = VBoxContainer.new()
	rows.add_theme_constant_override("separation", 2)
	rows.add_child(_climb_title)
	rows.add_child(_climb_left)
	_climb_time.add_theme_font_size_override("font_size", 14)
	_climb_time.add_theme_color_override("font_color", UiTheme.MUTED)
	rows.add_child(_climb_time)
	_climb_panel.add_child(rows)
	_climb_panel.hide()
	($RightColumn as VBoxContainer).add_child(_climb_panel)


## Progress on the climb the rider is on (`ride_state()["climb"]`), hidden between climbs.
func _show_climb(climb: Variant) -> void:
	if climb == null:
		_climb_panel.hide()
		return
	var info: Dictionary = climb
	var index: int = info["index"]
	var count: int = info["count"]
	var category: String = info["category"]
	var left_m: float = info["length_m"] - info["ridden_m"]
	var grade: float = info["grade"]
	var elapsed: float = info["elapsed_s"]
	_climb_title.text = (tr("%s  ·  climb %d of %d") % [tr(category).to_upper(), index + 1, count])
	var color: Color = UiTheme.CLIMB_COLORS.get(category, UiTheme.MUTED)
	_climb_title.add_theme_color_override("font_color", color)
	var left: String = (
		"%.2f mi" % (left_m / 1000.0 / KM_PER_MILE)
		if _imperial
		else ("%.1f km" % (left_m / 1000.0) if left_m >= 1000.0 else "%d m" % roundi(left_m))
	)
	_climb_left.text = tr("%s to go  ·  %.1f %%") % [left, grade]
	var time: String = UiTheme.duration(elapsed)
	if info["best_s"] != null:
		var best: float = info["best_s"]
		time += "  ·  " + tr("best %s") % UiTheme.duration(best)
	_climb_time.text = time
	_climb_panel.show()


func _on_climb_completed(_index: int, elapsed_s: float, previous_best_s: float) -> void:
	_show_toast(
		(
			tr("Climb done in %s") % UiTheme.duration(elapsed_s)
			+ _record_text(elapsed_s, previous_best_s)
		)
	)


func _on_route_completed(elapsed_s: float, previous_best_s: float) -> void:
	_show_toast(
		(
			tr("Finished in %s") % UiTheme.duration(elapsed_s)
			+ _record_text(elapsed_s, previous_best_s)
		)
	)
	_toast_left = TOAST_SECONDS * 2.0


## " — new record, 0:12 faster!", " (best 11:58)" or "" for a first time.
static func _record_text(elapsed_s: float, previous_best_s: float) -> String:
	if previous_best_s < 0.0:
		return ""
	if elapsed_s < previous_best_s:
		return (
			" — "
			+ (
				TranslationServer.translate("new record, %s faster!")
				% UiTheme.duration(previous_best_s - elapsed_s)
			)
		)
	return " " + TranslationServer.translate("(best %s)") % UiTheme.duration(previous_best_s)


## Suggests a lower graphics preset when the ride stays below 60 fps (R43). Simulated rides
## are left alone: they are for trying courses out.
func _watch_frame_time(delta: float) -> void:
	if _budget_noted or _simulation.visible or _torqa.riding_along_video() or _backdrop.visible:
		return
	_frame_time = lerpf(_frame_time if _frame_time > 0.0 else delta, delta, 0.05)
	_slow_for = _slow_for + delta if _frame_time > FRAME_BUDGET_S else 0.0
	if _slow_for > SLOW_FOR_S and _torqa.graphics_quality() != "low":
		_budget_noted = true
		_show_toast(
			tr("Below 60 fps: a lower graphics quality (Devices & Settings) runs smoother.")
		)


func _build_simulation_panel() -> void:
	_simulation.set_anchors_and_offsets_preset(Control.PRESET_CENTER_BOTTOM)
	_simulation.grow_horizontal = Control.GROW_DIRECTION_BOTH
	_simulation.grow_vertical = Control.GROW_DIRECTION_BEGIN
	_simulation.position.y -= 20.0
	_simulation.mouse_filter = Control.MOUSE_FILTER_STOP
	var rows: VBoxContainer = VBoxContainer.new()
	var speeds: HBoxContainer = HBoxContainer.new()
	speeds.add_theme_constant_override("separation", 6)
	speeds.add_child(UiTheme.caption(tr("Simulation")))
	var group: ButtonGroup = ButtonGroup.new()
	for scale: float in TIME_SCALES:
		var button: Button = Button.new()
		button.text = "%d×" % roundi(scale)
		button.toggle_mode = true
		button.button_group = group
		button.focus_mode = Control.FOCUS_NONE
		button.auto_translate_mode = Node.AUTO_TRANSLATE_MODE_DISABLED
		button.pressed.connect(_set_time_scale.bind(scale))
		speeds.add_child(button)
		_speed_buttons.append(button)
	rows.add_child(speeds)
	var hint: Label = Label.new()
	hint.text = tr(
		"Click the map or profile to jump · + / − speed · C: free camera, Shift + arrows or mouse to look"
	)
	hint.add_theme_font_size_override("font_size", 11)
	hint.add_theme_color_override("font_color", UiTheme.MUTED)
	rows.add_child(hint)
	_simulation.add_child(rows)
	_simulation.hide()
	add_child(_simulation)


func _set_time_scale(scale: float) -> void:
	_time_scale = _torqa.set_time_scale(scale) if _torqa != null else 1.0
	for i: int in range(_speed_buttons.size()):
		_speed_buttons[i].set_pressed_no_signal(is_equal_approx(TIME_SCALES[i], _time_scale))


func _step_time_scale(step: int) -> void:
	var index: int = TIME_SCALES.find(_time_scale)
	_set_time_scale(TIME_SCALES[clampi(index + step, 0, TIME_SCALES.size() - 1)])
	_show_toast(tr("Simulation: %d×") % roundi(_time_scale))


## A shifter's button asked for a control (#139): done as its key does.
func _on_control_requested(control: String) -> void:
	if not visible:
		return
	if control == "next_camera":
		# The overlay shows no world to look at.
		if not is_overlay():
			_cycle_camera()
	elif control == "overlay":
		if not _finished:
			overlay_requested.emit(not is_overlay())
	else:
		for music: Array in MUSIC_KEYS.values():
			if music[0] == control:
				_control_music(music)


## Plays, pauses or skips the music: `music` is a command and its message, as in MUSIC_KEYS.
func _control_music(music: Array) -> void:
	var command: String = music[0]
	var message: String = music[1]
	_torqa.control_music(command)
	_show_toast(tr(message))


func _cycle_camera() -> void:
	if _torqa.riding_along_video() or _backdrop.visible:
		return
	_show_toast(tr("Camera: %s") % tr(_world.cycle_camera()))


## Pauses the ride, or goes on with it: the clock, the road and the trainer wait (P).
func _toggle_pause() -> void:
	if _finished:
		return
	_show_paused(_torqa.set_paused(not _torqa.is_paused()))


func _show_paused(paused: bool) -> void:
	_pause_button.text = tr("Resume") if paused else tr("Pause")
	_overlay_hud.show_paused(paused)
	if paused:
		_show_toast(tr("Paused"))
		_toast_left = INF
	elif _toast_left == INF:
		_toast_left = 0.0
		_toast.hide()


func _show_toast(message: String) -> void:
	_toast_label.text = message
	# The overlay has no room for messages.
	if not is_overlay():
		_toast.show()
	_toast_left = TOAST_SECONDS


func _on_device_connected(device_name: String) -> void:
	_show_toast(tr("%s connected") % device_name)


func _on_device_disconnected(device_name: String) -> void:
	_show_toast(tr("%s disconnected — reconnecting…") % device_name)


func _on_ride_finished() -> void:
	# The finish toast with the time comes from `route_completed`; the whole screen shows it.
	overlay_requested.emit(false)
	_torqa.finish_ride()


## After the finish: to the summary, or back if nothing was recorded.
func _on_finish_pressed() -> void:
	if _saved:
		summary_requested.emit(_saved_path)
	else:
		closed.emit()


func _finish_ride() -> void:
	_finished = true
	_pause_button.hide()
	_settings_button.hide()
	_torqa.finish_ride()
	if not _saved:
		# Nothing to save (e.g. the trainer never connected): offer the way back.
		_show_toast(tr("Nothing recorded."))
		_finish_button.text = tr("Back")
		_finish_button.show()


func _on_ride_saved(path: String) -> void:
	_finished = true
	_pause_button.hide()
	_saved = true
	_saved_path = path
	_settings_button.hide()
	if _summary_when_saved:
		summary_requested.emit(path)
		return
	_show_toast(tr("Saved %s") % path.get_file())
	_toast_left = TOAST_SECONDS * 2.0
	_finish_button.text = tr("View summary")
	_finish_button.show()


func _on_failed(message: String) -> void:
	if visible:
		_show_toast(message)
