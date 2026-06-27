fn main() {
    // Required to link the ESP32 memory layout provided by esp-hal.
    println!("cargo:rustc-link-arg-bins=-Tlinkall.x");
}
