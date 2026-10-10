shader_type spatial;
// The wind clock: the world advances it, and holds it while the ride is paused.
global uniform float wind_time;
// Trees, bushes and rocks from Blender (art/vegetation, R45, ADR 0011): flat palette colours —
// leaves and rocks take their instance colour (COLOR, sRGB, chosen per plant by the world), trunks a
// shared one — faceted on screen, with trees swaying a little in the wind, more towards their
// tops.

uniform vec3 albedo : source_color = vec3(0.478, 0.341, 0.333);
// 1 colours by the instance, 0 by `albedo`.
uniform float instance_tint : hint_range(0.0, 1.0) = 0.0;
// How far a point 10 m up moves in the wind, in metres; trunk and leaves share it.
uniform float sway = 0.0;
// Set by the weather.
uniform float wind_strength : hint_range(0.0, 1.0) = 0.1;

varying vec3 tint;

vec3 to_linear(vec3 srgb) {
	return mix(srgb / 12.92, pow((srgb + 0.055) / 1.055, vec3(2.4)), step(0.04045, srgb));
}

void vertex() {
	// Each plant sways out of step with its neighbours: the phase follows where it stands.
	vec2 place = MODEL_MATRIX[3].xz;
	float phase = place.x * 0.13 + place.y * 0.07;
	float bend = max(VERTEX.y, 0.0) / 10.0;
	vec2 push = vec2(sin(wind_time * 1.3 + phase), cos(wind_time * 1.1 + phase * 1.7)) * bend * bend;
	VERTEX.xz += push * sway * wind_strength * 10.0;
	tint = mix(albedo, to_linear(COLOR.rgb), instance_tint);
}

void fragment() {
	NORMAL = normalize(cross(dFdy(VERTEX), dFdx(VERTEX)));
	ALBEDO = tint;
	ROUGHNESS = 0.9;
	SPECULAR = 0.15;
}
