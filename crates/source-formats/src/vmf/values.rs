//! Text encodings of the small value types that appear inside VMF strings.

use std::fmt::{self, Write as _};

use glam::DVec3;

/// Formats a number the way it should appear in a VMF: integers without a
/// fraction, everything else with the shortest representation that parses
/// back to the same `f64`.
pub fn format_number(value: f64) -> String {
    let mut out = String::new();
    push_number(&mut out, value);
    out
}

fn push_number(out: &mut String, value: f64) {
    write!(out, "{value}").expect("writing to a String cannot fail");
}

fn push_vec3(out: &mut String, v: DVec3) {
    push_number(out, v.x);
    out.push(' ');
    push_number(out, v.y);
    out.push(' ');
    push_number(out, v.z);
}

/// Parses whitespace-separated numbers, ignoring `(`, `)`, `[` and `]`.
pub fn parse_numbers(s: &str) -> Option<Vec<f64>> {
    s.split(|c: char| c.is_ascii_whitespace() || matches!(c, '(' | ')' | '[' | ']'))
        .filter(|t| !t.is_empty())
        .map(|t| t.parse().ok())
        .collect()
}

/// Parses `x y z`, optionally wrapped in `()` or `[]`.
pub fn parse_vec3(s: &str) -> Option<DVec3> {
    match parse_numbers(s)?.as_slice() {
        &[x, y, z] => Some(DVec3::new(x, y, z)),
        _ => None,
    }
}

pub fn format_vec3_bracketed(v: DVec3) -> String {
    let mut out = String::from("[");
    push_vec3(&mut out, v);
    out.push(']');
    out
}

pub fn parse_vec3_list(s: &str) -> Option<Vec<DVec3>> {
    let numbers = parse_numbers(s)?;
    if numbers.len() % 3 != 0 {
        return None;
    }
    Some(
        numbers
            .chunks_exact(3)
            .map(|c| DVec3::new(c[0], c[1], c[2]))
            .collect(),
    )
}

pub fn format_vec3_list(values: &[DVec3]) -> String {
    let mut out = String::new();
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        push_vec3(&mut out, *v);
    }
    out
}

pub fn format_number_list(values: &[f64]) -> String {
    let mut out = String::new();
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        push_number(&mut out, *v);
    }
    out
}

pub fn parse_int_list(s: &str) -> Option<Vec<i32>> {
    s.split_ascii_whitespace().map(|t| t.parse().ok()).collect()
}

pub fn format_int_list(values: &[i32]) -> String {
    let mut out = String::new();
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        write!(out, "{v}").expect("writing to a String cannot fail");
    }
    out
}

/// A brush face plane given by three points, as stored in `side.plane`.
/// Hammer orders the points clockwise when viewed from outside the brush.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane(pub [DVec3; 3]);

impl Plane {
    pub fn parse(s: &str) -> Option<Self> {
        match parse_numbers(s)?.as_slice() {
            &[ax, ay, az, bx, by, bz, cx, cy, cz] => Some(Self([
                DVec3::new(ax, ay, az),
                DVec3::new(bx, by, bz),
                DVec3::new(cx, cy, cz),
            ])),
            _ => None,
        }
    }
}

impl fmt::Display for Plane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        for (i, p) in self.0.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push('(');
            push_vec3(&mut out, *p);
            out.push(')');
        }
        f.write_str(&out)
    }
}

/// A texture projection axis, stored as `[x y z offset] scale`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvAxis {
    pub axis: DVec3,
    pub offset: f64,
    pub scale: f64,
}

impl UvAxis {
    pub fn parse(s: &str) -> Option<Self> {
        match parse_numbers(s)?.as_slice() {
            &[x, y, z, offset, scale] => Some(Self {
                axis: DVec3::new(x, y, z),
                offset,
                scale,
            }),
            _ => None,
        }
    }
}

impl fmt::Display for UvAxis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::from("[");
        push_vec3(&mut out, self.axis);
        out.push(' ');
        push_number(&mut out, self.offset);
        out.push_str("] ");
        push_number(&mut out, self.scale);
        f.write_str(&out)
    }
}

/// An 8-bit RGB color, stored as `r g b`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub [u8; 3]);

impl Color {
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.split_ascii_whitespace().map(|t| t.parse::<u8>().ok());
        let color = [it.next()??, it.next()??, it.next()??];
        it.next().is_none().then_some(Self(color))
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [r, g, b] = self.0;
        write!(f, "{r} {g} {b}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_round_trips() {
        let text = "(-1568 1056 64) (-1568 864 64) (-1600 864.5 64)";
        let plane = Plane::parse(text).unwrap();
        assert_eq!(plane.0[2], DVec3::new(-1600.0, 864.5, 64.0));
        assert_eq!(plane.to_string(), text);
    }

    #[test]
    fn uv_axis_round_trips() {
        let text = "[0 -1 0 12.5] 0.25";
        assert_eq!(UvAxis::parse(text).unwrap().to_string(), text);
    }

    #[test]
    fn accepts_msvc_exponents() {
        let v = parse_vec3("-0.0871556 0.996195 -1.21669e-008").unwrap();
        assert_eq!(v.z, -1.21669e-8);
    }

    #[test]
    fn color_rejects_out_of_range() {
        assert_eq!(Color::parse("92 93 178"), Some(Color([92, 93, 178])));
        assert_eq!(Color::parse("92 93 300"), None);
        assert_eq!(Color::parse("92 93"), None);
    }
}
