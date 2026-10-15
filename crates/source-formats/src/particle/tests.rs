use super::*;

fn string(data: &mut Vec<u8>, value: &str) {
    data.extend(value.as_bytes());
    data.push(0);
}

fn fixture() -> Vec<u8> {
    let mut data = b"<!-- dmx encoding binary 2 format pcf 1 -->\n\0".to_vec();
    let dictionary = [
        "DmElement",
        "DmeParticleSystemDefinition",
        "DmeParticleOperator",
        "DmeParticleChild",
        "particleSystemDefinitions",
        "material",
        "max_particles",
        "emitters",
        "functionName",
        "children",
        "child",
    ];
    data.extend((dictionary.len() as i16).to_le_bytes());
    for value in dictionary {
        string(&mut data, value);
    }
    data.extend(5i32.to_le_bytes());
    for (kind, name) in [
        (0i16, "root"),
        (1, "test_flame"),
        (2, "Continuous emission"),
        (3, "test_child"),
        (1, "test_child"),
    ] {
        data.extend(kind.to_le_bytes());
        string(&mut data, name);
        data.extend([0; 16]);
    }
    data.extend(1i32.to_le_bytes());
    data.extend(4i16.to_le_bytes());
    data.push(15);
    data.extend(2i32.to_le_bytes());
    data.extend(1i32.to_le_bytes());
    data.extend(4i32.to_le_bytes());
    data.extend(4i32.to_le_bytes());
    data.extend(5i16.to_le_bytes());
    data.push(5);
    string(&mut data, "particle/fire.vmt");
    data.extend(6i16.to_le_bytes());
    data.push(2);
    data.extend(100i32.to_le_bytes());
    data.extend(7i16.to_le_bytes());
    data.push(15);
    data.extend(1i32.to_le_bytes());
    data.extend(2i32.to_le_bytes());
    data.extend(9i16.to_le_bytes());
    data.push(15);
    data.extend(1i32.to_le_bytes());
    data.extend(3i32.to_le_bytes());
    data.extend(1i32.to_le_bytes());
    data.extend(8i16.to_le_bytes());
    data.push(5);
    string(&mut data, "emit_continuously");
    data.extend(1i32.to_le_bytes());
    data.extend(10i16.to_le_bytes());
    data.push(1);
    data.extend(4i32.to_le_bytes());
    data.extend(0i32.to_le_bytes());
    data
}

#[test]
fn parses_structured_names_materials_functions_and_child_references() {
    let file = parse(&fixture()).unwrap();
    assert_eq!(file.elements, 5);
    assert_eq!(file.definitions.len(), 2);
    let definition = &file.definitions[0];
    assert_eq!(definition.name, "test_flame");
    assert_eq!(definition.material.as_deref(), Some("particle/fire.vmt"));
    assert_eq!(definition.max_particles, Some(100));
    assert_eq!(definition.functions["emitters"], ["emit_continuously"]);
    assert_eq!(definition.children, ["test_child"]);
}

#[test]
fn rejects_every_truncation_bad_indices_counts_types_and_trailing_bytes() {
    let data = fixture();
    for length in 0..data.len() {
        assert!(parse(&data[..length]).is_err(), "prefix {length}");
    }
    let header = b"<!-- dmx encoding binary 2 format pcf 1 -->\n\0".len();
    for value in [-1i16, i16::MAX] {
        let mut corrupt = data.clone();
        corrupt[header..header + 2].copy_from_slice(&value.to_le_bytes());
        assert!(parse(&corrupt).is_err());
    }
    let mut reader = Reader {
        data: &data,
        offset: header,
    };
    let count = reader.short().unwrap();
    for _ in 0..count {
        reader.string().unwrap();
    }
    let element_count = reader.offset;
    for value in [-1i32, (MAX_ELEMENTS + 1) as i32] {
        let mut corrupt = data.clone();
        corrupt[element_count..element_count + 4].copy_from_slice(&value.to_le_bytes());
        assert!(parse(&corrupt).is_err());
    }
    let mut corrupt = data.clone();
    corrupt[element_count + 4..element_count + 6].copy_from_slice(&i16::MAX.to_le_bytes());
    assert!(parse(&corrupt).is_err());
    reader.int().unwrap();
    for _ in 0..5 {
        reader.short().unwrap();
        reader.string().unwrap();
        reader.take(16).unwrap();
    }
    let attributes = reader.offset;
    let mut corrupt = data.clone();
    corrupt[attributes..attributes + 4].copy_from_slice(&(-1i32).to_le_bytes());
    assert!(parse(&corrupt).is_err());
    let mut corrupt = data.clone();
    corrupt[attributes + 6] = 255;
    assert!(parse(&corrupt).is_err());
    let mut corrupt = data.clone();
    corrupt[attributes + 7..attributes + 11].copy_from_slice(&i32::MAX.to_le_bytes());
    assert!(parse(&corrupt).is_err());
    for value in [-2i32, 5] {
        let mut corrupt = data.clone();
        corrupt[attributes + 11..attributes + 15].copy_from_slice(&value.to_le_bytes());
        assert!(parse(&corrupt).is_err());
    }
    let mut corrupt = data;
    corrupt.push(0);
    assert!(parse(&corrupt).is_err());
}

