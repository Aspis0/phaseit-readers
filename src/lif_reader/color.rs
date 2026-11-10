/// Map a Leica LUT name to a display RGB, best-effort (metadata only).
pub(super) fn lut_color(name: &str) -> Option<[u8; 3]> {
    match name.to_ascii_lowercase().as_str() {
        "red" => Some([255, 0, 0]),
        "green" => Some([0, 255, 0]),
        "blue" => Some([0, 0, 255]),
        "cyan" => Some([0, 255, 255]),
        "magenta" => Some([255, 0, 255]),
        "yellow" => Some([255, 255, 0]),
        "gray" | "grey" => Some([255, 255, 255]),
        _ => None,
    }
}
