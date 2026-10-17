//! Display units. Designs are always stored in millimetres; the workspace
//! units and scale only change how lengths are shown and typed.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Units {
    #[default]
    Metric,
    Imperial,
}

/// What one displayed unit is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Mm,
    Cm,
    M,
    Inch,
    Foot,
}

impl Unit {
    pub fn mm(self) -> f64 {
        match self {
            Unit::Mm => 1.0,
            Unit::Cm => 10.0,
            Unit::M => 1000.0,
            Unit::Inch => 25.4,
            Unit::Foot => 304.8,
        }
    }
    pub fn suffix(self) -> &'static str {
        match self {
            Unit::Mm => "mm",
            Unit::Cm => "cm",
            Unit::M => "m",
            Unit::Inch => "in",
            Unit::Foot => "ft",
        }
    }
    /// Grid spacing in mm: (minor, major).
    pub fn grid(self) -> (f64, f64) {
        match self {
            Unit::Mm => (1.0, 10.0),
            Unit::Cm => (10.0, 100.0),
            Unit::M => (100.0, 1000.0),
            Unit::Inch => (25.4 / 8.0, 25.4),
            Unit::Foot => (25.4, 304.8),
        }
    }
    /// Snap grid choices, in display units (0 = off).
    pub fn snaps(self) -> &'static [f64] {
        match self {
            Unit::Mm => &[0.0, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0],
            Unit::Cm => &[0.0, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0],
            Unit::M => &[0.0, 0.001, 0.01, 0.05, 0.1, 0.5, 1.0],
            Unit::Inch => &[0.0, 1.0 / 64.0, 1.0 / 32.0, 1.0 / 16.0, 0.125, 0.25, 0.5, 1.0],
            Unit::Foot => &[0.0, 1.0 / 48.0, 1.0 / 12.0, 0.25, 0.5, 1.0],
        }
    }
    /// Default snap, in mm.
    pub fn default_snap(self) -> f64 {
        match self {
            Unit::Mm => 1.0,
            Unit::Cm => 1.0,
            Unit::M => 10.0,
            Unit::Inch => 25.4 / 8.0,
            Unit::Foot => 25.4,
        }
    }
    fn index(self) -> u8 {
        self as u8
    }
    fn from_index(i: u8) -> Unit {
        [Unit::Mm, Unit::Cm, Unit::M, Unit::Inch, Unit::Foot][i.min(4) as usize]
    }
}

/// Scale choices for a unit system: (scale value, label, unit).
pub fn scales(units: Units) -> &'static [(u32, &'static str, Unit)] {
    match units {
        Units::Metric => &[
            (1, "1:1 (millimetres, default)", Unit::Mm),
            (10, "1:10 (centimetres)", Unit::Cm),
            (1000, "1:1000 (metres)", Unit::M),
        ],
        Units::Imperial => &[(1, "1:1 (inches, default)", Unit::Inch), (12, "1:12 (feet)", Unit::Foot)],
    }
}

pub fn unit_for(units: Units, scale: u32) -> Unit {
    scales(units).iter().find(|s| s.0 == scale).map(|s| s.2).unwrap_or(scales(units)[0].2)
}

// The current display unit, set once per frame from the document settings.
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set(u: Unit) {
    CURRENT.store(u.index(), Ordering::Relaxed);
}

pub fn current() -> Unit {
    Unit::from_index(CURRENT.load(Ordering::Relaxed))
}

/// mm -> display value.
pub fn to_disp(mm: f64) -> f64 {
    mm / current().mm()
}

/// display value -> mm.
pub fn from_disp(v: f64) -> f64 {
    v * current().mm()
}

/// Decimals that make sense for the current unit.
pub fn decimals() -> usize {
    match current() {
        Unit::Mm => 2,
        Unit::Cm | Unit::Inch => 3,
        Unit::M | Unit::Foot => 4,
    }
}

/// "12.50 mm"
pub fn fmt(mm: f64) -> String {
    format!("{:.*} {}", decimals(), to_disp(mm), current().suffix())
}

/// "12.50" (no suffix)
pub fn num(mm: f64) -> String {
    format!("{:.*}", decimals(), to_disp(mm))
}

