class_name UiIcons
extends RefCounted
## Line icons for the interface's buttons, drawn from small SVGs in a theme colour: crisp at
## any size, and no image files to keep. Each is the paths of a 24 × 24 drawing.

const PATHS: Dictionary[String, String] = {
	"cog":
	(
		'<circle cx="12" cy="12" r="3"/><path d="M12 2v2.5M12 19.5V22M2 12h2.5M19.5 12H22'
		+ 'M4.9 4.9l1.8 1.8M17.3 17.3l1.8 1.8M4.9 19.1l1.8-1.8M17.3 6.7l1.8-1.8"/>'
		+ '<circle cx="12" cy="12" r="7"/>'
	),
	"overlay":
	(
		'<rect x="3" y="4" width="18" height="14" rx="2"/>'
		+ '<rect x="12" y="9" width="7" height="7" rx="1.5" fill="COLOR" stroke="none"/>'
	),
	"flag": '<path d="M5 21V4"/><path d="M5 4h11l-2 4 2 4H5"/>',
	"back": '<path d="M19 12H5"/><path d="M12 19l-7-7 7-7"/>',
	"fold": '<path d="M15 6l-6 6 6 6"/>',
	"unfold": '<path d="M9 6l6 6-6 6"/>',
	"pause": '<path d="M8 5v14"/><path d="M16 5v14"/>',
	"play": '<path d="M7 4l12 8-12 8z" fill="COLOR" stroke="none"/>',
	"pencil": '<path d="M4 20l4-1 10-10-3-3L5 16z"/><path d="M13 7l3 3"/>',
	"bin": '<path d="M4 7h16"/><path d="M9 7V4h6v3"/><path d="M6 7l1 13h10l1-13"/>',
	"plus": '<path d="M12 5v14"/><path d="M5 12h14"/>',
	"cross": '<path d="M6 6l12 12"/><path d="M18 6L6 18"/>',
}
static var _cache: Dictionary[String, ImageTexture] = {}


## The icon `name` at `size` pixels, in `color`.
static func texture(name: String, size: int = 20, color: Color = UiTheme.TEXT) -> ImageTexture:
	var key: String = "%s/%d/%s" % [name, size, color.to_html()]
	if _cache.has(key):
		return _cache[key]
	var hex: String = "#" + color.to_html(false)
	var svg: String = (
		(
			'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24" '
			+ 'fill="none" stroke="COLOR" stroke-width="1.8" stroke-linecap="round" '
			+ 'stroke-linejoin="round" opacity="%.3f">%s</svg>'
		)
		% [color.a, PATHS[name]]
	)
	var image: Image = Image.new()
	image.load_svg_from_string(svg.replace("COLOR", hex), size / 24.0)
	var icon: ImageTexture = ImageTexture.create_from_image(image)
	_cache[key] = icon
	return icon
