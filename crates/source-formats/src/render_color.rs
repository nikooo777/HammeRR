use std::io;

pub fn rgb(value: Option<&str>) -> io::Result<[u8; 3]> {
    let Some(value) = value else {
        return Ok([255; 3]);
    };
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Model tint needs three integer RGB channels from 0 to 255; an optional fourth component is preserved without opacity preview",
        )
    };
    let mut components = value.split_whitespace();
    let mut color = [0; 3];
    for channel in &mut color {
        *channel = components
            .next()
            .ok_or_else(invalid)?
            .parse()
            .map_err(|_| invalid())?;
    }
    components.next();
    if components.next().is_some() {
        return Err(invalid());
    }
    Ok(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_colors_read_rgb_without_changing_an_optional_fourth_component() {
        assert_eq!(rgb(None).unwrap(), [255; 3]);
        assert_eq!(rgb(Some("12 34 56")).unwrap(), [12, 34, 56]);
        assert_eq!(rgb(Some("12 34 56 064")).unwrap(), [12, 34, 56]);
        for value in [
            "",
            "1 2",
            "1 2 3 4 5",
            "-1 0 0",
            "256 0 0",
            "0.5 0 0",
            "nan 0 0",
        ] {
            assert!(rgb(Some(value)).is_err(), "{value}");
        }
    }
}
