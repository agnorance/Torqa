class_name CoursesTab
extends VBoxContainer
## The course library as a gallery (R39). Importing a GPX, a GoPro video with GPS or an
## Incyclist route video (R17) prepares a new course here: it is built, added to the library
## and opened. A video without GPS cannot become a course by itself: a dialog explains how to
## add it to the course of its GPX route instead.

## Open the detail page of `course` (from `TorqaApp.courses()`).
signal course_opened(course: Dictionary)

const STEP_WEIGHTS: Dictionary[String, Vector2] = {
	"Reading route": Vector2(0.0, 0.02),
	"Downloading map data": Vector2(0.02, 0.6),
	"Correcting elevations": Vector2(0.6, 0.8),
	"Building 3D world": Vector2(0.8, 1.0),
}

var _torqa: TorqaApp
var _gallery: HFlowContainer = HFlowContainer.new()
var _empty: Label = Label.new()
var _import_button: Button = Button.new()
var _loading: HBoxContainer = HBoxContainer.new()
var _loading_bar: ProgressBar = ProgressBar.new()
var _loading_label: Label = Label.new()
var _status: Label = Label.new()
var _file_dialog: FileDialog = FileDialog.new()
var _no_gps_dialog: AcceptDialog = AcceptDialog.new()
## Every import is named by the rider; a name in use asks before replacing that course (#40).
var _name_dialog: ConfirmationDialog = ConfirmationDialog.new()
var _name_edit: LineEdit = LineEdit.new()
var _replace_dialog: ConfirmationDialog = ConfirmationDialog.new()
## The file waiting for its course name.
var _pending: String = ""
## What the running import is: "" none, "gpx" a route or "video" a video course being
## prepared, "tqc" a course file, "probe" a video being looked at.
var _importing: String = ""


func bind(torqa: TorqaApp) -> void:
	_torqa = torqa
	_torqa.loading_progress.connect(_on_loading_progress)
	_torqa.course_added.connect(_on_course_added)
	_torqa.failed.connect(_on_failed)
	refresh()


## Rebuilds the gallery from the library.
func refresh() -> void:
	for child: Node in _gallery.get_children():
		child.queue_free()
	var imperial: bool = _torqa.profile().get("units", "metric") == "imperial"
	var courses: Array = _torqa.courses()
	for course: Dictionary in courses:
		var card: CourseCard = CourseCard.new(course, imperial)
		var course_path: String = course["path"]
		card.set_map(_torqa.course_preview(course_path))
		card.pressed.connect(func() -> void: course_opened.emit(course))
		_gallery.add_child(card)
	_empty.visible = courses.is_empty()


func _init() -> void:
	add_theme_constant_override("separation", 16)
	var header: HBoxContainer = HBoxContainer.new()
	var heading: Label = Label.new()
	heading.text = tr("Your courses")
	heading.add_theme_font_size_override("font_size", 22)
	heading.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	header.add_child(heading)
	_import_button.text = tr("Import")
	_import_button.tooltip_text = tr("GPX routes, videos with GPS, route videos and course files")
	_import_button.custom_minimum_size = Vector2(160, 44)
	_import_button.pressed.connect(func() -> void: _file_dialog.popup_centered_ratio(0.7))
	header.add_child(_import_button)
	add_child(header)

	_loading.add_theme_constant_override("separation", 16)
	_loading_bar.custom_minimum_size = Vector2(360, 20)
	_loading_bar.max_value = 1.0
	_loading_bar.show_percentage = false
	_loading_bar.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	_loading.add_child(_loading_bar)
	_loading.add_child(_loading_label)
	_loading.hide()
	add_child(_loading)
	_status.add_theme_color_override("font_color", Color(1, 0.55, 0.45))
	_status.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_status.hide()
	add_child(_status)

	_empty.text = tr("No courses yet. Import a GPX route or a video to prepare your first course.")
	_empty.add_theme_color_override("font_color", UiTheme.MUTED)
	add_child(_empty)
	var scroll: ScrollContainer = ScrollContainer.new()
	scroll.size_flags_vertical = Control.SIZE_EXPAND_FILL
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	_gallery.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_gallery.add_theme_constant_override("h_separation", 20)
	_gallery.add_theme_constant_override("v_separation", 20)
	scroll.add_child(_gallery)
	add_child(scroll)

	_file_dialog.title = tr("Open a GPX route, video or Torqa course")
	_file_dialog.file_mode = FileDialog.FILE_MODE_OPEN_FILE
	_file_dialog.access = FileDialog.ACCESS_FILESYSTEM
	var videos: PackedStringArray = PackedStringArray()
	for extension: String in TorqaApp.video_extensions():
		videos.append("*." + extension)
	_file_dialog.filters = PackedStringArray(
		[
			"*.gpx, *.tqc, %s ; %s" % [", ".join(videos), tr("Routes, videos and courses")],
			"*.gpx ; " + tr("GPX routes"),
			(
				"%s ; %s"
				% [
					", ".join(videos),
					tr("Videos with GPS, route videos (Incyclist .xml, Tacx .rlv)")
				]
			),
			"*.tqc ; " + tr("Torqa courses"),
		]
	)
	_file_dialog.use_native_dialog = true
	_file_dialog.file_selected.connect(_on_file_selected)
	add_child(_file_dialog)
	_name_dialog.title = tr("Name the course")
	_name_dialog.ok_button_text = tr("Import")
	_name_edit.custom_minimum_size = Vector2(420, 0)
	_name_edit.auto_translate_mode = Node.AUTO_TRANSLATE_MODE_DISABLED
	_name_dialog.add_child(_name_edit)
	_name_dialog.register_text_enter(_name_edit)
	_name_dialog.confirmed.connect(_on_name_confirmed)
	add_child(_name_dialog)
	_replace_dialog.title = tr("Course name in use")
	_replace_dialog.ok_button_text = tr("Replace")
	_replace_dialog.add_button(tr("Keep both"), false, "keep")
	_replace_dialog.confirmed.connect(func() -> void: _import(true))
	_replace_dialog.custom_action.connect(_on_replace_action)
	_replace_dialog.canceled.connect(func() -> void: _name_dialog.popup_centered())
	add_child(_replace_dialog)
	_no_gps_dialog.title = tr("Video without GPS")
	_no_gps_dialog.dialog_autowrap = true
	_no_gps_dialog.min_size = Vector2i(560, 0)
	add_child(_no_gps_dialog)


