class_name UiTheme
extends RefCounted
## The app's visual style: dark translucent panels, pill buttons and the brand accent.

const ACCENT: Color = Color(0.04, 0.61, 0.96)
const TEXT: Color = Color(0.94, 0.95, 0.97)
const MUTED: Color = Color(0.64, 0.68, 0.74)
const PANEL: Color = Color(0.06, 0.07, 0.09, 0.72)
const SURFACE: Color = Color(1, 1, 1, 0.07)
const RADIUS: int = 14
# i18n-begin: zone and climb names are shown translated.
## Power zones 1–7 (Coggan): name and colour, as commonly used by training platforms.
const POWER_ZONES: Array[Array] = [
	["Recovery", Color(0.6, 0.62, 0.66)],
	["Endurance", Color(0.25, 0.6, 0.95)],
	["Tempo", Color(0.3, 0.8, 0.45)],
	["Threshold", Color(0.98, 0.8, 0.2)],
	["VO2max", Color(0.98, 0.55, 0.2)],
	["Anaerobic", Color(0.95, 0.3, 0.3)],
	["Neuromuscular", Color(0.7, 0.4, 0.95)],
]
## Heart-rate zones 1–5: name and colour.
const HEART_RATE_ZONES: Array[Array] = [
	["Very light", Color(0.6, 0.62, 0.66)],
	["Light", Color(0.25, 0.6, 0.95)],
	["Moderate", Color(0.3, 0.8, 0.45)],
	["Hard", Color(0.98, 0.55, 0.2)],
	["Maximum", Color(0.95, 0.3, 0.3)],
]
## Climb categories as labelled by Torqa, easiest first.
const CLIMB_COLORS: Dictionary[String, Color] = {
	"Climb": Color(0.6, 0.62, 0.66),
	"Cat 4": Color(0.3, 0.8, 0.45),
	"Cat 3": Color(0.98, 0.8, 0.2),
	"Cat 2": Color(0.98, 0.55, 0.2),
	"Cat 1": Color(0.95, 0.3, 0.3),
	"HC": Color(0.7, 0.4, 0.95),
}
## Ghost riders and pacers (R20) on the road, the map and the profile.
const GHOST_COLOR: Color = Color(0.98, 0.55, 0.2)
# i18n-end
const POWER_COLOR: Color = Color(0.04, 0.61, 0.96)
const HEART_RATE_COLOR: Color = Color(0.95, 0.33, 0.38)