#[test]
fn rejects_unstructured_unsupported_or_oversize_data() {
    assert!(parse(b"DmeParticleSystemDefinition\0guessed_effect\0").is_err());
    for header in [
        "<!-- dmx encoding binary 5 format pcf 1 -->\n\0",
        "<!-- dmx encoding keyvalues2 1 format pcf 1 -->\n\0",
        "<!-- dmx encoding binary 2 format dmx 1 -->\n\0",
    ] {
        assert!(parse(header.as_bytes()).is_err());
    }
    let mut data = fixture();
    data.resize(MAX_FILE_BYTES + 1, 0);
    assert!(parse(&data).is_err());
    let mut string_data = b"<!-- dmx encoding binary 2 format pcf 1 -->\n\0".to_vec();
    string_data.extend(1i16.to_le_bytes());
    string_data.extend(vec![b'a'; MAX_STRING + 1]);
    string_data.push(0);
    assert!(parse(&string_data).is_err());
}

#[test]
fn manifest_checks_paths_precache_directives_and_structure() {
    let files =
        manifest(b"particles_manifest { file particles/one.pcf file !particles/two.pcf }").unwrap();
    assert_eq!(files[1].path, "particles/two.pcf");
    assert!(files[1].precache);
    assert!(!files[0].precache);
    for text in [
        "",
        "broken",
        "particles_manifest {}",
        "particles_manifest { file ../outside.pcf }",
        "particles_manifest { file particles/a.pcf file particles/a.pcf }",
        "particles_manifest { #include particles/extra.txt }",
        "particles_manifest { file particles/a.pcf",
        "particles_manifest { file /particles/a.pcf }",
        "particles_manifest { file particles/a.dmx }",
    ] {
        assert!(manifest(text.as_bytes()).is_err(), "{text}");
    }
}

#[test]
fn installed_css_active_manifest_uses_supported_structured_definitions() {
    let root = std::env::var_os("CSS_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| {
                std::path::PathBuf::from(home)
                    .join(".steam/steam/steamapps/common/Counter-Strike Source")
            })
        });
    let Some(path) = root
        .map(|root| root.join("cstrike/gameinfo.txt"))
        .filter(|path| path.is_file())
    else {
        return;
    };
    let game = crate::gamefs::GameFs::from_gameinfo(path).unwrap();
    let data = game
        .read_limited("particles/particles_manifest.txt", MAX_MANIFEST_BYTES)
        .unwrap()
        .unwrap();
    let mut names = BTreeSet::new();
    for file in manifest(&data).unwrap() {
        let data = game
            .read_limited(&file.path, MAX_FILE_BYTES)
            .unwrap()
            .unwrap();
        let parsed = parse(&data).unwrap_or_else(|error| panic!("{}: {error}", file.path));
        names.extend(
            parsed
                .definitions
                .into_iter()
                .map(|definition| definition.name),
        );
    }
    assert!(names.contains("error"));
    assert!(names.contains("blood_impact_red_01"));
    assert!(names.contains("water_splash_01"));
}

