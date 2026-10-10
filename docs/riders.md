# Riders

Each rider has a profile: name, weight, bike weight, FTP, maximum heart rate and units
(metric or imperial). Pick the rider in the **Profile** tab; the pencil beside the list
changes the profile, the plus adds a rider. The tab shows the rider's figures; **All settings**
under them unfolds everything the dialog has, the HUD's layout included; the zones stand beside
the figures with their ranges.

- **Weight + bike weight** set how hard climbs are and how fast you roll.
- **FTP** sets the power zones shown under the power figure (Coggan's seven zones:
  Recovery < 55 %, Endurance ≤ 75 %, Tempo ≤ 90 %, Threshold ≤ 105 %, VO2max ≤ 120 %,
  Anaerobic ≤ 150 %, Neuromuscular above), together with watts per kilogram.
- **Maximum heart rate** sets five heart-rate zones (≤ 60, 70, 80, 90 % and above).
- **Units** switch speed, distance and elevation between km/h, km, m and mph, mi, ft.
- **Drivetrain**: with a **cassette** you shift on the bike as outdoors. On a **single cog**
  (e.g. the Zwift Cog) Torqa gives you 24 **virtual gears** (R9): set your chainring and the
  cog's teeth, and shift with **↑** and **↓** while riding (see [riding.md](riding.md)).

Profiles are stored in the data directory as `profiles/<rider>/profile.toml` and can be edited by
hand; each rider's activities are saved in `profiles/<rider>/rides/`. Courses are shared by all
riders.