static func build() -> Theme:
	var theme: Theme = Theme.new()
	theme.default_font_size = 15

	theme.set_stylebox("panel", "PanelContainer", panel())
	theme.set_stylebox("panel", "Panel", panel())

	theme.set_color("font_color", "Label", TEXT)

	for type: String in ["Button", "OptionButton", "CheckBox"]:
		theme.set_color("font_color", type, TEXT)
		theme.set_color("font_hover_color", type, TEXT)
		theme.set_color("font_pressed_color", type, Color.WHITE)
		theme.set_color("font_disabled_color", type, Color(TEXT, 0.35))
	for type: String in ["Button", "OptionButton"]:
		theme.set_stylebox("normal", type, _box(SURFACE, 10, 14, 9))
		theme.set_stylebox("hover", type, _box(Color(1, 1, 1, 0.13), 10, 14, 9))
		theme.set_stylebox("pressed", type, _box(Color(ACCENT, 0.85), 10, 14, 9))
		theme.set_stylebox("disabled", type, _box(Color(1, 1, 1, 0.04), 10, 14, 9))
		theme.set_stylebox("focus", type, StyleBoxEmpty.new())
	theme.set_stylebox("normal", "CheckBox", StyleBoxEmpty.new())
	theme.set_stylebox("hover", "CheckBox", StyleBoxEmpty.new())
	theme.set_stylebox("pressed", "CheckBox", StyleBoxEmpty.new())
	theme.set_stylebox("focus", "CheckBox", StyleBoxEmpty.new())
	# The default boxes are dark grey, invisible on dark panels.
	theme.set_icon("unchecked", "CheckBox", _check_icon(false))
	theme.set_icon("checked", "CheckBox", _check_icon(true))
	theme.set_icon("unchecked_disabled", "CheckBox", _check_icon(false, 0.35))
	theme.set_icon("checked_disabled", "CheckBox", _check_icon(true, 0.35))

	var field: StyleBoxFlat = _box(SURFACE, 10, 12, 8)
	theme.set_stylebox("normal", "LineEdit", field)
	theme.set_stylebox("focus", "LineEdit", _box(Color(1, 1, 1, 0.12), 10, 12, 8))
	theme.set_color("font_color", "LineEdit", TEXT)

	theme.set_stylebox("background", "ProgressBar", _box(Color(1, 1, 1, 0.1), 6, 0, 4))
	theme.set_stylebox("fill", "ProgressBar", _box(ACCENT, 6, 0, 4))

	theme.set_stylebox("slider", "HSlider", _box(Color(1, 1, 1, 0.14), 4, 0, 3))
	theme.set_stylebox("grabber_area", "HSlider", _box(ACCENT, 4, 0, 3))
	theme.set_stylebox("grabber_area_highlight", "HSlider", _box(ACCENT, 4, 0, 3))

	# Dialogs are separate windows: they need the theme set on them and their own frame.
	# Square: the frame rounds the window; rounded corners here would let the window's grey
	# background show through. Not anti-aliased either: that fades the outermost pixels, and
	# the grey showed through them as thin lines along the dialog's edges (#48).
	var dialog: StyleBoxFlat = _box(Color(0.09, 0.1, 0.12), 0, 20, 16)
	dialog.anti_aliasing = false
	theme.set_stylebox("panel", "AcceptDialog", dialog)
	var frame: StyleBoxFlat = _box(Color(0.09, 0.1, 0.12), RADIUS, 0, 0)
	frame.expand_margin_top = 32
	frame.expand_margin_left = 6
	frame.expand_margin_right = 6
	frame.expand_margin_bottom = 6
	frame.border_color = Color(1, 1, 1, 0.08)
	frame.set_border_width_all(1)
	theme.set_stylebox("embedded_border", "Window", frame)
	theme.set_stylebox("embedded_unfocused_border", "Window", frame)
	theme.set_color("title_color", "Window", TEXT)

	# Tabs: plain captions, the current one underlined in the accent colour.
	var tab: StyleBoxFlat = _box(Color(0, 0, 0, 0), 0, 14, 8)
	var current_tab: StyleBoxFlat = tab.duplicate()
	current_tab.border_color = ACCENT
	current_tab.border_width_bottom = 2
	theme.set_stylebox("tab_selected", "TabContainer", current_tab)
	theme.set_stylebox("tab_unselected", "TabContainer", tab)
	theme.set_stylebox("tab_hovered", "TabContainer", tab)
	theme.set_stylebox("tabbar_background", "TabContainer", StyleBoxEmpty.new())
	var tab_panel: StyleBoxFlat = _box(Color(0, 0, 0, 0), 0, 0, 16)
	tab_panel.border_color = Color(1, 1, 1, 0.08)
	tab_panel.border_width_top = 1
	theme.set_stylebox("panel", "TabContainer", tab_panel)
	theme.set_color("font_selected_color", "TabContainer", TEXT)
	theme.set_color("font_unselected_color", "TabContainer", MUTED)
	theme.set_color("font_hovered_color", "TabContainer", TEXT)

	# Scroll bars: a slim light thumb, no track.
	for bar: String in ["VScrollBar", "HScrollBar"]:
		theme.set_stylebox("scroll", bar, _box(Color(0, 0, 0, 0), 3, 3, 3))
		theme.set_stylebox("grabber", bar, _box(Color(1, 1, 1, 0.18), 3, 3, 3))
		theme.set_stylebox("grabber_highlight", bar, _box(Color(1, 1, 1, 0.3), 3, 3, 3))
		theme.set_stylebox("grabber_pressed", bar, _box(Color(ACCENT, 0.6), 3, 3, 3))

	var popup: StyleBoxFlat = _box(Color(0.09, 0.1, 0.12, 0.98), 10, 6, 6)
	theme.set_stylebox("panel", "PopupMenu", popup)
	theme.set_stylebox("hover", "PopupMenu", _box(Color(ACCENT, 0.35), 6, 8, 4))
	theme.set_color("font_color", "PopupMenu", TEXT)
	return theme


## The translucent card used for HUD and setup panels.
static func panel() -> StyleBoxFlat:
	var box: StyleBoxFlat = _box(PANEL, RADIUS, 16, 14)
	box.border_color = Color(1, 1, 1, 0.07)
	box.set_border_width_all(1)
	box.shadow_color = Color(0, 0, 0, 0.22)
	box.shadow_size = 12
	box.anti_aliasing = true
	return box


