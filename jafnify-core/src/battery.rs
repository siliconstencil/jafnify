//! Battery and temperature wording. Values only; no action, no history.

/// At or below this percentage the battery line says "low".
pub const LOW_PCT: u8 = 20;
/// At or above 40.0 C the phone is reported as hot.
pub const HOT_C_X10: i16 = 400;
/// At or above 45.0 C the temperature line replaces the hot line.
pub const CRITICAL_C_X10: i16 = 450;

/// The battery line for a known 0-100 percentage; 255 and None are unknown.
pub fn level_line(level: Option<u8>) -> Option<String> {
    let level = level.filter(|value| *value <= 100)?;
    if level <= LOW_PCT {
        Some(format!("Phone battery is low: {level}%."))
    } else {
        Some(format!("Phone battery: {level}%."))
    }
}

/// The temperature line. 40.0 C and above is hot, 45.0 C and above is the
/// critical wording, below 40.0 C is a normal value.
pub fn temperature_line(temp_c_x10: Option<i16>) -> Option<String> {
    let temp = temp_c_x10?;
    if temp == i16::MIN {
        return None;
    }
    let text = temp_text(temp);
    if temp >= CRITICAL_C_X10 {
        Some(format!("Phone temperature: {text}."))
    } else if temp >= HOT_C_X10 {
        Some(format!("Phone is hot: {text}."))
    } else {
        Some(format!("Phone temperature: {text}."))
    }
}

/// The battery line first, then the temperature line.
pub fn lines(level: Option<u8>, temp_c_x10: Option<i16>) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(line) = level_line(level) {
        lines.push(line);
    }
    if let Some(line) = temperature_line(temp_c_x10) {
        lines.push(line);
    }
    lines
}

/// The temperature in the user-visible form, "39.5 C".
pub fn temp_text(temp_c_x10: i16) -> String {
    let tenths = i32::from(temp_c_x10);
    if tenths % 10 == 0 {
        format!("{} C", tenths / 10)
    } else {
        format!("{:.1} C", f64::from(tenths) / 10.0)
    }
}
