class_name ProfileTab
extends VBoxContainer
## The riders (R22): who rides, their figures, and editing them (with their HUD, R51) or adding
## a new one. Switching riders also switches the interface language (R24).

## The active rider changed (figures, units, language or HUD).
signal profile_changed

## Whether the summary shows every setting the dialog has, not only the figures (#194).
var expanded: bool = false:
	set(value):
		expanded = value
		_more.text = tr("Fewer") if expanded else tr("All settings")
		_more.icon = UiIcons.texture("up" if expanded else "down", 16)
		if _torqa != null:
			_show_summary(_torqa.profile())

var _torqa: TorqaApp
var _riders: OptionButton = OptionButton.new()
var _badge_slot: HBoxContainer = HBoxContainer.new()
var _name_label: Label = Label.new()
var _edit_button: Button = Button.new()
var _add_button: Button = Button.new()
var _summary: GridContainer = GridContainer.new()
var _more: Button = Button.new()
var _dialog: ProfileDialog = ProfileDialog.new()


func bind(torqa: TorqaApp) -> void:
	_torqa = torqa
	refresh()


## Lists the riders with the active one selected and shows its figures.
func refresh() -> void:
	var active: Dictionary = _torqa.profile()
	var language: String = active.get("language", "")
	apply_language(language)
	var active_id: String = active.get("id", "")
	_riders.clear()
	for profile: Dictionary in _torqa.profiles():
		var id: String = profile["id"]
		var profile_name: String = profile["name"]
		_riders.add_item(profile_name)
		_riders.set_item_metadata(_riders.item_count - 1, id)
		if id == active_id:
			_riders.select(_riders.item_count - 1)
	for child: Node in _badge_slot.get_children():
		child.queue_free()
	var rider_name: String = active.get("name", "")
	_badge_slot.add_child(UiTheme.initial(rider_name, 48))
	_name_label.text = rider_name
	_show_summary(active)


## Switches the interface to a rider's language; "" follows the system.
static func apply_language(code: String) -> void:
	var locale: String = code if not code.is_empty() else OS.get_locale_language()
	if TranslationServer.get_locale() != locale:
		TranslationServer.set_locale(locale)


func _init() -> void:
	add_theme_constant_override("separation", 16)
	# The rider as a heading: an initial in a badge and the name, the list of riders and the
	# pencil and plus beside it (#191).
	var row: HBoxContainer = HBoxContainer.new()
	row.add_theme_constant_override("separation", 12)
	_badge_slot.add_child(UiTheme.initial("", 48))
	row.add_child(_badge_slot)
	var titles: VBoxContainer = VBoxContainer.new()
	titles.add_theme_constant_override("separation", 0)
	titles.add_child(UiTheme.caption(tr("Rider")))
	# Rider names are never translated.
	_name_label.auto_translate_mode = Node.AUTO_TRANSLATE_MODE_DISABLED
	_name_label.add_theme_font_size_override("font_size", 22)
	titles.add_child(_name_label)
	titles.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	row.add_child(titles)
	_riders.auto_translate_mode = Node.AUTO_TRANSLATE_MODE_DISABLED
	_riders.custom_minimum_size = Vector2(240, 0)
	_riders.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	_riders.tooltip_text = tr("Switch the rider")
	_riders.item_selected.connect(_on_rider_selected)
	row.add_child(_riders)
	_icon_button(_edit_button, "pencil", tr("Edit…"))
	_edit_button.pressed.connect(
		func() -> void: _dialog.edit(_torqa.profile(), _torqa.hud_layout())
	)
	row.add_child(_edit_button)
	_icon_button(_add_button, "plus", tr("New rider…"))
	_add_button.pressed.connect(func() -> void: _dialog.edit({}, TorqaApp.hud_default_layout()))
	row.add_child(_add_button)
	add_child(row)
	var card: PanelContainer = PanelContainer.new()
	var rows: VBoxContainer = VBoxContainer.new()
	rows.add_theme_constant_override("separation", 12)
	_summary.columns = 2
	_summary.add_theme_constant_override("h_separation", 24)
	_summary.add_theme_constant_override("v_separation", 8)
	rows.add_child(_summary)
	_more.focus_mode = Control.FOCUS_NONE
	_more.size_flags_horizontal = Control.SIZE_SHRINK_END
	_more.pressed.connect(func() -> void: expanded = not expanded)
	rows.add_child(_more)
	card.add_child(rows)
	add_child(card)
	add_child(_dialog)
	expanded = false
	_dialog.profile_confirmed.connect(_on_profile_confirmed)


