use map_core::ObjectId;

pub fn id_string(id: ObjectId) -> String {
    match id {
        ObjectId::Solid(n) => format!("solid:{n}"),
        ObjectId::Entity(n) => format!("entity:{n}"),
    }
}

pub fn parse_id(text: &str) -> Result<ObjectId, String> {
    let bad = || format!("bad object id {text:?}; expected 'solid:<n>' or 'entity:<n>'");
    let (kind, number) = text.trim().split_once([':', ' ']).ok_or_else(bad)?;
    let number = number.trim().parse().map_err(|_| bad())?;
    match kind.to_ascii_lowercase().as_str() {
        "solid" | "brush" => Ok(ObjectId::Solid(number)),
        "entity" => Ok(ObjectId::Entity(number)),
        _ => Err(bad()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for id in [ObjectId::Solid(12), ObjectId::Entity(7)] {
            assert_eq!(parse_id(&id_string(id)), Ok(id));
        }
        assert_eq!(parse_id("entity 7"), Ok(ObjectId::Entity(7)));
        assert!(parse_id("7").is_err());
        assert!(parse_id("prop:7").is_err());
    }
}
