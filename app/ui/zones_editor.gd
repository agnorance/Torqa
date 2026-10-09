class_name ZonesEditor
extends VBoxContainer
## The rider's training zones (#173): power zones 1–7 as shares of FTP and heart-rate zones
## 1–5 as shares of the maximum heart rate, the upper bound of each adjustable, with the watts
## or beats it comes to beside it. Bounds keep their order: moving one pushes its neighbours.

## The standard zones: Coggan's power zones and the usual heart-rate zones, in percent.
const DEFAULT_POWER: PackedFloat64Array = [55.0, 75.0, 90.0, 105.0, 120.0, 150.0]
const DEFAULT_HEART: PackedFloat64Array = [60.0, 70.0, 80.0, 90.0]
## Heart-rate zone 1 starts here (`Profile::heart_rate_zone_range`).
const HEART_FLOOR_PCT: float = 50.0

var _ftp_w: float = 200.0
var _max_hr: float = 185.0
var _power: Array[SpinBox] = []
var _heart: Array[SpinBox] = []
var _power_values: Array[Label] = []
var _heart_values: Array[Label] = []


func _init() -> void:
	add_theme_constant_override("separation", 12)
	add_child(UiTheme.caption(tr("Power zones, the top of each as a share of FTP")))
	add_child(_table(UiTheme.POWER_ZONES, _power, _power_values, DEFAULT_POWER))
	add_child(
		UiTheme.caption(
			tr("Heart-rate zones, the top of each as a share of the maximum heart rate")
		)
	)
	add_child(_table(UiTheme.HEART_RATE_ZONES, _heart, _heart_values, DEFAULT_HEART))
	var reset: Button = Button.new()
	reset.text = tr("Standard zones")
	reset.tooltip_text = tr("Coggan's power zones and the usual heart-rate zones")
	reset.size_flags_horizontal = Control.SIZE_SHRINK_BEGIN
	reset.pressed.connect(func() -> void: edit(DEFAULT_POWER, DEFAULT_HEART, _ftp_w, _max_hr))
	add_child(reset)


## Shows the rider's zone bounds (percent) for their `ftp_w` and `max_hr`.
func edit(
	power_pct: PackedFloat64Array, heart_pct: PackedFloat64Array, ftp_w: float, max_hr: float
) -> void:
	_ftp_w = ftp_w
	_max_hr = max_hr
	_fill(_power, power_pct if power_pct.size() == _power.size() else DEFAULT_POWER)
	_fill(_heart, heart_pct if heart_pct.size() == _heart.size() else DEFAULT_HEART)
	_show_values()


## FTP or maximum heart rate changed on the profile tab: the watts and beats follow.
func set_bases(ftp_w: float, max_hr: float) -> void:
	_ftp_w = ftp_w
	_max_hr = max_hr
	_show_values()


func power_pct() -> PackedFloat64Array:
	return _bounds(_power)


func heart_pct() -> PackedFloat64Array:
	return _bounds(_heart)


func _table(
	zones: Array[Array], spins: Array[SpinBox], values: Array[Label], defaults: PackedFloat64Array
) -> GridContainer:
	var grid: GridContainer = GridContainer.new()
	grid.columns = 3
	grid.add_theme_constant_override("h_separation", 16)
	grid.add_theme_constant_override("v_separation", 6)
	for i: int in range(zones.size()):
		var zone_name: String = zones[i][0]
		var color: Color = zones[i][1]
		var caption: Label = Label.new()
		caption.text = "Z%d %s" % [i + 1, tr(zone_name)]
		caption.add_theme_color_override("font_color", color)
		caption.custom_minimum_size = Vector2(170, 0)
		grid.add_child(caption)
		if i < defaults.size():
			var spin: SpinBox = SpinBox.new()
			spin.min_value = 1.0
			spin.max_value = 300.0
			spin.step = 1.0
			spin.suffix = " %"
			spin.value = defaults[i]
			spin.custom_minimum_size = Vector2(120, 0)
			spin.value_changed.connect(func(_value: float) -> void: _on_bound(spins, i))
			spins.append(spin)
			grid.add_child(spin)
		else:
			var open: Label = Label.new()
			open.text = tr("and above")
			open.add_theme_color_override("font_color", UiTheme.MUTED)
			grid.add_child(open)
		var value: Label = Label.new()
		value.add_theme_color_override("font_color", UiTheme.MUTED)
		values.append(value)
		grid.add_child(value)
	return grid


## A bound moved: those before stay below it, those after above it.
func _on_bound(spins: Array[SpinBox], index: int) -> void:
	var value: float = spins[index].value
	for j: int in range(index):
		if spins[j].value >= value - float(index - j) + 1.0:
			spins[j].set_value_no_signal(value - float(index - j))
	for j: int in range(index + 1, spins.size()):
		if spins[j].value <= value + float(j - index) - 1.0:
			spins[j].set_value_no_signal(value + float(j - index))
	_show_values()


func _fill(spins: Array[SpinBox], pct: PackedFloat64Array) -> void:
	for i: int in range(spins.size()):
		spins[i].set_value_no_signal(pct[i])


func _bounds(spins: Array[SpinBox]) -> PackedFloat64Array:
	var out: PackedFloat64Array = []
	for spin: SpinBox in spins:
		out.append(spin.value)
	return out


func _show_values() -> void:
	_show(_power, _power_values, _ftp_w, 0.0, "W")
	_show(_heart, _heart_values, _max_hr, HEART_FLOOR_PCT, "bpm")


## Each zone's range in `unit`: from the bound before (or `floor_pct`) to its own, the last
## open.
func _show(
	spins: Array[SpinBox], values: Array[Label], base: float, floor_pct: float, unit: String
) -> void:
	var low: float = floor_pct
	for i: int in range(values.size()):
		if i < spins.size():
			var high: float = spins[i].value
			values[i].text = (
				"%d–%d %s" % [roundi(low * base / 100.0), roundi(high * base / 100.0), unit]
			)
			low = high
		else:
			values[i].text = "> %d %s" % [roundi(low * base / 100.0), unit]
