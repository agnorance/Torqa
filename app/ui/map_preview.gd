class_name MapPreview
extends SubViewport
## Draws a course's map with its route on it as the gallery's cards show it (#192): the flat
## map of the ride's minimap under the path in the logo blue. Captured once when a course is
## prepared and kept in its file, so the gallery needs no map data.

const SIZE: Vector2i = Vector2i(536, 320)
const PADDING: float = 28.0

var _canvas: Control = Control.new()
var _mesh: ArrayMesh
var _background: Color = Color(0.15, 0.17, 0.16)
var _track: PackedVector2Array = PackedVector2Array()


func _init() -> void:
	size = SIZE
	render_target_update_mode = SubViewport.UPDATE_ALWAYS
	_canvas.size = Vector2(SIZE)
	_canvas.draw.connect(_draw_map)
	add_child(_canvas)


## Draws `map` (as `TorqaApp.minimap_mesh()` gives it) under `track` and returns the picture
## as PNG; empty when there is nothing to draw, or no renderer to draw with.
static func capture(tree: SceneTree, map: Dictionary, track: PackedVector2Array) -> PackedByteArray:
	if map.is_empty() or track.size() < 2:
		return PackedByteArray()
	var preview: MapPreview = MapPreview.new()
	preview._take(map, track)
	tree.root.add_child(preview)
	await RenderingServer.frame_post_draw
	await RenderingServer.frame_post_draw
	var image: Image = preview.get_texture().get_image()
	preview.queue_free()
	if image == null or image.is_empty():
		return PackedByteArray()
	return image.save_png_to_buffer()


func _take(map: Dictionary, track: PackedVector2Array) -> void:
	_track = track
	_background = map["background"]
	var vertices: PackedVector2Array = map["vertices"]
	if not vertices.is_empty():
		var arrays: Array = []
		arrays.resize(Mesh.ARRAY_MAX)
		arrays[Mesh.ARRAY_VERTEX] = vertices
		arrays[Mesh.ARRAY_COLOR] = map["colors"]
		_mesh = ArrayMesh.new()
		_mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES, arrays)


func _draw_map() -> void:
	_canvas.draw_rect(Rect2(Vector2.ZERO, Vector2(SIZE)), _background)
	var bounds: Rect2 = Rect2(_track[0], Vector2.ZERO)
	for point: Vector2 in _track:
		bounds = bounds.expand(point)
	var extent: Vector2 = bounds.size.max(Vector2(1, 1))
	var available: Vector2 = Vector2(SIZE) - Vector2(PADDING, PADDING) * 2.0
	var fit: float = minf(available.x / extent.x, available.y / extent.y)
	# North up: screen y grows downwards.
	var view: Transform2D = (
		Transform2D()
		. translated(-bounds.get_center())
		. scaled(Vector2(fit, -fit))
		. translated(Vector2(SIZE) / 2.0)
	)
	if _mesh != null:
		_canvas.draw_mesh(_mesh, null, view)
	var points: PackedVector2Array = view * _track
	_canvas.draw_polyline(points, Color(PathCard.BLUE, 0.25), 7.0, true)
	_canvas.draw_polyline(points, PathCard.BLUE, 2.5, true)
	_canvas.draw_circle(points[points.size() - 1], 5.0, PathCard.FINISH)
	_canvas.draw_circle(points[0], 5.0, PathCard.START)
