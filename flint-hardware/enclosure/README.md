# FlintMesh Sensor Enclosure

Parametric Build123d model for the FlintMesh outdoor weatherproof sensor enclosure.
Mounts to a 2-inch steel pole. Designed for Nampa, Idaho conditions — intense heat,
high UV, and strong wind gusts.

---

## Components

| File exported | Description | Print notes |
|---|---|---|
| `enclosure_body.stl` | Main box, open top | PETG or ASA — UV stable |
| `lid.stl` | Flat lid with skirt | PETG or ASA |
| `pole_clamp_half.stl` | One half of split-ring clamp | Print × 2 |
| `solar_bracket.stl` | South-facing tilted panel mount | PETG or ASA |
| `anemometer_mount.stl` | 1-inch post stub + flange | PETG |
| `rain_sensor_bracket.stl` | L-bracket with forward tilt | PETG |

---

## Dependencies

```
build123d >= 0.10.0
bd-warehouse >= 0.2.0
```

Install with uv (recommended — already in `pyproject.toml`):

```bash
cd flint-hardware
uv sync
```

Or with pip:

```bash
pip install build123d bd-warehouse
```

For live 3D preview in VS Code, install the OCP CAD Viewer extension:

```bash
pip install ocp_vscode
```

---

## Running

Export all STL and STEP files to `enclosure/exports/`:

```bash
cd flint-hardware
uv run python enclosure/flint_enclosure.py
```

For live preview while editing in VS Code with the OCP CAD Viewer extension,
add this to the bottom of a scratch script:

```python
from ocp_vscode import show
from enclosure.flint_enclosure import build_assembly
show(build_assembly(explode=True))
```

---

## Modifying Parameters

Every dimension is defined in the `Params` dataclass at the top of
`flint_enclosure.py`. Nothing is hard-coded in the geometry functions.

To change a value, edit the relevant sub-dataclass default:

```python
# Make the enclosure taller
@dataclass(frozen=True)
class _EnclosureParams:
    height: float = 90.0  # was 75.0
```

Or override at call time without touching the defaults:

```python
from enclosure.flint_enclosure import build_enclosure_body, Params, _EnclosureParams

custom = Params(enc=_EnclosureParams(height=90.0, width=150.0))
body = build_enclosure_body(custom)
```

---

## Fastener Reference

All fasteners are metric with captive hex nuts. Nothing requires a nut to be
held from inside the enclosure during field assembly.

| Location | Fastener | Nut |
|---|---|---|
| Lid to body (× 4) | M3 × 12 SHCS | M3 hex captive in body boss |
| PCB standoffs (× 4) | M3 × 10 SHCS | Tapped boss |
| Anemometer flange (× 4) | M4 × 20 SHCS | M4 hex captive in lid |
| Solar bracket feet (× 4) | M4 × 16 SHCS | M4 hex captive in lid |
| Solar panel to bracket (× 4) | M4 × 16 SHCS | M4 hex captive in bracket |
| Pole clamp halves (× 2) | M5 × 45 SHCS | M5 hex captive in ear tab |
| Rain sensor (× 4) | M3 × 12 SHCS | M3 hex captive in wall |

SHCS = Socket Head Cap Screw

---

## Design Rationale

### Louver vents
Blades are angled 45° below horizontal — the same principle as a Stevenson
screen used in professional weather stations. Rain cannot drive straight
through, but air circulates freely past the BME680. Vent area is centred on
the middle third of each long wall to avoid the wetter lower zone near the
ground.

### Solar tilt
35° from horizontal. Nampa, ID sits at approximately 43.5°N latitude. Fixed
tilt equal to latitude maximises annual yield; the 35° value is slightly
below latitude to bias toward summer output when irradiance is highest and
the heat load on the electronics is greatest.

### Pole clamp
Split-ring design. One half is permanently attached to the enclosure rear
wall (four M4 screws); the second half is a field piece. Tightening the two
M5 clamp bolts cinches the ring around the pole. The ring wall is 8 mm thick
which provides enough contact area to resist wind loading without cracking
printed PETG.

### Material
ASA is preferred over PETG for all exterior-facing parts (body, lid, solar
bracket). ASA is significantly more UV stable and dimensionally stable at
high ambient temperatures — both important in Nampa summers. PETG is
acceptable for sheltered parts (pole clamp interior, rain sensor bracket if
mounted on the shaded side).

---

## Future Expansion

The file is structured so that new components are additive. To add a new
mount or bracket:

1. Add a `_MyNewParams` dataclass in the Parameters section
2. Add a `my_new_field: _MyNewParams = _MyNewParams()` line to `Params`
3. Write a `build_my_component(p: Params) -> Compound` function following
   the same pattern as the existing builders
4. Add it to `build_assembly()` and `export_all()`

Planned future components (not yet modelled):
- Hinged front access door with gasket groove
- Separate battery compartment tray
- Wind vane (direction) mount alongside anemometer
- Secondary conduit entry on side wall
- Version 2 pole clamp with stainless U-bolt option
