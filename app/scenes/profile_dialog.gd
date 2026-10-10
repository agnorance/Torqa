class_name ProfileDialog
extends ConfirmationDialog
## Rider settings: the profile (fields mirror `TorqaApp.profile()`) and the rider's HUD layout
## (R51). Saving is left to the caller.

## The edited profile, shaped like `TorqaApp.profile()`, and HUD layout; `id` is empty for a new
## rider.
signal profile_confirmed(id: String, profile: Dictionary, hud_layout: PackedStringArray)

const UNITS: Array[String] = ["metric", "imperial"]
const DRIVETRAINS: Array[String] = ["cassette", "single_cog"]
## Interface languages: locale code and name in that language; "" follows the system.
const LANGUAGES: Array[Array] = [["", "System language"], ["en", "English"], ["de", "Deutsch"]]

var _id: String = ""
var _name_edit: LineEdit = LineEdit.new()
var _rider_mass: SpinBox = _spin(30.0, 200.0, 0.5, " kg")
var _bike_mass: SpinBox = _spin(3.0, 40.0, 0.1, " kg")
var _ftp: SpinBox = _spin(50.0, 600.0, 1.0, " W")
var _max_heart_rate: SpinBox = _spin(100.0, 230.0, 1.0, " bpm")
var _units: OptionButton = OptionButton.new()
var _language: OptionButton = OptionButton.new()
var _avatar: OptionButton = OptionButton.new()
## A cassette, or a single cog with virtual gears (R9): its chainring and cog then.
var _drivetrain: OptionButton = OptionButton.new()
var _chainring: SpinBox = _spin(20.0, 60.0, 1.0, " T")
var _cog: SpinBox = _spin(9.0, 36.0, 1.0, " T")
var _teeth_rows: Array[Control] = []
var _hud: HudEditor = HudEditor.new()
var _badge_slot: HBoxContainer = HBoxContainer.new()
var _zones: ZonesEditor = ZonesEditor.new()


func _ready() -> void:
	theme = UiTheme.build()
	title = tr("Rider settings")
	ok_button_text = tr("Save")
	min_size = Vector2i(720, 460)
	var tabs: TabContainer = TabContainer.new()
	# The rider as a heading, an initial in a badge beside the name, then the figures in a
	# card (#191).
	var page: VBoxContainer = VBoxContainer.new()
	page.name = tr("Profile")
	page.add_theme_constant_override("separation", 16)
	var header: HBoxContainer = HBoxContainer.new()
	header.add_theme_constant_override("separation", 12)
	_badge_slot.add_child(UiTheme.initial("", 48))
	header.add_child(_badge_slot)
	_name_edit.placeholder_text = tr("Name")
	_name_edit.add_theme_font_size_override("font_size", 22)
	_name_edit.text_changed.connect(_show_initial)
	header.add_child(_name_edit)
	page.add_child(header)
	var card: PanelContainer = PanelContainer.new()
	var grid: GridContainer = GridContainer.new()
	grid.columns = 2
	grid.add_theme_constant_override("h_separation", 24)
	grid.add_theme_constant_override("v_separation", 12)
	_name_edit.custom_minimum_size = Vector2(260, 0)
	# The fields grow with the dialog rather than staying fixed in the middle (R53).
	for field: Control in [
		_name_edit,
		_rider_mass,
		_bike_mass,
		_ftp,
		_max_heart_rate,
		_units,
		_language,
		_avatar,
		_drivetrain,
		_chainring,
		_cog,
	]:
		field.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_units.add_item(tr("Metric (km, kg)"))
	_units.add_item(tr("Imperial (mi, lb)"))
	# In the order of RiderAvatar.RIDERS.
	_avatar.add_item(tr("Female rider"))
	_avatar.add_item(tr("Male rider"))
	# In the order of DRIVETRAINS.
	_drivetrain.add_item(tr("Cassette: shift on the bike"))
	_drivetrain.add_item(tr("Single cog: virtual gears"))
	_drivetrain.tooltip_text = tr(
		"On a single cog such as the Zwift Cog, Torqa shifts 24 virtual gears: ↑ and ↓ while riding"
	)
	_drivetrain.item_selected.connect(func(_index: int) -> void: _show_teeth())
	# Language names stay in their own language; only "System language" is translated.
	_language.auto_translate_mode = Node.AUTO_TRANSLATE_MODE_DISABLED
	for language: Array in LANGUAGES:
		var code: String = language[0]
		var language_name: String = language[1]
		_language.add_item(tr(language_name) if code.is_empty() else language_name)
	# i18n-begin
	for row: Array in [
		["Weight", _rider_mass],
		["Bike weight", _bike_mass],
		["FTP", _ftp],
		["Max heart rate", _max_heart_rate],
		["Units", _units],
		["Language", _language],
		["Rider on the bike", _avatar],
		["Drivetrain", _drivetrain],
		["Chainring", _chainring],
		["Cog", _cog],
	]:
		# i18n-end
		var caption: Label = Label.new()
		caption.text = row[0]
		var field: Control = row[1]
		grid.add_child(caption)
		grid.add_child(field)
		if field in [_chainring, _cog]:
			_teeth_rows.append_array([caption, field])
	card.add_child(grid)
	page.add_child(card)
	tabs.add_child(page)
	tabs.add_child(grid)
	_zones.name = tr("Zones")
	tabs.add_child(_zones)
	_hud.name = tr("HUD")
	tabs.add_child(_hud)
	add_child(tabs)
	for base: SpinBox in [_ftp, _max_heart_rate]:
		base.value_changed.connect(
			func(_value: float) -> void: _zones.set_bases(_ftp.value, _max_heart_rate.value)
		)
	_units.item_selected.connect(
		func(index: int) -> void: _hud.edit(_hud.layout(), UNITS[index] == "imperial")
	)
	confirmed.connect(_on_confirmed)