func _on_rider_selected(index: int) -> void:
	var id: String = _riders.get_item_metadata(index)
	_torqa.select_profile(id)
	refresh()
	profile_changed.emit()


func _on_profile_confirmed(id: String, profile: Dictionary, hud_layout: PackedStringArray) -> void:
	# Saving makes the rider active, so the layout goes to the right rider.
	if not _torqa.save_profile(id, profile).is_empty():
		_torqa.set_hud_layout(hud_layout)
	refresh()
	profile_changed.emit()


func _show_summary(profile: Dictionary) -> void:
	# Freed at once: the rows are rebuilt below and must not count twice meanwhile.
	for child: Node in _summary.get_children():
		_summary.remove_child(child)
		child.free()
	var imperial: bool = profile.get("units", "metric") == "imperial"
	var weight: float = profile.get("rider_mass_kg", 0.0)
	var bike: float = profile.get("bike_mass_kg", 0.0)
	var ftp: float = profile.get("ftp_w", 0.0)
	var max_hr: float = profile.get("max_heart_rate_bpm", 0.0)
	var mass: String = "%.1f lb" if imperial else "%.1f kg"
	var factor: float = 2.20462 if imperial else 1.0
	# i18n-begin
	var rows: Array[Array] = [
		["Weight", mass % (weight * factor)],
		["Bike weight", mass % (bike * factor)],
		["FTP", "%d W  ·  %.1f W/kg" % [roundi(ftp), ftp / maxf(weight, 1.0)]],
		["Max heart rate", "%d bpm" % roundi(max_hr)],
		["Units", "Imperial (mi, lb)" if imperial else "Metric (km, kg)"],
		["Rider on the bike", "Male rider" if profile.get("avatar") == "male" else "Female rider"],
	]
	# i18n-end
	if expanded:
		rows.append_array(_more_rows(profile))
	for row: Array in rows:
		var caption: String = row[0]
		_summary.add_child(UiTheme.caption(caption))
		var value: Label = Label.new()
		var text: String = row[1]
		value.text = tr(text)
		_summary.add_child(value)


static func _icon_button(button: Button, icon: String, tooltip: String) -> void:
	button.icon = UiIcons.texture(icon)
	button.tooltip_text = tooltip
	button.focus_mode = Control.FOCUS_NONE
	button.size_flags_vertical = Control.SIZE_SHRINK_CENTER


## The rest of what the dialog has (#194): name, language, drivetrain and the HUD's layout.
func _more_rows(profile: Dictionary) -> Array[Array]:
	var language: String = tr("System language")
	var chosen: String = profile.get("language", "")
	for entry: Array in ProfileDialog.LANGUAGES:
		var code: String = entry[0]
		var language_name: String = entry[1]
		if code == chosen and not code.is_empty():
			language = language_name
	var single_cog: bool = profile.get("drivetrain", "cassette") == "single_cog"
	var captions: Dictionary[String, String] = {}
	for metric: Dictionary in TorqaApp.hud_metrics():
		var id: String = metric["id"]
		var caption: String = metric["caption"]
		captions[id] = tr(caption)
	var hud: PackedStringArray = PackedStringArray()
	var layout: PackedStringArray = (
		_torqa.hud_layout() if _torqa != null else TorqaApp.hud_default_layout()
	)
	for id: String in layout:
		var shown: String = captions.get(id, id)
		hud.append(shown)
	# i18n-begin
	var rows: Array[Array] = [
		["Name", profile.get("name", "")],
		["Language", language],
		[
			"Drivetrain",
			"Single cog: virtual gears" if single_cog else "Cassette: shift on the bike"
		],
	]
	# i18n-end
	if single_cog:
		var chainring: int = profile.get("chainring", 50)
		var cog: int = profile.get("cog", 14)
		rows.append(["Chainring", "%d T" % chainring])
		rows.append(["Cog", "%d T" % cog])
	rows.append(["HUD", " · ".join(hud)])
	return rows