## The main action of a screen (e.g. Ride), in the accent colour.
static func accent_button() -> StyleBoxFlat:
	return _box(ACCENT, 12, 18, 10)


## A button background for use over the 3D scene, as dark as the HUD panels.
static func hud_button() -> StyleBoxFlat:
	return _box(PANEL, 10, 14, 9)


## The slim bar of icon buttons over the 3D scene (#189).
static func bar() -> StyleBoxFlat:
	return _box(PANEL, 12, 6, 4)


## A list entry that can be dragged; the highlighted one marks the HUD's large figure.
static func chip(highlighted: bool) -> StyleBoxFlat:
	var box: StyleBoxFlat = _box(Color(ACCENT, 0.22) if highlighted else SURFACE, 8, 12, 4)
	box.border_color = Color(ACCENT, 0.6) if highlighted else Color(1, 1, 1, 0.06)
	box.set_border_width_all(1)
	return box


## A rider's initial in a round accent badge, standing in for a face (#191).
static func initial(rider_name: String, size: int = 40) -> PanelContainer:
	var badge: PanelContainer = PanelContainer.new()
	badge.custom_minimum_size = Vector2(size, size)
	badge.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	badge.add_theme_stylebox_override("panel", _box(ACCENT, size / 2, 0, 0))
	var label: Label = Label.new()
	label.auto_translate_mode = Node.AUTO_TRANSLATE_MODE_DISABLED
	label.text = rider_name.strip_edges().left(1).to_upper()
	label.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	label.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	label.add_theme_font_size_override("font_size", size / 2)
	badge.add_child(label)
	return badge


## A caption label: small, muted, upper case; `text` is translated first.
static func caption(text: String) -> Label:
	var label: Label = Label.new()
	# Translated here, before upper-casing; the label must not look up the upper-cased text.
	label.auto_translate_mode = Node.AUTO_TRANSLATE_MODE_DISABLED
	label.text = TranslationServer.translate(text).to_upper()
	label.add_theme_font_size_override("font_size", 11)
	label.add_theme_color_override("font_color", MUTED)
	return label


## A value label in the given size.
static func value(size: int) -> Label:
	var label: Label = Label.new()
	label.text = "--"
	label.add_theme_font_size_override("font_size", size)
	return label


static func _box(color: Color, radius: int, horizontal: int, vertical: int) -> StyleBoxFlat:
	var box: StyleBoxFlat = StyleBoxFlat.new()
	box.bg_color = color
	box.set_corner_radius_all(radius)
	box.content_margin_left = horizontal
	box.content_margin_right = horizontal
	box.content_margin_top = vertical
	box.content_margin_bottom = vertical
	box.anti_aliasing = true
	return box


## A time as m:ss, or h:mm:ss from an hour.
static func duration(seconds: float) -> String:
	var total: int = roundi(seconds)
	if total >= 3600:
		return "%d:%02d:%02d" % [total / 3600, total / 60 % 60, total % 60]
	return "%d:%02d" % [total / 60, total % 60]


## An 18 px check box: a light outline, or an accent square with a white tick.
static func _check_icon(checked: bool, alpha: float = 1.0) -> ImageTexture:
	const SIZE: int = 18
	var image: Image = Image.create_empty(SIZE, SIZE, false, Image.FORMAT_RGBA8)
	var outline: Color = Color(TEXT, 0.55 * alpha)
	var fill: Color = Color(ACCENT, alpha)
	for y: int in range(1, SIZE - 1):
		for x: int in range(1, SIZE - 1):
			# Corners left out for a slightly rounded look.
			var corner: bool = (x == 1 or x == SIZE - 2) and (y == 1 or y == SIZE - 2)
			if corner:
				continue
			var edge: bool = x <= 2 or y <= 2 or x >= SIZE - 3 or y >= SIZE - 3
			if checked:
				image.set_pixel(x, y, fill)
			elif edge:
				image.set_pixel(x, y, outline)
	if checked:
		var tick: Color = Color(1, 1, 1, alpha)
		for i: int in range(4):
			image.set_pixel(4 + i, 8 + i, tick)
			image.set_pixel(4 + i, 9 + i, tick)
		for i: int in range(7):
			image.set_pixel(7 + i, 11 - i, tick)
			image.set_pixel(7 + i, 10 - i, tick)
	return ImageTexture.create_from_image(image)
