use super::*;
use crate::bsp::{Bsp, lump};

fn limits() -> FlatBlockLimits {
    FlatBlockLimits {
        max_bytes: 4096,
        max_blocks: 8,
        max_pairs: 32,
    }
}

#[test]
fn anonymous_blocks_preserve_pair_order_duplicates_and_empty_blocks() {
    let source = r#"// entity lump
{
"classname" "worldspawn"
"skyname" "Sky_Custom"
"SKYNAME" "duplicate"
"message" "first"
"message" "second"
}
{ classname info_target origin "1 2 3" }
{}
"#;
    let blocks = parse_flat_blocks_bounded(source, limits()).unwrap();
    assert_eq!(
        blocks,
        vec![
            vec![
                ("classname".into(), "worldspawn".into()),
                ("skyname".into(), "Sky_Custom".into()),
                ("SKYNAME".into(), "duplicate".into()),
                ("message".into(), "first".into()),
                ("message".into(), "second".into()),
            ],
            vec![
                ("classname".into(), "info_target".into()),
                ("origin".into(), "1 2 3".into()),
            ],
            vec![],
        ]
    );
    for source in [
        "",
        " \r\n\t",
        "// no blocks",
        "// no blocks\n // still none",
    ] {
        assert!(
            parse_flat_blocks_bounded(source, limits())
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn quoted_braces_comments_and_backslashes_remain_literal() {
    let source = r#"{
"path" "C:\maps\test\n"
"braces" "} { // /* literal */"
"trailing" "ends\"
"" ""
"é" "🙂"
} // trailing comment
"#;
    let blocks = parse_flat_blocks_bounded(source, limits()).unwrap();
    assert_eq!(
        blocks[0],
        vec![
            ("path".into(), r"C:\maps\test\n".into()),
            ("braces".into(), "} { // /* literal */".into()),
            ("trailing".into(), "ends\\".into()),
            ("".into(), "".into()),
            ("é".into(), "🙂".into()),
        ]
    );
}

#[test]
fn malformed_or_truncated_blocks_never_return_a_valid_prefix() {
    for (source, kind) in [
        ("{", ParseErrorKind::UnclosedBlock),
        ("{} {", ParseErrorKind::UnclosedBlock),
        ("{ key value", ParseErrorKind::UnclosedBlock),
        ("{ key", ParseErrorKind::MissingValue),
        ("{ key }", ParseErrorKind::MissingValue),
        ("{ key \"unfinished }", ParseErrorKind::UnterminatedString),
        ("root {}", ParseErrorKind::ExpectedOpenBrace),
        ("{} trailing", ParseErrorKind::ExpectedOpenBrace),
        (
            "\"classname\" \"worldspawn\"",
            ParseErrorKind::ExpectedOpenBrace,
        ),
        ("{ key { nested value } }", ParseErrorKind::NestedBlock),
        ("{{}}", ParseErrorKind::NestedBlock),
        ("{} }", ParseErrorKind::UnexpectedCloseBrace),
        ("}", ParseErrorKind::UnexpectedCloseBrace),
        ("/* block comment */ {}", ParseErrorKind::ExpectedOpenBrace),
    ] {
        let error = parse_flat_blocks_bounded(source, limits()).unwrap_err();
        assert_eq!(error.kind, kind, "{source}");
    }
}

#[test]
fn byte_block_and_total_pair_limits_admit_exact_boundaries() {
    let source = "{\"é\" \"🙂\"}";
    let exact_bytes = FlatBlockLimits {
        max_bytes: source.len(),
        ..limits()
    };
    assert!(parse_flat_blocks_bounded(source, exact_bytes).is_ok());
    assert_eq!(
        parse_flat_blocks_bounded(
            source,
            FlatBlockLimits {
                max_bytes: source.len() - 1,
                ..limits()
            }
        )
        .unwrap_err()
        .kind,
        ParseErrorKind::ByteLimit
    );
    let exact_blocks = FlatBlockLimits {
        max_blocks: 2,
        max_pairs: 0,
        ..limits()
    };
    assert_eq!(
        parse_flat_blocks_bounded("{}{}", exact_blocks)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        parse_flat_blocks_bounded("{}{}{}", exact_blocks)
            .unwrap_err()
            .kind,
        ParseErrorKind::BlockLimit
    );
    let exact_pairs = FlatBlockLimits {
        max_pairs: 2,
        ..limits()
    };
    assert!(parse_flat_blocks_bounded("{a 1}{a 2}", exact_pairs).is_ok());
    assert!(parse_flat_blocks_bounded("{a 1 a 2}", exact_pairs).is_ok());
    assert_eq!(
        parse_flat_blocks_bounded("{a 1}{a 2 a 3}", exact_pairs)
            .unwrap_err()
            .kind,
        ParseErrorKind::PairLimit
    );
    let zero = FlatBlockLimits {
        max_bytes: 0,
        max_blocks: 0,
        max_pairs: 0,
    };
    assert!(parse_flat_blocks_bounded("", zero).unwrap().is_empty());
    assert_eq!(
        parse_flat_blocks_bounded(
            "{}",
            FlatBlockLimits {
                max_blocks: 0,
                ..limits()
            }
        )
        .unwrap_err()
        .kind,
        ParseErrorKind::BlockLimit
    );
    assert_eq!(
        parse_flat_blocks_bounded(
            "{a 1}",
            FlatBlockLimits {
                max_pairs: 0,
                ..limits()
            }
        )
        .unwrap_err()
        .kind,
        ParseErrorKind::PairLimit
    );
}

#[test]
fn embedded_nul_and_error_positions_are_preserved() {
    for source in ["{}\0{}", "{ key \"a\0b\" }", "// comment\0\n{}"] {
        assert_eq!(
            parse_flat_blocks_bounded(source, limits())
                .unwrap_err()
                .kind,
            ParseErrorKind::NulByte
        );
    }
    for (source, position, kind) in [
        (
            "{\n \"é\" value\n key\n}",
            (3, 2),
            ParseErrorKind::MissingValue,
        ),
        ("{}\n  { key value", (2, 3), ParseErrorKind::UnclosedBlock),
        ("{\n key {\n }\n}", (2, 6), ParseErrorKind::NestedBlock),
    ] {
        let error = parse_flat_blocks_bounded(source, limits()).unwrap_err();
        assert_eq!((error.line, error.column), position);
        assert_eq!(error.kind, kind);
    }
}

#[test]
fn controlled_parser_preserves_callback_failures_before_and_during_parsing() {
    let error = parse_flat_blocks_controlled("{", limits(), &mut || {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "stopped before parsing",
        ))
    })
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(error.to_string(), "stopped before parsing");
    let mut checkpoints = 0;
    let error = parse_flat_blocks_controlled("{a 1}{b 2}", limits(), &mut || {
        checkpoints += 1;
        if checkpoints == 6 {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "cancelled after first block",
            ))
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert_eq!(checkpoints, 6);
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    assert_eq!(error.to_string(), "cancelled after first block");
}

#[test]
fn controlled_parser_retains_typed_syntax_and_limit_failures() {
    for (source, supplied, kind) in [
        ("{", limits(), ParseErrorKind::UnclosedBlock),
        (
            "{}{}",
            FlatBlockLimits {
                max_blocks: 1,
                ..limits()
            },
            ParseErrorKind::BlockLimit,
        ),
        (
            "{a 1}",
            FlatBlockLimits {
                max_pairs: 0,
                ..limits()
            },
            ParseErrorKind::PairLimit,
        ),
    ] {
        let error = parse_flat_blocks_controlled(source, supplied, &mut || Ok(())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let syntax = error
            .get_ref()
            .unwrap()
            .downcast_ref::<ParseError>()
            .unwrap();
        assert_eq!(syntax.kind, kind);
    }
    let source = "{classname worldspawn skyname Sky_Custom}{}";
    assert_eq!(
        parse_flat_blocks_controlled(source, limits(), &mut || Ok(())).unwrap(),
        parse_flat_blocks_bounded(source, limits()).unwrap()
    );
}

#[test]
fn bsp_entity_lump_uses_native_nul_boundary_without_mutating_bytes() {
    let mut bsp = Bsp::new();
    bsp.set_entities("{ classname worldspawn skyname Sky_Custom message \"} {\" }");
    bsp.lumps[lump::ENTITIES]
        .data
        .extend_from_slice(b"ignored\xff");
    let before = bsp.to_bytes();
    let blocks = parse_flat_blocks_bounded(bsp.entities().unwrap(), limits()).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0][1], ("skyname".into(), "Sky_Custom".into()));
    assert_eq!(blocks[0][2], ("message".into(), "} {".into()));
    assert_eq!(bsp.to_bytes(), before);
    bsp.lumps[lump::ENTITIES].data = b"{ classname \xff }\0".to_vec();
    assert!(bsp.entities().is_err());
}
