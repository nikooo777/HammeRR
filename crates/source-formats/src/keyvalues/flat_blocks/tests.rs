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

#[test]
fn bare_native_punctuation_cannot_hide_a_skyname_override() {
    let source = "{ classname worldspawn skyname test message x()skyname other v()x }";
    assert_eq!(
        parse_flat_blocks_bounded(source, limits())
            .unwrap_err()
            .kind,
        ParseErrorKind::UnsupportedBareToken
    );
    for token in ["bad(key", "bad)key", "bad'key"] {
        let source = format!("{{ {token} value }}");
        assert_eq!(
            parse_flat_blocks_bounded(&source, limits())
                .unwrap_err()
                .kind,
            ParseErrorKind::UnsupportedBareToken
        );
    }
    let quoted =
        r#"{ "classname" "worldspawn" "skyname" "test" "message" "x()skyname" "other" "v()x'" }"#;
    let blocks = parse_flat_blocks_bounded(quoted, limits()).unwrap();
    assert_eq!(
        blocks[0],
        vec![
            ("classname".into(), "worldspawn".into()),
            ("skyname".into(), "test".into()),
            ("message".into(), "x()skyname".into()),
            ("other".into(), "v()x'".into()),
        ]
    );
}

#[test]
fn bare_quote_adjacency_rejects_and_quoted_first_adjacency_remains_literal() {
    for source in [r#"{ a"b" c }"#, r#"{ key va"lue" other }"#] {
        assert_eq!(
            parse_flat_blocks_bounded(source, limits())
                .unwrap_err()
                .kind,
            ParseErrorKind::UnsupportedBareToken
        );
    }
    for source in [r#"{ "a"b }"#, r#"{ "a""b" }"#] {
        assert_eq!(
            parse_flat_blocks_bounded(source, limits()).unwrap(),
            vec![vec![("a".into(), "b".into())]]
        );
    }
}

#[test]
fn unsupported_bare_control_separators_reject_and_quoted_controls_remain_literal() {
    for byte in 1u8..=31 {
        let control = char::from(byte);
        let source = format!(
            "{{ classname worldspawn skyname test message harmless{control}skyname{control}other }}"
        );
        if !byte.is_ascii_whitespace() {
            assert_eq!(
                parse_flat_blocks_bounded(&source, limits())
                    .unwrap_err()
                    .kind,
                ParseErrorKind::UnsupportedBareToken
            );
            let key = format!("{{ skyname{control}other{control}x ignored }}");
            assert_eq!(
                parse_flat_blocks_bounded(&key, limits()).unwrap_err().kind,
                ParseErrorKind::UnsupportedBareToken
            );
        }
        let quoted = format!("{{ message \"harmless{control}skyname{control}other\" }}");
        let blocks = parse_flat_blocks_bounded(&quoted, limits()).unwrap();
        assert_eq!(
            blocks[0][0].1.as_bytes(),
            format!("harmless{control}skyname{control}other").as_bytes()
        );
    }
    let source = "{\tclassname worldspawn\u{000c}skyname\rtest\n}";
    assert_eq!(
        parse_flat_blocks_bounded(source, limits()).unwrap()[0],
        vec![
            ("classname".into(), "worldspawn".into()),
            ("skyname".into(), "test".into()),
        ]
    );
}

#[test]
fn bare_non_ascii_rejects_without_changing_quoted_utf8_or_named_keyvalues() {
    for source in ["{clé value}", "{key café}", "{key skyname\u{00a0}other}"] {
        assert_eq!(
            parse_flat_blocks_bounded(source, limits())
                .unwrap_err()
                .kind,
            ParseErrorKind::UnsupportedBareToken
        );
    }
    let source = "{ \"clé()'\" \"café\u{00a0}other\" }";
    assert_eq!(
        parse_flat_blocks_bounded(source, limits()).unwrap()[0],
        vec![("clé()'".into(), "café\u{00a0}other".into())]
    );
    assert!(crate::keyvalues::parse("root { clé()' café\u{0001}value }").is_ok());
}