func _on_file_selected(path: String) -> void:
	_status.hide()
	var extension: String = path.get_extension().to_lower()
	if not extension in ["tqc", "gpx", "xml", "rlv"]:
		# Set so that a video that cannot be read is reported like a failed import.
		_importing = "probe"
		var probe: Dictionary = _torqa.video_probe(path)
		if probe.is_empty():
			return
		_importing = ""
		if not probe["has_gps"]:
			_no_gps_dialog.dialog_text = no_gps_steps(path.get_file())
			_no_gps_dialog.popup_centered()
			return
	_pending = path
	_name_edit.text = TorqaApp.suggested_course_name(path)
	_name_dialog.popup_centered()
	_name_edit.grab_focus()
	_name_edit.select_all()


func _on_name_confirmed() -> void:
	var name: String = _name_edit.text.strip_edges()
	if name.is_empty():
		_name_dialog.popup_centered.call_deferred()
		return
	if _torqa.course_exists(name):
		_replace_dialog.dialog_text = (
			tr("A course named “%s” exists already. Replace it, or keep both?") % name
		)
		_replace_dialog.popup_centered()
	else:
		_import(false)


func _on_replace_action(action: StringName) -> void:
	_replace_dialog.hide()
	if action == &"keep":
		_import(false)


## Imports the pending file as the course named in the name dialog.
func _import(replace: bool) -> void:
	_torqa.name_next_import(_name_edit.text.strip_edges(), replace)
	var path: String = _pending
	match path.get_extension().to_lower():
		"tqc":
			_importing = "tqc"
			_torqa.import_course(path)
		"gpx":
			_start_loading("gpx", path)
			_torqa.load_route(path, false)
		_:
			_start_loading("video", path)
			_torqa.load_video(path, false)


## What to do with a video without GPS: it rides on the course of its GPX route.
func no_gps_steps(file: String) -> String:
	var steps: PackedStringArray = [
		tr("%s has no GPS, so it cannot become a course by itself.") % file,
		tr("1. Import the GPX route of the ride shown in the video."),
		tr("2. Open that course."),
		tr("3. Press Add video…, choose the video and set where the route starts and ends."),
	]
	return "\n\n".join(steps)


func _start_loading(kind: String, path: String) -> void:
	_importing = kind
	_import_button.disabled = true
	_loading_bar.value = 0.0
	_loading_label.text = tr("Loading %s …") % path.get_file()
	_loading.show()


func _on_loading_progress(step: String, unit: String, done: int, total: int) -> void:
	if _importing != "gpx" and _importing != "video":
		return
	var share: Vector2 = STEP_WEIGHTS.get(step, Vector2(0.0, 1.0))
	_loading_bar.value = lerpf(share.x, share.y, float(done) / float(maxi(total, 1)))
	_loading_label.text = "%s… %d / %d %s" % [tr(step), done, total, tr(unit)]


## A prepared route (or an imported course file) is in the library: show it and open it.
func _on_course_added(path: String) -> void:
	var importing: String = _importing
	_importing = ""
	_loading.hide()
	_import_button.disabled = false
	# The course's map picture for its card, from the world just built (#192).
	if _torqa.course_preview(path).is_empty():
		var png: PackedByteArray = await MapPreview.capture(
			get_tree(), _torqa.minimap_mesh(), _torqa.track(2000)
		)
		if not png.is_empty():
			_torqa.set_course_preview(path, png)
	refresh()
	if importing.is_empty():
		return
	for course: Dictionary in _torqa.courses():
		if course["path"] == path:
			course_opened.emit(course)


func _on_failed(message: String) -> void:
	if _importing.is_empty():
		return
	_importing = ""
	_loading.hide()
	_import_button.disabled = false
	_status.text = message
	_status.show()
