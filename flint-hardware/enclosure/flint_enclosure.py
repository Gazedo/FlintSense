"""
flint_enclosure.py
==================
FlintMesh parametric outdoor sensor enclosure — Build123d model.

This file is intentionally written as a learning example for Build123d.
Each section is commented to explain both the geometry being created and
the Build123d pattern being demonstrated.

Hardware housed
---------------
  - RAK4631 WisBlock base board (60 × 30 mm)
  - BME680 environmental sensor (RAK1906 WisBlock module)
  - LiPo battery pack
  - LoRa antenna (SMA bulkhead through rear wall)

External provisions
-------------------
  - 2-inch (50.8 mm) schedule-40 steel pole mount — split clamp, rear
  - Anemometer post stub — top centre
  - Raindrop sensor bracket — side, slight forward tilt
  - South-facing solar panel bracket — top, 35° tilt (Nampa ID latitude)

Fastener strategy
-----------------
  All fasteners are metric. Captive hex nuts are used throughout so that
  nothing needs to be held with a tool from inside the enclosure.
  Sizes used: M3 (PCB + lid), M4 (solar + anemometer), M5 (pole clamp).

Usage
-----
  # Export STL and STEP files for all components:
  uv run python enclosure/flint_enclosure.py

  # Or import individual builders for inspection in ocp_vscode:
  from enclosure.flint_enclosure import build_enclosure_body, PARAMS

Design notes
------------
  Louver geometry follows Stevenson screen principles: blades are angled
  45° below horizontal so rain cannot drive straight through, but air
  circulates freely past the BME680. The solar tilt default of 35° is
  optimised for Nampa, ID (43.5 °N) — slightly below latitude for
  summer-bias generation given the intense heat load.
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from pathlib import Path

from build123d import (
    # ── Builders (context managers that accumulate geometry) ──────────────
    BuildPart,
    BuildSketch,
    # ── 3-D primitives ─────────────────────────────────────────────────────
    Box,
    Cylinder,
    # ── 2-D primitives (used inside BuildSketch) ───────────────────────────
    Circle,
    Rectangle,
    RegularPolygon,
    # ── Operations ─────────────────────────────────────────────────────────
    offset,      # hollow a solid (negative amount + openings) or grow/shrink geometry
    fillet,      # round edges
    chamfer,     # bevel edges
    mirror,      # reflect geometry about a plane
    add,         # insert an existing shape into the active builder
    # ── Placement ──────────────────────────────────────────────────────────
    Location,           # position + optional rotation
    Locations,          # single-location context
    GridLocations,      # rectangular grid of locations
    PolarLocations,     # circular pattern of locations
    # ── Topology selectors ─────────────────────────────────────────────────
    Axis,        # X / Y / Z — used to filter/sort faces and edges
    Plane,       # XY / YZ / XZ — used for mirror and workplanes
    # ── Alignment ──────────────────────────────────────────────────────────
    Align,       # MIN / CENTER / MAX — controls primitive anchor point
    # ── Mode ───────────────────────────────────────────────────────────────
    Mode,        # ADD (default) / SUBTRACT / INTERSECT / REPLACE
    # ── Export ─────────────────────────────────────────────────────────────
    export_stl,
    export_step,
    # ── Geometry types ─────────────────────────────────────────────────────
    Compound,
)

from bd_warehouse.fastener import (
    HexNut,
    SocketHeadCapScrew,
    ClearanceHole,
)


# ══════════════════════════════════════════════════════════════════════════════
#  PARAMETERS
#  ----------
#  Every tunable dimension lives here. Change a value and all downstream
#  geometry updates automatically — nothing is hard-coded in the builders.
#
#  Build123d pattern: using frozen dataclasses as parameter namespaces keeps
#  values grouped, gives IDE autocomplete, and prevents accidental mutation.
# ══════════════════════════════════════════════════════════════════════════════

@dataclass(frozen=True)
class _EnclosureParams:
    """Main enclosure shell dimensions."""
    width:        float = 130.0  # X — left / right
    depth:        float = 90.0   # Y — front / back
    height:       float = 75.0   # Z — up / down
    wall:         float = 3.0    # shell wall thickness
    corner_r:     float = 5.0    # external vertical-edge fillet radius
    lid_skirt:    float = 8.0    # depth the lid skirt overlaps the body
    lid_gap:      float = 0.3    # printable clearance between body and lid skirt


@dataclass(frozen=True)
class _LouverParams:
    """Stevenson-screen louver vent geometry."""
    count:        int   = 6      # blades per vent panel
    angle_deg:    float = 45.0   # blade tilt below horizontal — blocks rain
    gap:          float = 2.5    # clear air-gap height per blade
    margin:       float = 10.0   # solid border around the vent array


@dataclass(frozen=True)
class _PoleClampParams:
    """Split-ring clamp for a 2-inch schedule-40 steel pole."""
    pole_dia:     float = 50.8   # 2 inch nominal OD in mm
    bore_clear:   float = 0.5    # radial clearance over pole surface
    band_width:   float = 50.0   # clamp height (Z)
    ring_wall:    float = 8.0    # radial wall thickness of ring
    ear_width:    float = 22.0   # bolt-ear tab width
    ear_thick:    float = 12.0   # bolt-ear tab thickness
    bolt_size:    str   = "M5-0.8"
    bolt_length:  float = 45.0   # long enough for both ears + nut pocket


@dataclass(frozen=True)
class _SolarBracketParams:
    """South-facing tilted solar panel top bracket."""
    tilt_deg:          float = 35.0   # optimal for Nampa ID (~43.5 °N) year-round
    arm_height:        float = 40.0   # bracket arm rise above lid surface
    arm_thickness:     float = 5.0    # arm wall thickness
    panel_hole_span_x: float = 160.0  # panel mounting hole X spacing
    panel_hole_span_y: float = 100.0  # panel mounting hole Y spacing
    bolt_size:         str   = "M4-0.7"
    bolt_length:       float = 16.0


@dataclass(frozen=True)
class _AnemometerMountParams:
    """Vertical post stub for a standard cup anemometer."""
    post_dia:     float = 25.4   # 1-inch post — fits most consumer anemometers
    post_height:  float = 80.0   # stub height above lid
    base_dia:     float = 55.0   # mounting flange outer diameter
    base_thick:   float = 7.0    # flange thickness
    n_bolts:      int   = 4      # bolts on a PolarLocations pattern
    bolt_pcd:     float = 42.0   # bolt circle diameter
    bolt_size:    str   = "M4-0.7"
    bolt_length:  float = 20.0


@dataclass(frozen=True)
class _RainSensorParams:
    """L-bracket for a raindrop / tipping-bucket sensor."""
    plate_width:  float = 70.0   # bracket mounting plate width
    plate_depth:  float = 50.0   # bracket mounting plate depth
    plate_thick:  float = 3.5    # bracket plate thickness
    tilt_deg:     float = 10.0   # slight forward tilt for drainage
    bolt_size:    str   = "M3-0.5"
    bolt_length:  float = 12.0


@dataclass(frozen=True)
class _PCBMountParams:
    """RAK WisBlock base-board standoff dimensions."""
    board_width:  float = 60.0
    board_depth:  float = 30.0
    standoff_h:   float = 6.0    # clearance under PCB
    standoff_od:  float = 6.0    # standoff outer diameter
    hole_inset:   float = 3.0    # PCB mounting hole inset from board edge
    bolt_size:    str   = "M3-0.5"
    bolt_length:  float = 10.0


# ── Instantiated defaults — pass custom instances to override ─────────────────
# Example: build_enclosure_body(enc=_EnclosureParams(height=90.0))

@dataclass(frozen=True)
class Params:
    """Single namespace for all sub-parameter groups."""
    enc:   _EnclosureParams   = _EnclosureParams()
    louver: _LouverParams     = _LouverParams()
    pole:  _PoleClampParams   = _PoleClampParams()
    solar: _SolarBracketParams = _SolarBracketParams()
    anem:  _AnemometerMountParams = _AnemometerMountParams()
    rain:  _RainSensorParams  = _RainSensorParams()
    pcb:   _PCBMountParams    = _PCBMountParams()


PARAMS = Params()  # module-level default — use this unless overriding


# ══════════════════════════════════════════════════════════════════════════════
#  HELPERS
# ══════════════════════════════════════════════════════════════════════════════

def _nut_and_clearance(
    bolt_size: str,
    bolt_length: float,
    depth_override: float | None = None,
) -> tuple[HexNut, SocketHeadCapScrew]:
    """
    Return a matched (HexNut, SocketHeadCapScrew) pair for a given size string.

    Build123d pattern: bd_warehouse fastener objects carry all dimensions
    (across-flats, thickness, clearance diameters) so NutHole / ClearanceHole
    can compute pocket geometry automatically — no manual diameter look-up.
    """
    nut   = HexNut(bolt_size)
    screw = SocketHeadCapScrew(bolt_size, bolt_length)
    return nut, screw


def _captive_nut_boss(
    bolt_size: str,
    bolt_length: float,
    boss_od: float,
    boss_height: float,
) -> Compound:
    """
    Standalone cylinder boss with a captive hex-nut pocket in the top and a
    bolt clearance hole through the centre.

    Use this with mode=Mode.ADD to grow a boss from a flat surface, then
    position it with Locations.

    Build123d pattern: building reusable sub-parts as standalone functions
    that return Compound objects is cleaner than copy-pasting geometry.
    The caller decides placement; this function only decides shape.
    """
    nut, screw = _nut_and_clearance(bolt_size, bolt_length)

    with BuildPart() as boss:
        Cylinder(
            radius=boss_od / 2,
            height=boss_height,
            align=(Align.CENTER, Align.CENTER, Align.MIN),
        )
        # captive_nut=True creates the hex pocket (nut thickness) at the face
        # plus a bolt-clearance through-hole for the remaining depth.
        with Locations(Location((0, 0, boss_height))):
            ClearanceHole(fastener=nut, captive_nut=True, counter_sunk=False, depth=boss_height)

    return boss.part


# ══════════════════════════════════════════════════════════════════════════════
#  COMPONENT 1 — ENCLOSURE BODY
#  Build123d patterns demonstrated:
#    - Box + Shell to create a hollow body
#    - fillet / chamfer on selected edge sets
#    - Louver cuts via angled Box subtractions in a loop
#    - Cable gland boss (additive cylinder + subtractive bore)
#    - PCB standoffs via GridLocations
#    - Lid-screw captive-nut bosses at internal corners
# ══════════════════════════════════════════════════════════════════════════════

def build_enclosure_body(p: Params = PARAMS) -> Compound:
    """
    Main weatherproof enclosure body.

    Open top. Closed bottom and four walls.
    Louver vents on both long (Y-axis) side walls allow the BME680 to breathe
    while blocking direct rain. A cable gland bore on the rear wall accepts a
    PG-7 fitting for antenna + power leads.
    """
    e = p.enc
    lv = p.louver
    pcb = p.pcb

    with BuildPart() as body:

        # ── Step 1: solid outer block ────────────────────────────────────────
        # Align.MIN on Z means the box sits on Z=0, making all subsequent
        # Z positions easy to reason about (Z=0 is the mounting surface).
        Box(
            e.width, e.depth, e.height,
            align=(Align.CENTER, Align.CENTER, Align.MIN),
        )

        # ── Step 2: round the four vertical edges ────────────────────────────
        # filter_by(Axis.Z) selects edges whose tangent is parallel to Z —
        # i.e. the four vertical corner edges only, not horizontal rim edges.
        # Do this BEFORE hollowing so only exterior corners are rounded.
        fillet(body.edges().filter_by(Axis.Z), radius=e.corner_r)

        # ── Step 3: chamfer the top rim ───────────────────────────────────────
        # Must happen BEFORE hollowing while the top is still a solid flat face.
        # After the inner-box subtraction the top face is gone (open ring), so
        # chamfer would have no valid face to operate on.
        # Water sheds outward over this chamfer instead of pooling on the rim.
        chamfer(body.faces().sort_by(Axis.Z)[-1].edges(), length=1.5)

        # ── Step 4: hollow interior via inner-box subtraction ────────────────
        # Subtract an inner box that starts at Z=e.wall (NOT Z=0).
        # Starting at Z=e.wall preserves the solid floor slab (0..e.wall),
        # which PCB standoffs and corner bosses can fuse into later.
        # The inner box is taller than the enclosure so it exits through
        # the open top — no separate top-face operation needed.
        #
        # Build123d pattern: placing the subtraction at a Z offset via
        # Locations is cleaner than computing a partial height.
        with Locations(Location((0, 0, e.wall))):
            Box(
                e.width  - 2 * e.wall,
                e.depth  - 2 * e.wall,
                e.height,               # oversized — clears the open top
                align=(Align.CENTER, Align.CENTER, Align.MIN),
                mode=Mode.SUBTRACT,
            )

        # ── Step 5: louver vents on both long side walls ─────────────────────
        # Each louver is a thin Box subtracted through the wall, tilted 45°
        # about the X axis. The tilt means:
        #   - from outside (rain side): the slot faces upward → rain sheds off
        #   - from below (airflow): air rises through the angled gap freely
        # This is the Stevenson screen principle used in weather stations.
        #
        # Build123d pattern: Location((x,y,z), axis, angle) places geometry
        # at (x,y,z) rotated <angle> degrees about the given axis vector.

        vent_width  = e.width * 0.55    # louver field spans 55 % of wall width
        vent_z_lo   = e.height * 0.30   # vent zone: middle 40 % of height
        vent_z_hi   = e.height * 0.70
        pitch       = (vent_z_hi - vent_z_lo) / lv.count
        # Diagonal length needed to fully penetrate wall at the louver angle
        cut_depth   = (e.wall + 2) / math.cos(math.radians(lv.angle_deg))

        for side_y in [-e.depth / 2, e.depth / 2]:
            for i in range(lv.count):
                z = vent_z_lo + (i + 0.5) * pitch
                with Locations(Location((0, side_y, z), (1, 0, 0), lv.angle_deg)):
                    Box(vent_width, cut_depth, lv.gap, mode=Mode.SUBTRACT)

        # ── Step 6: cable entry bore through rear wall ───────────────────────
        # PG-7 gland bore (12.5 mm dia) drilled horizontally through the rear
        # wall. The cylinder's default axis is Z; rotating 90° about X makes
        # it point along Y, which is the rear wall's outward normal.
        #
        # Build123d pattern: rotating a primitive via the axis-angle form of
        # Location is the idiomatic way to orient a cylinder along a non-Z axis.
        rear_wall_y = -(e.depth / 2 - e.wall / 2)   # Y centre of rear wall slab
        cable_z     = e.height * 0.22
        with Locations(Location((0, rear_wall_y, cable_z), (1, 0, 0), 90)):
            Cylinder(
                radius=6.25,
                height=e.wall + 4,      # +4 ensures clean break-through
                align=(Align.CENTER, Align.CENTER, Align.CENTER),
                mode=Mode.SUBTRACT,
            )

        # ── Step 7: PCB standoffs ────────────────────────────────────────────
        # Four posts for RAK WisBlock base (60 × 30 mm, holes 3 mm from edge).
        # Each post starts at Z=0 so it roots into the floor slab and fuses.
        # Height = e.wall + standoff_h so the top sits standoff_h above the
        # interior floor surface.
        #
        # Build123d pattern: GridLocations(x_spacing, y_spacing, nx, ny)
        # creates an nx×ny grid centred at the active origin.
        so_x = pcb.board_width / 2 - pcb.hole_inset   # = 27 mm
        so_y = pcb.board_depth / 2 - pcb.hole_inset   # = 12 mm

        with GridLocations(so_x * 2, so_y * 2, 2, 2):
            Cylinder(
                radius=pcb.standoff_od / 2,
                height=e.wall + pcb.standoff_h,  # roots into floor slab
                align=(Align.CENTER, Align.CENTER, Align.MIN),
            )
            Cylinder(
                radius=1.25,                     # M3 tapped bore
                height=e.wall + pcb.standoff_h,
                align=(Align.CENTER, Align.CENTER, Align.MIN),
                mode=Mode.SUBTRACT,
            )

        # ── Step 8: lid-screw captive-nut bosses ─────────────────────────────
        # Four corner bosses, each with a captive M3 hex-nut pocket in the top.
        # Lid screws pass through the lid plate and thread into these nuts.
        #
        # CRITICAL: each boss is positioned so its outer edge overlaps 1.5 mm
        # into the adjacent corner walls. This shared volume is required for
        # OCCT's boolean union to fuse the boss with the wall — geometry that
        # merely touches (zero overlap) will remain a separate body.
        #
        # boss_offset_x = outer_half - wall + overlap - boss_radius
        # With e.width=130, e.wall=3, wall_overlap=1.5, boss_od=10:
        #   offset_x = 65 - 3 + 1.5 - 5 = 58.5
        #   boss X extent: [58.5-5, 58.5+5] = [53.5, 63.5]
        #   wall X extent: [62, 65]
        #   overlap zone:  [62, 63.5] = 1.5 mm ✓

        nut_m3       = HexNut("M3-0.5")
        boss_od      = 10.0
        boss_h       = 15.0
        wall_overlap = 1.5    # mm — minimum overlap for guaranteed fusion

        off_x = e.width / 2 - e.wall + wall_overlap - boss_od / 2
        off_y = e.depth / 2 - e.wall + wall_overlap - boss_od / 2
        boss_z = e.height - boss_h   # sit near top so screw reaches down from lid

        for sx, sy in [(-1, -1), (1, -1), (-1, 1), (1, 1)]:
            bx, by = sx * off_x, sy * off_y
            with Locations(Location((bx, by, boss_z))):
                Cylinder(
                    radius=boss_od / 2,
                    height=boss_h,
                    align=(Align.CENTER, Align.CENTER, Align.MIN),
                )
                with Locations(Location((0, 0, boss_h))):
                    ClearanceHole(
                        fastener=nut_m3,
                        captive_nut=True,
                        counter_sunk=False,
                        depth=boss_h,
                    )

    return body.part


# ══════════════════════════════════════════════════════════════════════════════
#  COMPONENT 2 — LID
#  Build123d patterns demonstrated:
#    - Flat box with a dependent skirt that fits over the body rim
#    - ClearanceHole matching the body's captive-nut bosses
#    - Through-holes sized for anemometer mount and solar bracket
# ══════════════════════════════════════════════════════════════════════════════

def build_lid(p: Params = PARAMS) -> Compound:
    """
    Flat lid with a downward skirt that registers over the body rim.

    The lid is retained by four M3 socket-head cap screws that thread into
    the captive nuts inside the body. Anemometer and solar bracket bolt
    patterns are pre-punched so no drilling is needed in the field.
    """
    e  = p.enc
    a  = p.anem
    s  = p.solar
    nut_m3, screw_m3 = _nut_and_clearance(p.pcb.bolt_size, p.pcb.bolt_length)

    # Lid outer dimensions match the body outer footprint
    lid_w = e.width
    lid_d = e.depth
    lid_t = e.wall + 1.0   # slightly thicker than walls for stiffness

    # Skirt is a hollow downward extension that fits just inside the body rim
    skirt_wall   = e.wall
    skirt_depth  = e.lid_skirt
    skirt_clear  = e.lid_gap    # radial clearance for printability

    with BuildPart() as lid:

        # ── Top plate ────────────────────────────────────────────────────────
        Box(
            lid_w, lid_d, lid_t,
            align=(Align.CENTER, Align.CENTER, Align.MIN),
        )
        fillet(lid.edges().filter_by(Axis.Z), radius=e.corner_r)

        # ── Downward skirt ────────────────────────────────────────────────────
        # The skirt locates the lid over the body rim and prevents lateral
        # movement. Align.MAX makes the box top sit at Z=0 (the underside of
        # the lid plate), so the skirt hangs downward in negative Z.
        # The skirt outer face clears the body outer wall by lid_gap on each side.
        #
        # Build123d pattern: Align.MAX on the Z axis anchors the box's
        # maximum-Z face at the origin — useful for features that hang below
        # a reference surface.
        skirt_outer_w = lid_w - 2 * (e.wall + skirt_clear)
        skirt_outer_d = lid_d - 2 * (e.wall + skirt_clear)

        Box(
            skirt_outer_w,
            skirt_outer_d,
            skirt_depth,
            align=(Align.CENTER, Align.CENTER, Align.MAX),
        )
        # Hollow the skirt — hollow depth only (NOT lid_t) so the lid plate
        # stays solid. The +0 means the subtraction stops exactly at Z=0.
        Box(
            skirt_outer_w - 2 * skirt_wall,
            skirt_outer_d - 2 * skirt_wall,
            skirt_depth,                # same depth — does NOT enter lid plate
            align=(Align.CENTER, Align.CENTER, Align.MAX),
            mode=Mode.SUBTRACT,
        )

        # ── M3 lid-screw clearance holes ─────────────────────────────────────
        # Positions must exactly match the body boss centres computed in
        # build_enclosure_body (same wall_overlap and boss_od values).
        boss_od      = 10.0
        wall_overlap = 1.5
        off_x = lid_w / 2 - e.wall + wall_overlap - boss_od / 2
        off_y = lid_d / 2 - e.wall + wall_overlap - boss_od / 2

        for sx, sy in [(-1, -1), (1, -1), (-1, 1), (1, 1)]:
            with Locations(Location((sx * off_x, sy * off_y, lid_t))):
                ClearanceHole(fastener=screw_m3, fit="Normal", depth=lid_t)

        # ── Anemometer mount bolt pattern (centre of lid, forward of centre) ─
        # Build123d pattern: PolarLocations creates a circular bolt pattern
        # centred on the active location.
        anem_offset_y = lid_d * 0.15   # push slightly toward front
        nut_m4, screw_m4 = _nut_and_clearance(a.bolt_size, a.bolt_length)

        with Locations(Location((0, anem_offset_y, lid_t))):
            with PolarLocations(radius=a.bolt_pcd / 2, count=a.n_bolts):
                ClearanceHole(fastener=screw_m4, fit="Normal", depth=lid_t)

        # ── Solar bracket foot bolt pattern (rear half of lid) ────────────────
        solar_offset_y = -lid_d * 0.15
        with Locations(Location((0, solar_offset_y, lid_t))):
            with GridLocations(s.panel_hole_span_x / 2, 20, 2, 2):
                ClearanceHole(fastener=screw_m4, fit="Normal", depth=lid_t)

    return lid.part


# ══════════════════════════════════════════════════════════════════════════════
#  COMPONENT 3 — POLE CLAMP (single half — mirror for second half)
#  Build123d patterns demonstrated:
#    - Cylinder boolean subtraction to make a ring
#    - Box subtraction to cut the ring in half
#    - Additive ear tabs on split faces
#    - Captive nut pockets in ears
#    - mirror() to produce the matching half
# ══════════════════════════════════════════════════════════════════════════════

def build_pole_clamp_half(p: Params = PARAMS) -> Compound:
    """
    One half of the split-ring pole clamp.

    Mirror about the YZ plane to get the second half. The two halves bolt
    together around the pole with M5 socket-head cap screws threading into
    captive hex nuts in the ear tabs. A separate bolt pattern (not shown
    here — use four M4 screws) attaches the clamp to the back wall of the
    enclosure body.
    """
    pc = p.pole

    bore_r      = pc.pole_dia / 2 + pc.bore_clear   # inner bore radius
    outer_r     = bore_r + pc.ring_wall              # outer ring radius

    nut_m5, screw_m5 = _nut_and_clearance(pc.bolt_size, pc.bolt_length)

    with BuildPart() as half:

        # ── Full ring ─────────────────────────────────────────────────────────
        Cylinder(
            radius=outer_r,
            height=pc.band_width,
            align=(Align.CENTER, Align.CENTER, Align.MIN),
        )
        # Bore for pole
        Cylinder(
            radius=bore_r,
            height=pc.band_width,
            align=(Align.CENTER, Align.CENTER, Align.MIN),
            mode=Mode.SUBTRACT,
        )

        # ── Cut in half along the YZ plane (remove +X half) ──────────────────
        # Build123d pattern: subtracting an oversized Box is the simplest way
        # to bisect a solid. Align.MIN on X anchors the cut at X=0.
        Box(
            outer_r + 2,
            (outer_r + pc.ear_width) * 2 + 2,
            pc.band_width + 2,
            align=(Align.MIN, Align.CENTER, Align.MIN),
            mode=Mode.SUBTRACT,
        )

        # ── Ear tab on the cut face (+X side, ±Y ends of the half-ring) ──────
        # Two ear tabs protrude in +X from the flat split face.
        # Each ear receives one M5 captive nut that the mating half's bolt threads into.
        for ear_y in [-(bore_r + pc.ring_wall / 2), (bore_r + pc.ring_wall / 2)]:
            with Locations(Location((pc.ear_thick / 2, ear_y, pc.band_width / 2))):
                Box(
                    pc.ear_thick,
                    pc.ear_width,
                    pc.band_width,
                    align=(Align.CENTER, Align.CENTER, Align.CENTER),
                )
                # Captive M5 nut pocket — hex pocket + bolt clearance from +X face
                with Locations(Location((pc.ear_thick / 2, 0, 0))):
                    ClearanceHole(fastener=nut_m5, captive_nut=True, counter_sunk=False, depth=pc.ear_thick)

        # ── Flat mounting face for attachment to enclosure rear wall ─────────
        # Four M4 clearance holes on a rectangular pattern. The enclosure rear
        # wall has matching captive-nut bosses (add in future revision).
        mount_nut_m4, mount_screw_m4 = _nut_and_clearance("M4-0.7", 20.0)
        with Locations(Location((-outer_r, 0, pc.band_width / 2))):
            with GridLocations(0, pc.band_width * 0.5, 1, 2):
                ClearanceHole(
                    fastener=mount_screw_m4,
                    fit="Normal",
                    depth=p.enc.wall + 5,
                )

    return half.part


def build_pole_clamp(p: Params = PARAMS) -> Compound:
    """
    Complete split-ring clamp — both halves shown side-by-side.

    In the actual assembly one half mounts to the rear wall; the second half
    is a field-removable captive piece. This function produces both halves
    offset for visual clarity in the viewer.
    """
    half_a = build_pole_clamp_half(p)

    # Build123d pattern: mirror() reflects geometry about a named Plane.
    # The mirrored half is offset in X so both halves are visible.
    with BuildPart() as clamp:
        add(half_a)
        # Second half: mirror about YZ, then shift in +X for display gap
        with Locations(Location((p.pole.pole_dia + p.pole.ring_wall * 2 + 5, 0, 0))):
            add(half_a.mirror(Plane.YZ))

    return clamp.part


# ══════════════════════════════════════════════════════════════════════════════
#  COMPONENT 4 — SOLAR PANEL BRACKET
#  Build123d patterns demonstrated:
#    - Angled geometry using sin/cos derived dimensions
#    - Rising arm pair with a cross-member at the tilt angle
#    - Captive M4 nut pockets for panel fasteners
# ══════════════════════════════════════════════════════════════════════════════

def build_solar_bracket(p: Params = PARAMS) -> Compound:
    """
    South-facing solar panel bracket for top of enclosure lid.

    Two parallel arms rise from the lid surface and support a tilted mounting
    plate at PARAMS.solar.tilt_deg from horizontal. Captive M4 nuts in the
    plate accept the panel mounting bolts.

    Tilt default: 35° — tuned for Nampa ID (43.5 °N) to maximise
    year-round generation while tolerating summer heat angles.
    """
    s   = p.solar
    e   = p.enc

    tilt_rad   = math.radians(s.tilt_deg)
    arm_spacing = e.width * 0.6       # gap between the two arms (Y axis)
    foot_depth  = 30.0                # arm foot plate depth (Y)
    foot_height = e.wall + 3.0        # foot sits on lid top surface

    # Plate dimensions (the panel rests on this)
    plate_w = e.width * 0.8
    plate_d = s.arm_height / math.sin(tilt_rad) if math.sin(tilt_rad) > 0 else 100.0
    plate_t = s.arm_thickness

    nut_m4, screw_m4 = _nut_and_clearance(s.bolt_size, s.bolt_length)

    with BuildPart() as bracket:

        # ── Arm feet (two boxes sitting flat on lid surface) ──────────────────
        for arm_y in [-arm_spacing / 2, arm_spacing / 2]:
            with Locations(Location((0, arm_y, 0))):
                # Foot plate
                Box(
                    s.arm_thickness,
                    foot_depth,
                    foot_height,
                    align=(Align.CENTER, Align.CENTER, Align.MIN),
                )
                # Vertical riser from foot
                Box(
                    s.arm_thickness,
                    s.arm_thickness,
                    s.arm_height,
                    align=(Align.CENTER, Align.CENTER, Align.MIN),
                )

        # ── Tilted panel-mounting plate ───────────────────────────────────────
        # Position: base of the plate sits at the top of the arms.
        # Rotation: tilt forward (toward south = -Y direction) by tilt_deg.
        plate_loc = Location(
            (0, 0, s.arm_height),
            (1, 0, 0),          # rotate about X axis
            -s.tilt_deg,        # negative = tip the top away (south-facing)
        )
        with Locations(plate_loc):
            Box(
                plate_w,
                plate_d,
                plate_t,
                align=(Align.CENTER, Align.MIN, Align.CENTER),
            )
            # Captive M4 nut pockets on panel mounting hole pattern
            with GridLocations(s.panel_hole_span_x, s.panel_hole_span_y, 2, 2):
                ClearanceHole(fastener=nut_m4, captive_nut=True, counter_sunk=False, depth=plate_t)

        # ── Foot bolt holes (match lid solar bolt pattern) ────────────────────
        foot_bolt_y_offset = -p.enc.depth * 0.15
        with Locations(Location((0, foot_bolt_y_offset, 0))):
            with GridLocations(s.panel_hole_span_x / 2, 20, 2, 2):
                ClearanceHole(fastener=screw_m4, fit="Normal", depth=foot_height)

    return bracket.part


# ══════════════════════════════════════════════════════════════════════════════
#  COMPONENT 5 — ANEMOMETER MOUNT
#  Build123d patterns demonstrated:
#    - PolarLocations for a circular bolt pattern
#    - Hollow post stub (cylinder - cylinder)
# ══════════════════════════════════════════════════════════════════════════════

def build_anemometer_mount(p: Params = PARAMS) -> Compound:
    """
    Vertical post stub for a standard 1-inch cup anemometer.

    A circular flange bolts to the lid via four M4 captive nuts; the post
    stub protrudes upward to accept the anemometer's mounting collar.
    Post diameter is 25.4 mm (1 inch) — fits Davis, Inspeed, and generic
    consumer anemometers.
    """
    a = p.anem
    nut_m4, screw_m4 = _nut_and_clearance(a.bolt_size, a.bolt_length)

    with BuildPart() as mount:

        # ── Mounting flange ────────────────────────────────────────────────────
        Cylinder(
            radius=a.base_dia / 2,
            height=a.base_thick,
            align=(Align.CENTER, Align.CENTER, Align.MIN),
        )

        # ── Captive M4 nut pockets on PolarLocations bolt circle ──────────────
        # Build123d pattern: PolarLocations distributes n_bolts locations
        # evenly around a circle of the given radius.
        with Locations(Location((0, 0, a.base_thick))):
            with PolarLocations(radius=a.bolt_pcd / 2, count=a.n_bolts):
                ClearanceHole(fastener=nut_m4, captive_nut=True, counter_sunk=False, depth=a.base_thick)

        # ── Post stub ──────────────────────────────────────────────────────────
        Cylinder(
            radius=a.post_dia / 2,
            height=a.post_height,
            align=(Align.CENTER, Align.CENTER, Align.MIN),
        )
        # Hollow interior reduces weight and material; 2 mm wall in the stub
        Cylinder(
            radius=a.post_dia / 2 - 2.0,
            height=a.post_height - a.base_thick,
            align=(Align.CENTER, Align.CENTER, Align.MIN),
            mode=Mode.SUBTRACT,
        )

        # Chamfer the top opening of the post for easy insertion
        chamfer(mount.faces().sort_by(Axis.Z)[-1].edges(), length=1.5)

    return mount.part


# ══════════════════════════════════════════════════════════════════════════════
#  COMPONENT 6 — RAIN SENSOR BRACKET
#  Build123d patterns demonstrated:
#    - L-bracket via two perpendicular Box additions
#    - Slight tilt via angled mounting face
#    - GridLocations for sensor mounting hole pattern
# ══════════════════════════════════════════════════════════════════════════════

def build_rain_sensor_bracket(p: Params = PARAMS) -> Compound:
    """
    L-bracket for a raindrop or tipping-bucket rain sensor.

    A vertical back plate bolts to the enclosure side wall (M3 captive nuts
    in wall). A horizontal shelf protrudes outward with a slight forward tilt
    so rainwater drains off the sensor face. Four M3 clearance holes in the
    shelf match a standard 60 × 40 mm sensor PCB footprint.
    """
    r   = p.rain
    e   = p.enc
    nut_m3, screw_m3 = _nut_and_clearance(r.bolt_size, r.bolt_length)

    tilt_rad  = math.radians(r.tilt_deg)
    shelf_len = 45.0   # how far the shelf extends from wall
    back_h    = 50.0   # back plate height

    with BuildPart() as bracket:

        # ── Back plate (bolts to side wall of enclosure) ──────────────────────
        Box(
            r.plate_thick,
            r.plate_width,
            back_h,
            align=(Align.MIN, Align.CENTER, Align.MIN),
        )

        # M3 clearance holes for wall attachment (two holes vertically)
        with Locations(Location((r.plate_thick, 0, back_h / 2))):
            with GridLocations(0, back_h * 0.5, 1, 2):
                ClearanceHole(fastener=screw_m3, fit="Normal", depth=r.plate_thick)

        # ── Shelf (sensor rests on this) ──────────────────────────────────────
        # Slight tilt so rain drains off the sensor surface.
        shelf_loc = Location(
            (r.plate_thick, 0, back_h * 0.5),
            (0, 1, 0),        # rotate about Y axis
            r.tilt_deg,       # tip the shelf forward
        )
        with Locations(shelf_loc):
            Box(
                shelf_len,
                r.plate_width - 4,
                r.plate_thick,
                align=(Align.MIN, Align.CENTER, Align.CENTER),
            )
            # Sensor PCB mounting holes (standard 60 × 40 mm grid)
            with GridLocations(r.plate_depth * 0.8, r.plate_width * 0.6, 2, 2):
                ClearanceHole(fastener=screw_m3, fit="Normal", depth=r.plate_thick)

    return bracket.part


# ══════════════════════════════════════════════════════════════════════════════
#  FULL ASSEMBLY (exploded view)
#  Build123d pattern: add() inserts existing Compound objects into the active
#  BuildPart. Applying a Location offset to each add() produces an exploded
#  view for visual inspection in ocp_vscode without affecting individual exports.
# ══════════════════════════════════════════════════════════════════════════════

def build_assembly(p: Params = PARAMS, explode: bool = True) -> Compound:
    """
    Full FlintMesh enclosure assembly.

    Set explode=False for a collapsed (as-installed) view.
    Set explode=True (default) to see all components separated for inspection.
    """
    e = p.enc

    body    = build_enclosure_body(p)
    lid     = build_lid(p)
    clamp   = build_pole_clamp(p)
    solar   = build_solar_bracket(p)
    anem    = build_anemometer_mount(p)
    rain    = build_rain_sensor_bracket(p)

    # Explode offsets — how far each component is displaced for display
    explode_gap = 30.0 if explode else 0.0

    with BuildPart() as assembly:

        # Body at origin
        add(body)

        # Lid floats above body
        add(lid.moved(Location((0, 0, e.height + explode_gap))))

        # Solar bracket on top of lid
        add(solar.moved(Location((0, 0, e.height + e.wall + 1 + explode_gap * 2))))

        # Anemometer mount centred on lid
        add(anem.moved(Location((0, e.depth * 0.15, e.height + e.wall + 1 + explode_gap * 2))))

        # Pole clamp at rear, centred on body height
        add(clamp.moved(Location((-e.width / 2 - 10 - explode_gap, 0, e.height / 2))))

        # Rain sensor bracket on side wall
        add(rain.moved(Location((e.width / 2 + explode_gap, 0, e.height * 0.3))))

    return assembly.part


# ══════════════════════════════════════════════════════════════════════════════
#  EXPORT
# ══════════════════════════════════════════════════════════════════════════════

def export_all(output_dir: Path = Path(__file__).parent / "exports") -> None:
    """
    Build every component and export STL + STEP files to *output_dir*.

    STL is used for 3D printing. STEP preserves exact B-rep geometry for
    import into Fusion 360, FreeCAD, or for sharing with a machinist.
    """
    output_dir.mkdir(parents=True, exist_ok=True)

    components = {
        "enclosure_body":      build_enclosure_body,
        "lid":                 build_lid,
        "pole_clamp_half":     build_pole_clamp_half,
        "pole_clamp_assembly": build_pole_clamp,
        "solar_bracket":       build_solar_bracket,
        "anemometer_mount":    build_anemometer_mount,
        "rain_sensor_bracket": build_rain_sensor_bracket,
        "assembly_exploded":   lambda p: build_assembly(p, explode=True),
    }

    for name, builder in components.items():
        print(f"Building {name}...")
        part = builder(PARAMS)

        stl_path  = output_dir / f"{name}.stl"
        step_path = output_dir / f"{name}.step"

        export_stl(part, str(stl_path))
        export_step(part, str(step_path))
        print(f"  → {stl_path}")
        print(f"  → {step_path}")

    print("\nAll exports complete.")


# ══════════════════════════════════════════════════════════════════════════════
#  ENTRY POINT
# ══════════════════════════════════════════════════════════════════════════════

if __name__ == "__main__":
    export_all()
