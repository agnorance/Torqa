// Grass tufts and flower clumps (R45): swaying in the wind — tips more than bases, in gusts
// that travel across the field — lit like the ground they grow from (their normals point up)
// with some light shining through. Flower heads take one of the palette's flower colours per
// clump.
shader_type spatial;
// The wind clock: the world advances it, and holds it while the ride is paused.
global uniform float wind_time;
render_mode cull_disabled;

uniform float wind_strength : hint_range(0.0, 1.0) = 0.12;
uniform vec2 wind_direction = vec2(1.0, 0.35);
uniform vec4 flower_colors[4] : source_color;

varying vec3 tint;

float hash(float n) {
	return fract(sin(n * 12.9898) * 43758.5453);
}

void vertex() {
	// UV.y runs from the root (0) to the tip (1).
	float bend = UV.y * UV.y;
	vec3 world = (MODEL_MATRIX * vec4(VERTEX, 1.0)).xyz;
	float gust = sin(wind_time * 1.6 + world.x * 0.31 + world.z * 0.17) * 0.65
		+ sin(wind_time * 3.3 + world.x * 1.1 - world.z * 0.7) * 0.35;
	vec3 wind = vec3(wind_direction.x, 0.0, wind_direction.y) * (0.6 + 0.4 * gust);
	// The wind blows in world space; the tuft is turned and scaled by its instance.
	mat3 to_local = inverse(mat3(MODEL_MATRIX));
	VERTEX += to_local * wind * wind_strength * bend;
	// Flower heads (alpha 0) take a colour per clump.
	vec3 color = pow(COLOR.rgb, vec3(2.2));
	if (COLOR.a < 0.5) {
		color = flower_colors[int(hash(float(INSTANCE_ID)) * 3.999)].rgb;
	}
	tint = color * (0.85 + hash(float(INSTANCE_ID) + 7.0) * 0.3);
}

void fragment() {
	ALBEDO = tint;
	ROUGHNESS = 0.85;
	SPECULAR = 0.15;
	BACKLIGHT = tint * 0.35;
}