#[test]
fn typed_scalar_and_array_payloads_are_consumed_without_string_scanning() {
    let data = fixture();
    let mut reader = Reader {
        data: &data,
        offset: b"<!-- dmx encoding binary 2 format pcf 1 -->\n\0".len(),
    };
    let count = reader.short().unwrap();
    for _ in 0..count {
        reader.string().unwrap();
    }
    let count = reader.int().unwrap();
    for _ in 0..count {
        reader.short().unwrap();
        reader.string().unwrap();
        reader.take(16).unwrap();
    }
    let root = reader.offset;
    let insertion = root + 4 + 2 + 1 + 4 + 8;
    for kind in 1..=14u8 {
        if kind == 7 {
            continue;
        }
        let value = match kind {
            1 => (-1i32).to_le_bytes().to_vec(),
            2 => 17i32.to_le_bytes().to_vec(),
            3 => 1.5f32.to_le_bytes().to_vec(),
            4 => vec![1],
            5 => b"unreferenced_guessed_effect\0".to_vec(),
            6 => {
                let mut bytes = 3i32.to_le_bytes().to_vec();
                bytes.extend([0, 1, 2]);
                bytes
            }
            8 => vec![255; 4],
            9 => vec![0; 8],
            10 | 12 => vec![0; 12],
            11 | 13 => vec![0; 16],
            14 => vec![0; 64],
            _ => unreachable!(),
        };
        for array in [false, true] {
            let mut payload = 5i16.to_le_bytes().to_vec();
            payload.push(if array { kind + 14 } else { kind });
            if array {
                payload.extend(2i32.to_le_bytes());
                payload.extend(&value);
            }
            payload.extend(&value);
            let mut modified = data.clone();
            modified[root..root + 4].copy_from_slice(&2i32.to_le_bytes());
            modified.splice(insertion..insertion, payload);
            let file = parse(&modified)
                .unwrap_or_else(|error| panic!("type {kind}, array {array}: {error}"));
            assert_eq!(file.definitions.len(), 2);
            assert!(
                !file
                    .definitions
                    .iter()
                    .any(|definition| definition.name == "unreferenced_guessed_effect")
            );
        }
    }
    let mut payload = 5i16.to_le_bytes().to_vec();
    payload.push(6);
    payload.extend((-1i32).to_le_bytes());
    let mut corrupt = data;
    corrupt[root..root + 4].copy_from_slice(&2i32.to_le_bytes());
    corrupt.splice(insertion..insertion, payload);
    assert!(parse(&corrupt).is_err());
}

#[test]
fn repeated_function_references_use_indexed_metadata_at_the_value_budget() {
    let mut dictionary = vec![
        "DmElement".to_owned(),
        "DmeParticleSystemDefinition".into(),
        "DmeParticleOperator".into(),
        "particleSystemDefinitions".into(),
        "emitters".into(),
    ];
    dictionary.extend((0..511).map(|index| format!("padding{index:03}")));
    dictionary.push("functionName".into());
    let mut data = b"<!-- dmx encoding binary 2 format pcf 1 -->\n\0".to_vec();
    data.extend((dictionary.len() as i16).to_le_bytes());
    for value in &dictionary {
        string(&mut data, value);
    }
    data.extend(3i32.to_le_bytes());
    for (kind, name) in [(0i16, "root"), (1, "repeated"), (2, "x")] {
        data.extend(kind.to_le_bytes());
        string(&mut data, name);
        data.extend([0; 16]);
    }
    data.extend(1i32.to_le_bytes());
    data.extend(3i16.to_le_bytes());
    data.push(15);
    data.extend(1i32.to_le_bytes());
    data.extend(1i32.to_le_bytes());
    data.extend(1i32.to_le_bytes());
    data.extend(4i16.to_le_bytes());
    data.push(15);
    let references = MAX_VALUES - 513;
    data.extend((references as i32).to_le_bytes());
    for _ in 0..references {
        data.extend(2i32.to_le_bytes());
    }
    data.extend(512i32.to_le_bytes());
    for index in 5..516i16 {
        data.extend(index.to_le_bytes());
        data.extend([4, 1]);
    }
    data.extend(516i16.to_le_bytes());
    data.push(5);
    string(&mut data, "x");
    let file = parse(&data).unwrap();
    let names = &file.definitions[0].functions["emitters"];
    assert_eq!(names.len(), references);
    assert!(names.iter().all(|name| name == "x"));
}

#[test]
fn quoted_system_names_are_rejected_before_they_can_reach_vmf_properties() {
    let mut data = fixture();
    let offset = data
        .windows(b"test_flame".len())
        .position(|bytes| bytes == b"test_flame")
        .unwrap();
    data[offset..offset + b"bad\"effect".len()].copy_from_slice(b"bad\"effect");
    assert!(
        parse(&data)
            .unwrap_err()
            .to_string()
            .contains("name is invalid")
    );
}
