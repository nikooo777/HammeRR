use super::*;

fn texture(format: u32, values: [u16; 4]) -> Vtf {
    let mut bytes = vec![0u8; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    bytes[4..8].copy_from_slice(&7u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&80u32.to_le_bytes());
    bytes[16..18].copy_from_slice(&1u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&1u16.to_le_bytes());
    bytes[24..26].copy_from_slice(&1u16.to_le_bytes());
    bytes[52..56].copy_from_slice(&format.to_le_bytes());
    bytes[56] = 1;
    bytes[57..61].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes[63..65].copy_from_slice(&1u16.to_le_bytes());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    Vtf::parse(bytes).unwrap()
}
#[test]
fn half_radiance_above_one_is_preserved_and_nonfinite_or_negative_rgb_rejects() {
    let values = [0x0000, 0x3400, 0x4400, 0x4c00];
    let texture = texture(24, values);
    validate_hdr_texture(&texture).unwrap();
    let bytes = hdr_mip_bytes(&texture, 0).unwrap();
    assert!(matches!(bytes, std::borrow::Cow::Borrowed(_)));
    assert_eq!(
        bytes.as_ref(),
        values
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    );
    for bits in [0x7c00, 0xfc00, 0x7e00, 0xbc00] {
        let texture = super::tests::texture(24, [bits, 0x3c00, 0, 0x3c00]);
        assert!(validate_hdr_texture(&texture).is_err());
    }
    assert!(validate_hdr_texture(&super::tests::texture(24, [0, 0, 0, 0x7e00])).is_err());
}
#[test]
fn integer_hdr_uses_linear_unorm_times_sixteen_without_srgb_or_eight_bit_clamping() {
    let texture = texture(25, [0, 16384, 65535, 65535]);
    validate_hdr_texture(&texture).unwrap();
    let bytes = hdr_mip_bytes(&texture, 0).unwrap();
    let values = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| half::f16::from_bits(u16::from_le_bytes([pair[0], pair[1]])).to_f32())
        .collect::<Vec<_>>();
    assert_eq!(values, [0.0, 4.0, 16.0, 1.0]);
    assert!(matches!(bytes, std::borrow::Cow::Owned(_)));
}
#[test]
fn hdr_payload_identity_changes_even_for_same_family_after_cache_replacement() {
    let faces = || {
        super::super::SUFFIXES.map(|suffix| {
            super::super::Face::hdr(
                None,
                suffix.into(),
                None,
                None,
                None,
                None,
                Some("missing".into()),
            )
        })
    };
    let first = super::super::Skybox::hdr("family".into(), faces());
    let second = super::super::Skybox::hdr("family".into(), faces());
    assert_ne!(first.payload_id, second.payload_id);
    assert!(!first.ready());
}
