.PHONY: all build flash clean check \
        node node-release node-flash node-erase \
        bridge bridge-release bridge-flash \
        gateway gateway-release \
        ui ui-release \
        hardware hardware-export

# ── Default ───────────────────────────────────────────────────────────────────

all build: node bridge gateway ui

# ── flint-node (RAK4631 / nRF52840, thumbv7em-none-eabihf) ───────────────────

node:
	cargo build -p flint-node

node-release:
	cargo build -p flint-node --release

node-flash:
	cargo run -p flint-node

# One-time per board: wipe the RAK4631 factory bootloader + SoftDevice + UICR and
# clear APPROTECT (nRF ERASEALL) so the app at 0x0 boots on every power-up/reset,
# not just under the debugger. Run once before the first `node-flash`.
node-erase:
	probe-rs erase --chip nRF52840_xxAA

# ── flint-bridge (Heltec V2 / ESP32 Xtensa, xtensa-esp32-none-elf) ───────────

bridge:
	cd flint-bridge && cargo build

bridge-release:
	cd flint-bridge && cargo build --release

bridge-flash:
	cd flint-bridge && cargo run --release

# ── flint-gateway (host) ──────────────────────────────────────────────────────

gateway:
	cd flint-gateway && cargo build

gateway-release:
	cd flint-gateway && cargo build --release

# ── flint-ui (host, stable toolchain) ────────────────────────────────────────

ui:
	cd flint-ui && cargo build

ui-release:
	cd flint-ui && cargo build --release

# ── flint-hardware (CadQuery / build123d, uv-managed) ────────────────────────

hardware:
	cd flint-hardware && uv run python main.py

hardware-export:
	cd flint-hardware && uv run python enclosure/flint_enclosure.py

# ── Workspace-wide ────────────────────────────────────────────────────────────

check:
	cargo check -p flint-proto -p flint-node
	cd flint-bridge && cargo check
	cd flint-gateway && cargo check
	cd flint-ui && cargo check

clean:
	cargo clean
	cd flint-bridge && cargo clean
	cd flint-ui && cargo clean