/// "+12.50"
pub fn signed(mm: f64) -> String {
    let v = to_disp(mm);
    let v = if v.abs() < 0.5 * 10f64.powi(-(decimals() as i32)) { 0.0 } else { v };
    format!("{v:+.*}", decimals())
}

/// Label for a snap value given in display units.
pub fn snap_label(v: f64) -> String {
    if v == 0.0 {
        return "Off".into();
    }
    if matches!(current(), Unit::Inch | Unit::Foot) {
        // Show common fractions.
        for den in [1u32, 2, 4, 8, 12, 16, 32, 48, 64] {
            let n = v * den as f64;
            if (n - n.round()).abs() < 1e-9 && n.round() >= 1.0 {
                let n = n.round() as u32;
                return if den == 1 {
                    format!("{n} {}", current().suffix())
                } else {
                    format!("{n}/{den} {}", current().suffix())
                };
            }
        }
    }
    let s = format!("{v}");
    format!("{s} {}", current().suffix())
}

/// Parse a typed length in display units. Accepts plain numbers, simple
/// fractions ("1/8") and a trailing unit ("12mm", "0.5 in", "2cm").
pub fn parse(s: &str) -> Option<f64> {
    let t = s.trim().to_lowercase();
    let units = [("mm", 1.0), ("cm", 10.0), ("in", 25.4), ("\"", 25.4), ("ft", 304.8), ("m", 1000.0)];
    for (suf, mm) in units {
        if let Some(n) = t.strip_suffix(suf) {
            return parse_num(n.trim()).map(|v| v * mm);
        }
    }
    parse_num(&t).map(from_disp)
}

fn parse_num(s: &str) -> Option<f64> {
    if let Some((a, b)) = s.split_once('/') {
        // "1 1/2" or "1/2"
        let (whole, num) = match a.trim().rsplit_once(' ') {
            Some((w, n)) => (w.trim().parse::<f64>().ok()?, n.trim().parse::<f64>().ok()?),
            None => (0.0, a.trim().parse::<f64>().ok()?),
        };
        let den: f64 = b.trim().parse().ok()?;
        if den == 0.0 {
            return None;
        }
        return Some(whole + num / den);
    }
    s.replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite())
}

// ---------------------------------------------------------------------------
// Printer presets (bed width x depth, mm)
// ---------------------------------------------------------------------------

pub const PRESETS: &[(&str, f64, f64)] = &[
    ("Custom (default)", 300.0, 300.0),
    ("Bambu Lab A1 mini", 180.0, 180.0),
    ("Bambu Lab A1", 256.0, 256.0),
    ("Bambu Lab P1S / P1P / P2S", 256.0, 256.0),
    ("Bambu Lab X1 Carbon", 256.0, 256.0),
    ("Bambu Lab H2D / H2S", 350.0, 320.0),
    ("Prusa MINI+", 180.0, 180.0),
    ("Prusa MK4S", 250.0, 210.0),
    ("Prusa CORE One", 250.0, 220.0),
    ("Prusa XL", 360.0, 360.0),
    ("Creality Ender-3 V3 SE", 220.0, 220.0),
    ("Creality K1 / K1C", 220.0, 220.0),
    ("Creality K2", 260.0, 260.0),
    ("Creality K1 Max / K2 Pro", 300.0, 300.0),
    ("Creality K2 Plus", 350.0, 350.0),
    ("Elegoo Neptune 4 Pro", 225.0, 225.0),
    ("Anycubic Kobra 3", 250.0, 250.0),
    ("Voron 2.4 (350)", 350.0, 350.0),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing() {
        set(Unit::Mm);
        assert_eq!(parse("12.5"), Some(12.5));
        assert_eq!(parse("1 in"), Some(25.4));
        assert_eq!(parse("2cm"), Some(20.0));
        set(Unit::Inch);
        assert_eq!(parse("1/2"), Some(12.7));
        assert!((parse("1 1/2").unwrap() - 38.1).abs() < 1e-9);
        assert_eq!(snap_label(0.125), "1/8 in");
        set(Unit::Mm);
        assert_eq!(parse("abc"), None);
    }
}
