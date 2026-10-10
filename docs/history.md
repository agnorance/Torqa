# Ride history

Every finished ride is saved as a FIT file (for Strava, intervals.icu, Garmin Connect, …) in
the rider's folder, `profiles/<rider>/rides/` in the Torqa data directory, together with a small
JSON file holding its summary.

After a ride, its **summary** shows the same figures as the history: give the ride a name,
keep it with **Done** or **Discard ride**. Afterwards it is in the **History** tab.

**Names**: a ride is called after its course and date (e.g. *Gurtenstrasse · Sat 3 Oct*)
until you name it — press the pencil beside the title (or click the title), in the summary or later in the history. Names
are stored in the ride's JSON file only, so the files keep their names (sync-safe); uploads
will send the name as the activity title.

The history shows:

- **Figures**: time, distance, climbing, average speed, average / normalized / maximum power,
  intensity factor, training stress score (TSS), work in kJ, average heart rate and cadence.
- **Chart**: power and heart rate over the ride, on top of the elevation.
- **Time in zones**: power zones from your FTP, heart-rate zones from your maximum heart rate
  (see [riders.md](riders.md)). Sections without data, e.g. no heart-rate strap, are hidden.
- The bin beside the title (**Delete ride**) removes the FIT file and its summary, after asking.

Normalized power needs at least 30 s of power data. Intensity and TSS use the FTP the rider had
when the ride was saved, so they do not change when you update your FTP later; time in zones
uses the current profile.

FIT files copied into a rider's `rides` folder by hand appear in the history too; their summary
is computed and saved the first time the history is opened.

## Climbs and personal records

Torqa finds the climbs of every route automatically — rises of at least 3 % on average and
300 m long, with length × gradient of at least 3 000 (e.g. 1 km at 3 %) — and rates them like popular platforms by length × gradient: *Climb* (small),
*Cat 4* (from 8 000, e.g. 2 km at 4 %), *Cat 3* (16 000), *Cat 2* (32 000), *Cat 1* (64 000) and
*HC* (80 000, e.g. Alpe d'Huez).

- A course page lists its climbs with your best times there.
- While riding, the elevation profile marks the climbs in their category colour; on a climb a
  panel shows the category, distance to the top, gradient, your time so far and your best.
- At the top of each climb and at the finish you see your time — and whether it is a new
  personal record.
- In the history, each ride lists its route and climb times; ★ marks your personal records.

Records count per rider and per course: riding the same GPX file or course again compares with
your earlier rides on it. Rides added from FIT files alone count towards no records, as their
route is unknown.
