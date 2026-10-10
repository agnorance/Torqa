class_name EditableTitle
extends HBoxContainer
## A large title that can be renamed in place (R50): it reads like a heading, and the pencil
## beside it (or a click on it) turns it into a text field. Enter or leaving the field ends
## editing.

## Editing ended; `text` holds what was typed.
signal edit_finished

## The title shown; empty shows `placeholder_text`.
var text: String:
	get:
		return _edit.text
	set(value):
		_edit.text = value
var placeholder_text: String:
	get:
		return _edit.placeholder_text
	set(value):
		_edit.placeholder_text = value

var _edit: LineEdit = LineEdit.new()
var _button: Button = Button.new()


func _init(rename_tooltip: String = "") -> void:
	add_theme_constant_override("separation", 8)
	_edit.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_edit.auto_translate_mode = Node.AUTO_TRANSLATE_MODE_DISABLED
	_edit.add_theme_font_size_override("font_size", 26)
	_edit.add_theme_stylebox_override("normal", StyleBoxEmpty.new())
	_edit.tooltip_text = rename_tooltip
	_edit.text_submitted.connect(func(_text: String) -> void: _edit.release_focus())
	_edit.focus_exited.connect(func() -> void: edit_finished.emit())
	add_child(_edit)
	_button.icon = UiIcons.texture("pencil")
	_button.tooltip_text = rename_tooltip
	_button.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	_button.focus_mode = Control.FOCUS_NONE
	_button.pressed.connect(start_editing)
	add_child(_button)


## Puts the cursor in the title with all of it selected, ready to type a new one.
func start_editing() -> void:
	_edit.grab_focus()
	_edit.select_all()
