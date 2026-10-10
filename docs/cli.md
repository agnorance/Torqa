# torqa-cli

Headless tool to find and ride smart trainers without the app. Useful for testing trainers and
sensors.

## Install (macOS)

Download the `torqa-cli-macos-arm64.tar.gz` artifact from a CI run, then:

```sh
tar -xzf torqa-cli-macos-arm64.tar.gz
xattr -d com.apple.quarantine torqa-cli   # unsigned binary
./torqa-cli --help
```

On first use macOS asks whether your terminal app may use Bluetooth — allow it
(System Settings → Privacy & Security → Bluetooth).

## Install (Linux)

Download `torqa-cli-linux-x86_64.tar.gz` (or `-arm64`) from a CI run, then:

```sh
tar -xzf torqa-cli-linux-x86_64.tar.gz
./torqa-cli --help
```

It runs on distributions with glibc 2.35 or newer (Ubuntu 22.04+, Debian 12+). Bluetooth goes
through BlueZ, so its service must be running (`systemctl status bluetooth`) and the adapter
powered on (`bluetoothctl power on`).

## Find devices

```sh
./torqa-cli scan               # scans 5 s for FTMS trainers and heart-rate sensors
./torqa-cli scan --seconds 10
```

Wake the trainer by pedalling and make sure no other app (Zwift, Wahoo app) is connected to it —
FTMS allows only one controlling app. Heart-rate straps usually only advertise while worn (moisten
the electrodes) and while not connected to a watch or phone.

## Ride

```sh
./torqa-cli ride                       # strongest trainer found
./torqa-cli ride --trainer kickr --hr  # trainer by name + strongest heart-rate strap
./torqa-cli ride --hr polar            # heart-rate strap by name
./torqa-cli ride --fake                # simulated trainer, no hardware
./torqa-cli ride --fake --fake-power 250 --fake-cadence 95   # the simulated rider's output
```

| Option | Meaning |
|---|---|
| `--trainer kickr` | Trainer by name (part of it, any case); default the strongest signal |
| `--hr [name]` | Also connect a heart-rate strap, by name or the strongest |
| `--fake` | The simulated trainer instead of Bluetooth, with a simulated heart rate (with `--hr`, a real strap connects instead) |
| `--fake-power 200`, `--fake-cadence 90` | What the simulated rider pedals, in W and rpm |
| `--controller [name]` | Also connect a Shimano Di2 shifter, by name or the strongest (e.g. `RDR9250`) |
| `--up-channel 1`, `--down-channel 2` | The Di2 shifter's D-Fly channels that shift up and down (with `--gears`) |
| `--scan-seconds 5` | How long to scan for the trainer and strap |

Live readings are printed every second. Without a route, type a command and press Enter:

| Command | Effect |
|---|---|
| `g 5` | SIM mode, 5 % grade (negative for descents) |
| `p 200` | ERG mode, hold 200 W |
| `r 30` | Resistance at 30 % of the trainer's range |
| `q` | Quit and disconnect (Ctrl+C works too; press it twice to skip the disconnect) |

If no heart-rate strap is found, the ride continues without one. Wahoo trainers estimate cadence
from the flywheel, so it may read 0 for the first seconds or at very low power.

## Ride a route

```sh
./torqa-cli route my-ride.gpx                     # length, climbing, elevation source, map
./torqa-cli route my-ride.gpx --world             # also build its 3D world: size and timings
./torqa-cli ride --route my-ride.gpx --hr         # ride it; the trainer follows the gradient
./torqa-cli ride --route my-ride.gpx --difficulty 100 --descent flat --mass 90
./torqa-cli ride --route my-ride.gpx --fake --time-scale 50   # quick simulated test ride
```

Like an import in the app, the track is put onto the roads it rides (OpenStreetMap) and its
elevations come from a terrain model (corrected and smoothed); bridges and tunnels of the roads
ridden run straight between their ends. Map and terrain data are downloaded once and cached, so
a route you have imported before also works offline (`--offline` forces cache-only). Without
terrain data, the GPX elevations are used; without map data, the track stays as recorded and
bridges and tunnels follow the ground (short dips or humps).

| Option | Meaning |
|---|---|
| `--difficulty 50` | Share of the road gradient you feel on the trainer (speed always uses the real gradient) |
| `--descent coast\|flat` | Coast: gravity builds speed downhill. Flat: descents ride like flat roads |
| `--mass 83` | Rider plus bike in kg |
| `--output ride.fit` | Where to save the activity (default `torqa-<date>-<time>.fit`) |
| `--offline` | Use cached map and terrain data only (also for `route`) |
| `--time-scale 50` | Run simulated time faster; only with `--fake` |
| `--gears 50x14` | Virtual gears on a single cog (chainring x cog); type `u` / `d` and Enter to shift |

On a route the trainer follows the gradient, so `g`, `p` and `r` do not apply: the ride starts
when the trainer connects and ends at the finish or with `q` / Ctrl+C; the FIT
file can be uploaded to Strava, intervals.icu, Garmin Connect and others as a virtual ride.

