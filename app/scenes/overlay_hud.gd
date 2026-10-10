class_name OverlayHud
extends Control
## What the overlay shows (R55): the rider's HUD and the workout's targets under a slim bar
## that moves the window, with a grip in the corner to resize it. The rest of the ride screen
## is hidden while it shows.

## Full view was asked for.
signal leave_requested
## The rider wants the overlay `steps` sizes larger, or smaller if negative (#124).
signal zoom_requested(steps: int)
## The rider wants the ride paused, or to go on with it.
signal pause_requested

## The HUD keeps the width it has in the ride screen.
const HUD_WIDTH: float = 260.0
const GRIP_SIZE: float = 14.0

var _box: VBoxContainer = VBoxContainer.new()
var _bar: PanelContainer = PanelContainer.new()
var _hud: HudPanel = HudPanel.new()
var _workout: WorkoutPanel = WorkoutPanel.new()
var _grip: Control = Control.new()

var _pause: Button


func _init() -> void:
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	mouse_filter = Control.MOUSE_FILTER_IGNORE
	# The window is see-through between the panels; the panels themselves are opaque here.
	# Over the 3D scene their translucency only tints the view, but over another window its
	# text and icons showed through the figures.
	var opaque: Theme = Theme.new()
	opaque.set_stylebox("panel", "PanelContainer", UiTheme.panel(1.0))
	theme = opaque
	_box.add_theme_constant_override("separation", 6)
	_box.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(_box)

	var bar: HBoxContainer = HBoxContainer.new()
	var title: Label = UiTheme.caption("Torqa")
	title.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	title.mouse_filter = Control.MOUSE_FILTER_IGNORE
	bar.add_child(title)
	var smaller: Button = _bar_button("A−", tr("Smaller (−)"))
	smaller.pressed.connect(func() -> void: zoom_requested.emit(-1))
	bar.add_child(smaller)
	var larger: Button = _bar_button("A+", tr("Larger (+)"))
	larger.pressed.connect(func() -> void: zoom_requested.emit(1))
	bar.add_child(larger)
	_pause = _bar_button(tr("Pause"), tr("Pause the ride (P)"))
	_pause.pressed.connect(func() -> void: pause_requested.emit())
	bar.add_child(_pause)
	var full: Button = _bar_button(tr("Full view"), tr("Back to the whole ride screen (O or Esc)"))
	full.pressed.connect(func() -> void: leave_requested.emit())
	bar.add_child(full)
	_bar.add_child(bar)
	_bar.tooltip_text = tr("Drag to move the overlay")
	_bar.gui_input.connect(_on_bar_input)
	_box.add_child(_bar)

	var hud_panel: PanelContainer = PanelContainer.new()
	hud_panel.custom_minimum_size.x = HUD_WIDTH
	hud_panel.add_child(_hud)
	_box.add_child(hud_panel)
	_workout.hide()
	_box.add_child(_workout)

	_grip.custom_minimum_size = Vector2(GRIP_SIZE, GRIP_SIZE)
	_grip.size_flags_horizontal = Control.SIZE_SHRINK_END
	_grip.mouse_default_cursor_shape = Control.CURSOR_FDIAGSIZE
	_grip.tooltip_text = tr("Drag to resize the overlay")
	_grip.gui_input.connect(_on_grip_input)
	_grip.draw.connect(_draw_grip)
	_box.add_child(_grip)


## Shows whether the ride is paused on the pause button.
func show_paused(paused: bool) -> void:
	_pause.text = tr("Resume") if paused else tr("Pause")


## Prepares the overlay for the ride: the rider's HUD `layout` and the `workout` being ridden
## (`WorkoutsTab.workout()`, empty on a plain ride).
func begin(layout: PackedStringArray, imperial: bool, workout: Dictionary) -> void:
	_hud.imperial = imperial
	_hud.show_layout(layout)
	_workout.visible = not workout.is_empty()
	_workout.show_workout(workout)
	_box.reset_size()


## Shows the ride's state (`TorqaApp.ride_state()`).
func show_state(state: Dictionary) -> void:
	var metrics: Dictionary = state["metrics"]
	_hud.show_values(metrics, state["watts_per_kg"], state["power_zone"])
	_workout.show_state(state["workout"], state["heart_rate"])


## The size the overlay's content needs, in interface units: the overlay window's base size.
func content_size() -> Vector2:
	return _box.get_combined_minimum_size()


## The outline of the content in window pixels: clicks elsewhere go to the windows below.
func clickable_outline() -> PackedVector2Array:
	var to_window: Transform2D = (
		get_viewport().get_final_transform() * _box.get_global_transform_with_canvas()
	)
	var area: Vector2 = _box.size
	return PackedVector2Array(
		[
			to_window * Vector2.ZERO,
			to_window * Vector2(area.x, 0.0),
			to_window * area,
			to_window * Vector2(0.0, area.y),
		]
	)


func _bar_button(text: String, tooltip: String) -> Button:
	var button: Button = Button.new()
	button.text = text
	button.tooltip_text = tooltip
	button.focus_mode = Control.FOCUS_NONE
	button.add_theme_font_size_override("font_size", 12)
	button.add_theme_stylebox_override("normal", UiTheme.hud_button())
	return button


func _on_bar_input(event: InputEvent) -> void:
	var button: InputEventMouseButton = event as InputEventMouseButton
	if button != null and button.pressed and button.button_index == MOUSE_BUTTON_LEFT:
		# The system moves the window: smooth, and across screens.
		DisplayServer.window_start_drag(get_window().get_window_id())


func _on_grip_input(event: InputEvent) -> void:
	var button: InputEventMouseButton = event as InputEventMouseButton
	if button != null and button.pressed and button.button_index == MOUSE_BUTTON_LEFT:
		DisplayServer.window_start_resize(
			DisplayServer.WINDOW_EDGE_BOTTOM_RIGHT, get_window().get_window_id()
		)


func _draw_grip() -> void:
	# On its own backing: over a video, bare lines would vanish.
	_grip.draw_rect(Rect2(Vector2.ZERO, _grip.size), Color(UiTheme.PANEL, 1.0))
	var color: Color = UiTheme.MUTED
	for i: int in range(3):
		var inset: float = 3.0 + i * 4.0
		_grip.draw_line(Vector2(inset, GRIP_SIZE), Vector2(GRIP_SIZE, inset), color, 1.0, true)
