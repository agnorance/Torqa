class_name StartPage
extends Control
## The start page (R38): tabs for Courses, Workouts (R58), History, Profile and Devices &
## Settings. The ride view is separate; riding starts from a course's detail page or the
## Workouts tab.

## A ride has started with `options` (`RideOptions.options()`); a workout's also carry
## `workout` (`WorkoutsTab.workout()`) and `on_course`.
signal ride_started(options: Dictionary)

enum Tab { COURSES, WORKOUTS, HISTORY, PROFILE, DEVICES }

var _torqa: TorqaApp
var _tabs: TabContainer = TabContainer.new()
var _courses_page: Control = Control.new()
var _courses: CoursesTab = CoursesTab.new()
var _detail: CourseDetail = CourseDetail.new()
var _workouts: WorkoutsTab = WorkoutsTab.new()
var _history: HistoryScreen = (
	(preload("res://scenes/history_screen.tscn") as PackedScene).instantiate() as HistoryScreen
)
var _profile: ProfileTab = ProfileTab.new()
var _devices: DevicesTab = DevicesTab.new()
## The options and ghost of the ride waiting for its world.
var _pending_ride: Array = []
## The workout waiting for its course's world, and whether it has one.
var _pending_workout: Dictionary = {}
var _workout_on_course: bool = false


func bind(torqa: TorqaApp) -> void:
	_torqa = torqa
	_courses.bind(torqa)
	_detail.bind(torqa)
	_workouts.bind(torqa)
	_history.bind(torqa)
	_profile.bind(torqa)
	_devices.bind(torqa)


## Back from a ride: records, history and courses may have changed.
func refresh() -> void:
	_courses.refresh()
	_workouts.refresh()
	_detail.refresh_records()
	if _tabs.current_tab == Tab.HISTORY:
		_history.open()


## The ride options of the course being ridden, e.g. to show them in the ride.
func ride_options() -> Dictionary:
	return _detail.ride_options()


func _ready() -> void:
	(%Rows as VBoxContainer).add_child(_tabs)
	_tabs.size_flags_vertical = Control.SIZE_EXPAND_FILL
	# i18n-begin
	for page: Array in [
		[_courses_page, "Courses"],
		[_workouts, "Workouts"],
		[_history, "History"],
		[_profile, "Profile"],
		[_devices, "Devices & Settings"],
	]:
		# i18n-end
		var node: Control = page[0]
		var title: String = page[1]
		node.name = title
		_tabs.add_child(node)
		_tabs.set_tab_title(_tabs.get_tab_count() - 1, tr(title))
	_courses.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	_courses_page.add_child(_courses)
	_detail.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	_detail.hide()
	_courses_page.add_child(_detail)
	_history.embedded = true
	_courses.course_opened.connect(_open_course)
	_detail.back_requested.connect(_show_gallery)
	_detail.ride_requested.connect(_on_ride_requested)
	_detail.ready_to_ride.connect(_start_ride)
	_detail.course_changed.connect(_courses.refresh)
	_workouts.start_requested.connect(_on_workout_requested)
	_workouts.ready_to_start.connect(_start_workout)
	_profile.profile_changed.connect(_courses.refresh)
	_tabs.tab_changed.connect(_on_tab_changed)
	# The course page sits over the gallery in its tab: a click on the Courses tab, already
	# the current one, brings the gallery back like the page's own button does (#188).
	_tabs.get_tab_bar().tab_clicked.connect(_on_tab_clicked)


func _on_tab_clicked(tab: int) -> void:
	if tab == Tab.COURSES and _detail.visible:
		_show_gallery()


func _open_course(course: Dictionary) -> void:
	_courses.hide()
	_detail.show()
	_detail.open(course)


func _show_gallery() -> void:
	_detail.hide()
	_courses.show()


func _on_tab_changed(tab: int) -> void:
	if tab == Tab.HISTORY:
		_history.open()
	elif tab == Tab.WORKOUTS:
		# Courses and riders change on the other tabs.
		_workouts.refresh()


## Riding needs a trainer: without one, the Devices tab says what to do (R41).
func _on_ride_requested(options: Dictionary, ghost: Dictionary) -> void:
	_detail.show_status("")
	if not _devices.connect_selected():
		_detail.show_status(tr("No trainer connected — choose one under Devices & Settings."))
		_tabs.current_tab = Tab.DEVICES
		return
	_pending_ride = [options, ghost]
	_detail.build()


func _on_workout_requested(workout: Dictionary, course_path: String) -> void:
	if not _devices.connect_selected():
		_workouts.show_status(tr("No trainer connected — choose one under Devices & Settings."))
		_tabs.current_tab = Tab.DEVICES
		return
	_pending_workout = workout
	_workout_on_course = not course_path.is_empty()
	_workouts.prepare()


func _start_workout() -> void:
	if _pending_workout.is_empty():
		return
	var workout: Dictionary = _pending_workout
	_pending_workout = {}
	# On a course, its world looks as last chosen on the course page.
	var options: Dictionary = _detail.ride_options()
	var flat_descents: bool = options["flat_descents"]
	if _torqa.start_workout(workout, _workout_on_course, flat_descents):
		options["workout"] = workout
		options["on_course"] = _workout_on_course
		options["overlay"] = _workouts.start_as_overlay()
		ride_started.emit(options)


func _start_ride() -> void:
	if _pending_ride.is_empty():
		return
	var options: Dictionary = _pending_ride[0]
	var ghost: Dictionary = _pending_ride[1]
	_pending_ride = []
	var difficulty: float = options["difficulty"]
	var flat_descents: bool = options["flat_descents"]
	if _torqa.start_ride(difficulty, flat_descents, ghost):
		ride_started.emit(options)
