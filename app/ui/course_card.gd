class_name CourseCard
extends PanelContainer
## A course in the gallery (R39): its path card, name and key figures in rows, marked if it is
## ridden along a video (R17); click to open it. The elevation profile is left to the course
## page.

signal pressed

const WIDTH: float = 300.0

var _title: Label = Label.new()
var _path_card: PathCard = PathCard.new()


func _init(course: Dictionary, imperial: bool) -> void:
	custom_minimum_size = Vector2(WIDTH, 0)
	mouse_default_cursor_shape = Control.CURSOR_POINTING_HAND
	var rows: VBoxContainer = VBoxContainer.new()
	rows.add_theme_constant_override("separation", 12)
	rows.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_path_card.custom_minimum_size = Vector2(WIDTH - 32.0, 160)
	_path_card.mouse_filter = Control.MOUSE_FILTER_IGNORE
	var track: PackedVector2Array = course.get("track", PackedVector2Array())
	path.set_track(track)
	rows.add_child(path)
	_title.auto_translate_mode = Node.AUTO_TRANSLATE_MODE_DISABLED
	_title.text = course["name"]
	_title.add_theme_font_size_override("font_size", 18)
	_title.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_title.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	# Sees the mouse for its tooltip; the click still reaches the card.
	_title.mouse_filter = Control.MOUSE_FILTER_PASS
	_title.resized.connect(_fit_title)
	_path_card.set_track(track)
	rows.add_child(_path_card)
	var title: Label = Label.new()
	title.auto_translate_mode = Node.AUTO_TRANSLATE_MODE_DISABLED
	title.text = course["name"]
	title.add_theme_font_size_override("font_size", 18)
	title.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	title.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	var heading: HBoxContainer = HBoxContainer.new()
	heading.mouse_filter = Control.MOUSE_FILTER_IGNORE
	heading.add_child(_title)
	if not str(course.get("video", "")).is_empty():
		heading.add_child(video_badge())
	rows.add_child(heading)
	rows.add_child(figure_rows(course, imperial))
	add_child(rows)


## A name the card's width cuts off shows in full on hover, as file managers do; one that fits
## needs no tooltip.
func _fit_title() -> void:
	var font: Font = _title.get_theme_font("font")
	var size: int = _title.get_theme_font_size("font_size")
	var width: float = font.get_string_size(_title.text, HORIZONTAL_ALIGNMENT_LEFT, -1, size).x
	_title.tooltip_text = _title.text if width > _title.size.x else ""


## The name shown on hover; empty while the whole name fits on the card.
func name_tooltip() -> String:
	return _title.tooltip_text
## Shows the course's map under its route, `png` as the app drew it (#192); empty for none.
func set_map(png: PackedByteArray) -> void:
	_path_card.set_map(png)


## The mark of a video course.
static func video_badge() -> PanelContainer:
	var badge: PanelContainer = PanelContainer.new()
	badge.mouse_filter = Control.MOUSE_FILTER_IGNORE
	badge.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	badge.add_theme_stylebox_override("panel", UiTheme.chip(true))
	var label: Label = Label.new()
	label.text = TranslationServer.translate("Video")
	label.add_theme_font_size_override("font_size", 12)
	badge.add_child(label)
	return badge


## The key figures, one per row: caption on the left, value on the right, in the rider's units.
static func figure_rows(course: Dictionary, imperial: bool) -> GridContainer:
	var grid: GridContainer = GridContainer.new()
	grid.columns = 2
	grid.mouse_filter = Control.MOUSE_FILTER_IGNORE
	grid.add_theme_constant_override("h_separation", 16)
	grid.add_theme_constant_override("v_separation", 6)
	for figure: PackedStringArray in figures(course, imperial):
		var caption: Label = Label.new()
		caption.text = TranslationServer.translate(figure[0])
		caption.auto_translate_mode = Node.AUTO_TRANSLATE_MODE_DISABLED
		caption.add_theme_color_override("font_color", UiTheme.MUTED)
		caption.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		grid.add_child(caption)
		var value: Label = Label.new()
		value.text = figure[1]
		value.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
		grid.add_child(value)
	return grid


## `[caption, value]` pairs: length, climbing and steepest gradient, in the rider's units.
static func figures(course: Dictionary, imperial: bool) -> Array[PackedStringArray]:
	var length_km: float = course["length_m"] / 1000.0
	var gain_m: float = course["elevation_gain_m"]
	var max_grade: float = course["max_grade"]
	var distance: String = (
		"%.1f mi" % (length_km / HudPanel.KM_PER_MILE) if imperial else "%.1f km" % length_km
	)
	var climbing: String = (
		"%d ft" % roundi(gain_m / HudPanel.METERS_PER_FOOT) if imperial else "%d m" % roundi(gain_m)
	)
	# i18n-begin
	return [
		PackedStringArray(["Length", distance]),
		PackedStringArray(["Climbing", climbing]),
		PackedStringArray(["Steepest", "%d %%" % roundi(max_grade)]),
	]
	# i18n-end


func _gui_input(event: InputEvent) -> void:
	var click: InputEventMouseButton = event as InputEventMouseButton
	if click != null and click.pressed and click.button_index == MOUSE_BUTTON_LEFT:
		pressed.emit()