## Opens the dialog for `profile` (shaped like `TorqaApp.profile()`) with its `hud_layout`, or
## for a new rider if `profile` has no id.
func edit(profile: Dictionary, hud_layout: PackedStringArray) -> void:
	_id = profile.get("id", "")
	_name_edit.text = profile.get("name", "")
	_show_initial(_name_edit.text)
	_rider_mass.value = profile.get("rider_mass_kg", 75.0)
	_bike_mass.value = profile.get("bike_mass_kg", 8.0)
	_ftp.value = profile.get("ftp_w", 200.0)
	_max_heart_rate.value = profile.get("max_heart_rate_bpm", 185.0)
	_units.select(maxi(UNITS.find(profile.get("units", "metric")), 0))
	_language.select(0)
	for i: int in range(LANGUAGES.size()):
		if LANGUAGES[i][0] == profile.get("language", ""):
			_language.select(i)
	var avatar: String = profile.get("avatar", RiderAvatar.RIDERS[0])
	_avatar.select(maxi(RiderAvatar.RIDERS.find(avatar), 0))
	var drivetrain: String = profile.get("drivetrain", "cassette")
	_drivetrain.select(maxi(DRIVETRAINS.find(drivetrain), 0))
	_chainring.value = profile.get("chainring", 50)
	_cog.value = profile.get("cog", 14)
	_show_teeth()
	var power_zones: PackedFloat64Array = profile.get("power_zones_pct", ZonesEditor.DEFAULT_POWER)
	var heart_zones: PackedFloat64Array = profile.get(
		"heart_rate_zones_pct", ZonesEditor.DEFAULT_HEART
	)
	_zones.edit(power_zones, heart_zones, _ftp.value, _max_heart_rate.value)
	_hud.edit(hud_layout, UNITS[_units.selected] == "imperial")
	title = tr("New rider") if _id.is_empty() else tr("Rider settings")
	popup_centered(Vector2i(960, 600))
	_name_edit.grab_focus()


func _on_confirmed() -> void:
	var profile_name: String = _name_edit.text.strip_edges()
	(
		profile_confirmed
		. emit(
			_id,
			{
				"name": profile_name if not profile_name.is_empty() else tr("Rider"),
				"rider_mass_kg": _rider_mass.value,
				"bike_mass_kg": _bike_mass.value,
				"ftp_w": _ftp.value,
				"max_heart_rate_bpm": _max_heart_rate.value,
				"units": UNITS[_units.selected],
				"language": LANGUAGES[_language.selected][0],
				"avatar": RiderAvatar.RIDERS[_avatar.selected],
				"drivetrain": DRIVETRAINS[_drivetrain.selected],
				"chainring": _chainring.value,
				"cog": _cog.value,
				"power_zones_pct": _zones.power_pct(),
				"heart_rate_zones_pct": _zones.heart_pct(),
			},
			_hud.layout()
		)
	)


func _show_initial(rider_name: String) -> void:
	for child: Node in _badge_slot.get_children():
		child.queue_free()
	_badge_slot.add_child(UiTheme.initial(rider_name, 48))


func _show_teeth() -> void:
	for control: Control in _teeth_rows:
		control.visible = DRIVETRAINS[_drivetrain.selected] == "single_cog"


static func _spin(low: float, high: float, step: float, suffix: String) -> SpinBox:
	var spin: SpinBox = SpinBox.new()
	spin.min_value = low
	spin.max_value = high
	spin.step = step
	spin.suffix = suffix
	return spin