Map data: © [OpenFreeMap](https://openfreemap.org) © [OpenMapTiles](https://openmaptiles.org),
data © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright) (ODbL).
Terrain data: [Mapterhorn](https://mapterhorn.com/attribution) (CC BY 4.0) and
[AWS Terrain Tiles](https://registry.opendata.aws/terrain-tiles/).

## Check virtual gears

```sh
./torqa-cli gear-check --trainer kickr --gears 50x14   # chainring x cog on the trainer
./torqa-cli gear-check --trainer kickr --controller     # shift with the Di2 buttons
```

Rides a flat virtual road in virtual gears (ADR 0003) and shows, every second, whether the
trainer brakes the way the gear asks it to. Type `u` / `d` and Enter to shift, a number (1–24)
to jump to that gear, `g 4` to ride a 4 % road, `q` to quit. Pedal steadily for a few seconds
after each change; the values are averaged over the second.

```text
gear 24 5.50  sent +0.0 % Crr 0.0062 Cw 0.72 |  420 W  91 rpm  41.0 km/h | felt 3.57 (real 3.57) | expect 1114 W, ungeared 327 W | virtual 49.3 km/h = 4.29
```

| Part | Meaning |
|---|---|
| `gear 24 5.50` | The virtual gear and its ratio (chainring over cog) |
| `sent …` | The gradient, rolling resistance and wind coefficient sent to the trainer for that gear |
| `W rpm km/h` | Power, cadence and speed as the trainer measures them |
| `felt 3.57 (real 3.57)` | The ratio the trainer turns in: its speed over cadence × wheel. It should match the real gear; if not, `--gears` does not match the bike, or the trainer assumes another wheel |
| `expect … W` | The power the trainer should brake at its speed with the parameters sent |
| `ungeared … W` | The power it would brake at that speed without the gear (the road's parameters) |
| `virtual … = 4.29` | Torqa's speed from your power, and the ratio it implies at your cadence: it settles at the gear's ratio when the trainer brakes as expected |

If the measured power stays near `ungeared` instead of `expect` on a flat road, the trainer
ignores the rolling resistance and wind coefficient of the simulation command, so the gears
only take effect where there is a gradient. `--mass 83` (rider plus bike) and `--wheel 2.105`
(circumference in m) set the values the expectation is computed with; the trainer uses its own
rider weight, so on gradients the expectation is approximate. Speed and the felt ratio need a
trainer that reports speed; the fake trainer (`--fake`) does not.

## Ride a workout

```sh
./torqa-cli ride --power 200 --hr                    # hold 200 W (ERG) as long as you like
./torqa-cli ride --hr-zone 3 --hr --max-hr 190 --ftp 250   # hold the middle of heart-rate zone 3
./torqa-cli ride --hr-target 135 --hr --min-power 120 --max-power 220
./torqa-cli ride --fake --hr-zone 2                  # try it with the simulated rider's heart
```

A workout needs no route: the trainer holds a power (ERG mode) and you ride a flat road, so speed
and distance come from your power. A **heart-rate workout** adjusts that power continuously so
your heart rate settles at the target: it starts at the minimum power, rises by at most 30 W a
minute (heart rate lags power by 30–60 s, so faster changes would overshoot), never leaves the
limits you set, and eases off as your heart rate drifts up during a long ride. Expect the target
to be reached after about 5–10 minutes. Without a heart rate (strap off, or dropped out) the power
stays where it is until the heart rate is back.

| Option | Meaning |
|---|---|
| `--power 200` | Constant-power workout: hold this many watts |
| `--hr-zone 3` | Heart-rate workout: hold the middle of this zone (1–5, e.g. 75 % of your maximum for zone 3) |
| `--hr-target 140` | Heart-rate workout: hold this heart rate in bpm |
| `--min-power 100`, `--max-power 250` | The least and most a heart-rate workout asks for, in W |
| `--workout file.zwo` | Structured workout: a ZWO, ERG, MRC or FIT workout file, or `builtin:<name>` |
| `--ftp 200`, `--max-hr 185` | Your FTP and maximum heart rate: the zones, how much power a beat off target is worth, and the watts of structured workouts |

Live readings show the target power (and heart rate, or the step of a structured workout and its
messages). `./torqa-cli workout builtins` lists the built-in workouts and
`./torqa-cli workout my.zwo --ftp 250` shows a file's steps as Torqa reads them. End the workout with `q` or Ctrl+C; it is
saved as a FIT file like a route ride, as indoor cycling without positions. `--mass`,
`--output` and (for constant power only) `--time-scale` apply as for routes.

## Check a video

```sh
./torqa-cli video my-ride.mp4                 # how the video keeps up on this machine
./torqa-cli video my-ride.mp4 --seconds 20
```

Plays the video against the clock for eight seconds, as a ride at 1× does, and reports the
frames shown per second, the longest wait for a frame and the straight decoding rate, with a
verdict. A video that cannot keep up here stutters in the app too; see [video.md](video.md)
on sizes and codecs.

## Connection

If the trainer drops out, the CLI reconnects automatically and re-applies the last command.
Set `RUST_LOG=debug` for protocol details (accepted/rejected commands, resistance range).
