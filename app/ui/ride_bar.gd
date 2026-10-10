class_name RideBar
extends PanelContainer
## The ride view's controls in one slim bar at the bottom left (#189): icon buttons with
## tooltips for the settings, the overlay and the way out of the ride, and the simulation's
## speeds when the ride is simulated. A chevron folds the bar away to its corner and back;
## the keys work either way.

## The settings were asked for (S).
signal settings_requested
## The overlay was asked for (O).
signal overlay_requested
## The summary, or the way back, was asked for.
signal finish_requested
## A simulation speed was chosen.
signal speed_chosen(scale: float)
## The bar was folded away, or unfolded.
signal folded_changed(folded: bool)

const ICON: int = 20
const TIME_SCALES: Array[float] = [1.0, 2.0, 5.0, 10.0, 20.0]

## Folded away to its corner: only the chevron shows.
var folded: bool = false:
	set(value):
		folded = value
		_tools.visible = not folded
		_fold.icon = UiIcons.texture("unfold" if folded else "fold", ICON)
		_fold.tooltip_text = tr("Show the controls") if folded else tr("Fold the controls away")

var _fold: Button = Button.new()
var _tools: HBoxContainer = HBoxContainer.new()
var _settings: Button = Button.new()
var _overlay: Button = Button.new()
var _finish: Button = Button.new()
var _simulation: HBoxContainer = HBoxContainer.new()
var _speed_buttons: Array[Button] = []


func _init() -> void:
	mouse_filter = Control.MOUSE_FILTER_STOP
	add_theme_stylebox_override("panel", UiTheme.bar())
	var row: HBoxContainer = HBoxContainer.new()
	row.add_theme_constant_override("separation", 4)
	add_child(row)
	_icon_button(_fold, "fold", "")
	_fold.pressed.connect(_toggle_fold)
	row.add_child(_fold)
	_tools.add_theme_constant_override("separation", 4)
	row.add_child(_tools)
	_icon_button(
		_settings, "cog", tr("Camera, difficulty, weather, HUD; finish or abort (keyboard: S)")
	)
	_settings.pressed.connect(settings_requested.emit)
	_tools.add_child(_settings)
	_icon_button(
		_overlay, "overlay", tr("Only the HUD, on top of other windows, e.g. over a video (O)")
	)
	_overlay.pressed.connect(overlay_requested.emit)
	_tools.add_child(_overlay)
	_icon_button(_finish, "flag", tr("View summary"))
	_finish.pressed.connect(finish_requested.emit)
	_finish.hide()
	_tools.add_child(_finish)
	_simulation.add_theme_constant_override("separation", 4)
	_simulation.add_child(VSeparator.new())
	var caption: Label = UiTheme.caption(tr("Simulation"))
	caption.mouse_filter = Control.MOUSE_FILTER_STOP
	caption.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	caption.tooltip_text = tr(
		"Click the map or profile to jump · + / − speed · C: free camera, Shift + arrows or mouse to look"
	)
	_simulation.add_child(caption)
	var group: ButtonGroup = ButtonGroup.new()
	for scale: float in TIME_SCALES:
		var button: Button = Button.new()
		button.text = "%d×" % roundi(scale)
		button.toggle_mode = true
		button.button_group = group
		button.focus_mode = Control.FOCUS_NONE
		button.auto_translate_mode = Node.AUTO_TRANSLATE_MODE_DISABLED
		button.tooltip_text = tr("Simulation speed")
		button.pressed.connect(speed_chosen.emit.bind(scale))
		_simulation.add_child(button)
		_speed_buttons.append(button)
	_simulation.hide()
	_tools.add_child(_simulation)
	folded = false


## Shows the simulation's speeds, or hides them.
func set_simulating(simulating: bool) -> void:
	_simulation.visible = simulating


## Marks the speed in effect.
func show_speed(scale: float) -> void:
	for i: int in range(_speed_buttons.size()):
		_speed_buttons[i].set_pressed_no_signal(is_equal_approx(TIME_SCALES[i], scale))


## Shows the settings button, or hides it (once the ride is over).
func show_settings(shown: bool) -> void:
	_settings.visible = shown


## Shows the way out of the ride: `"summary"`, `"back"` to where the ride came from, or `""`
## for none.
func show_finish(way: String) -> void:
	_finish.visible = not way.is_empty()
	if way == "back":
		_finish.icon = UiIcons.texture("back", ICON)
		_finish.tooltip_text = tr("Back")
	elif way == "summary":
		_finish.icon = UiIcons.texture("flag", ICON)
		_finish.tooltip_text = tr("View summary")


func _toggle_fold() -> void:
	folded = not folded
	folded_changed.emit(folded)


func _icon_button(button: Button, icon: String, tooltip: String) -> void:
	button.icon = UiIcons.texture(icon, ICON)
	button.tooltip_text = tooltip
	button.focus_mode = Control.FOCUS_NONE
	button.add_theme_stylebox_override("normal", StyleBoxEmpty.new())
	button.add_theme_stylebox_override("hover", UiTheme.chip(false))
	button.add_theme_stylebox_override("pressed", UiTheme.chip(true))
