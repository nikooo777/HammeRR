use super::*;

fn material(source: &str) -> Result<Vec<Node>, ParseError> {
    parse_static_material_bounded(source, 64, 4096)
}

#[test]
fn boundary_comments_preserve_literal_nodes_and_quoted_contents() {
    let source = r#"/* lead */ "Water"/* shader */{
        // ignored [$WIN32] /* text
        $normalmap /* value */ "dev/water_normal"/* quoted boundary */
        $fogcolor "[0.1 0.2 0.3]"
        %keywords "quoted/*marker*/and//text*/"
        path "a\nb"
        repeat one repeat two
        Proxies { proxy { key value } }
    }/* trailing */"#;
    let expected = vec![Node::block(
        "Water",
        vec![
            Node::string("$normalmap", "dev/water_normal"),
            Node::string("$fogcolor", "[0.1 0.2 0.3]"),
            Node::string("%keywords", "quoted/*marker*/and//text*/"),
            Node::string("path", "a\\nb"),
            Node::string("repeat", "one"),
            Node::string("repeat", "two"),
            Node::block(
                "Proxies",
                vec![Node::block("proxy", vec![Node::string("key", "value")])],
            ),
        ],
    )];
    assert_eq!(material(source).unwrap(), expected);
    assert_eq!(parse_lenient(source).unwrap(), expected);
    assert_eq!(
        material("/* [] \" ignored */ // trailing").unwrap(),
        Vec::<Node>::new()
    );
}

#[test]
fn embedded_markers_reject_at_the_literal_shader_key_or_value_marker() {
    for (source, column) in [
        ("Water/*x*/ { key value }", 6),
        ("Water//x { key value }", 6),
        ("Water { $normalmap/*x*/ dev/water_normal }", 19),
        ("Water { $normalmap//x dev/water_normal }", 19),
        ("Water { $normalmap dev/water_normal/*x*/ }", 36),
        ("Water { $normalmap dev//water_normal }", 23),
        ("Water { $normalmap dev*/water_normal }", 23),
        ("Water { $normalmap dev//[selector] }", 23),
        ("Water { clé/*x*/ value }", 12),
    ] {
        assert_eq!(
            material(source).unwrap_err(),
            ParseError {
                line: 1,
                column,
                kind: ParseErrorKind::EmbeddedCommentMarker,
            },
            "{source}"
        );
    }
    let source = "/* café */\nWater {\n  $normalmap/*x*/ value\n}";
    assert_eq!(
        material(source).unwrap_err(),
        ParseError {
            line: 3,
            column: 13,
            kind: ParseErrorKind::EmbeddedCommentMarker,
        }
    );
}

#[test]
fn unquoted_selectors_reject_without_restricting_quoted_colors_or_markers() {
    for (source, column) in [
        ("[Water] { key value }", 1),
        ("Water { [$WIN32] value }", 9),
        ("Water { key [$WIN32] }", 13),
        ("Water { key path[$WIN32] }", 17),
        ("Water { key [x]/*marker*/ }", 13),
    ] {
        assert_eq!(
            material(source).unwrap_err(),
            ParseError {
                line: 1,
                column,
                kind: ParseErrorKind::ConditionalToken,
            },
            "{source}"
        );
    }
    assert_eq!(
        material("Water { \"[$WIN32]\" \"[1 2 3]\" key \"dev//[name]/*x*/\" }").unwrap(),
        vec![Node::block(
            "Water",
            vec![
                Node::string("[$WIN32]", "[1 2 3]"),
                Node::string("key", "dev//[name]/*x*/"),
            ]
        )]
    );
}

#[test]
fn static_comments_require_a_complete_nonoverlapping_closing_marker() {
    for source in [
        "Water {} /* open",
        "Water {} /*",
        "Water {} /*/",
        "Water {} /* open\n café",
    ] {
        assert_eq!(
            material(source).unwrap_err(),
            ParseError {
                line: 1,
                column: 10,
                kind: ParseErrorKind::UnterminatedComment,
            },
            "{source}"
        );
        assert_eq!(
            parse_lenient(source).unwrap(),
            vec![Node::block("Water", vec![])]
        );
    }
    assert_eq!(
        material("Water { /* open").unwrap_err(),
        ParseError {
            line: 1,
            column: 9,
            kind: ParseErrorKind::UnterminatedComment,
        }
    );
    assert_eq!(
        parse_lenient("Water {} /*/ tail value").unwrap(),
        vec![Node::block("Water", vec![]), Node::string("tail", "value"),]
    );
    assert_eq!(
        material("Water {} /*/ tail value").unwrap_err().kind,
        ParseErrorKind::UnterminatedComment
    );
    assert_eq!(
        material("Water {} /**/").unwrap(),
        vec![Node::block("Water", vec![])]
    );
    assert_eq!(
        parse("Water {} /* open").unwrap(),
        vec![Node::block("Water", vec![]), Node::string("/*", "open"),]
    );
}

#[test]
fn static_blocks_do_not_inherit_lenient_implicit_closure() {
    let source = "Water {\n Proxies {\n key value";
    let expected_error = ParseError {
        line: 2,
        column: 10,
        kind: ParseErrorKind::UnclosedBlock,
    };
    assert_eq!(material(source).unwrap_err(), expected_error);
    assert_eq!(parse(source).unwrap_err(), expected_error);
    assert_eq!(
        parse_lenient(source).unwrap(),
        vec![Node::block(
            "Water",
            vec![Node::block("Proxies", vec![Node::string("key", "value")]),]
        )]
    );
    assert_eq!(
        material("Water { key }").unwrap_err(),
        ParseError {
            line: 1,
            column: 9,
            kind: ParseErrorKind::MissingValue,
        }
    );
    assert_eq!(
        material("Water {\n key \"unfinished").unwrap_err(),
        ParseError {
            line: 2,
            column: 6,
            kind: ParseErrorKind::UnterminatedString,
        }
    );
}

#[test]
fn node_depth_and_nul_bounds_keep_their_existing_meanings() {
    let source = "Water { key value }";
    assert_eq!(
        parse_static_material_bounded(source, 1, 2).unwrap(),
        vec![Node::block("Water", vec![Node::string("key", "value")]),]
    );
    assert_eq!(
        parse_static_material_bounded(source, 1, 1).unwrap_err(),
        ParseError {
            line: 1,
            column: 9,
            kind: ParseErrorKind::NodeLimit,
        }
    );
    assert_eq!(
        parse_static_material_bounded(source, 0, 2).unwrap_err(),
        ParseError {
            line: 1,
            column: 7,
            kind: ParseErrorKind::NestingLimit,
        }
    );
    assert!(parse_static_material_bounded("", 0, 0).unwrap().is_empty());
    let deep = format!(
        "{}{}",
        "n {".repeat(MAX_NESTING + 1),
        "}".repeat(MAX_NESTING + 1)
    );
    assert_eq!(
        parse_static_material_bounded(&deep, usize::MAX, usize::MAX)
            .unwrap_err()
            .kind,
        ParseErrorKind::NestingLimit
    );

    let nul = "Water { key \"a\0b\" }";
    let expected = vec![Node::block("Water", vec![Node::string("key", "a\0b")])];
    assert_eq!(parse(nul).unwrap(), expected);
    assert_eq!(parse_lenient(nul).unwrap(), expected);
    for result in [
        material(nul),
        parse_bounded(nul, 64, 4096),
        parse_lenient_bounded(nul, 64, 4096),
    ] {
        assert_eq!(result.unwrap_err().kind, ParseErrorKind::NulByte);
    }
}

#[test]
fn existing_named_and_flat_modes_keep_literal_bare_markers() {
    let source = "Water { $normalmap/*x*/ dev//water_normal }";
    let expected = vec![Node::block(
        "Water",
        vec![Node::string("$normalmap/*x*/", "dev//water_normal")],
    )];
    assert_eq!(parse(source).unwrap(), expected);
    assert_eq!(parse_lenient(source).unwrap(), expected);
    assert_eq!(parse_bounded(source, 1, 2).unwrap(), expected);
    assert_eq!(parse_lenient_bounded(source, 1, 2).unwrap(), expected);
    assert_eq!(
        material(source).unwrap_err().kind,
        ParseErrorKind::EmbeddedCommentMarker
    );

    let limits = FlatBlockLimits {
        max_bytes: 1024,
        max_blocks: 1,
        max_pairs: 1,
    };
    assert_eq!(
        parse_flat_blocks_bounded("{ $normalmap/*x*/ dev//water_normal }", limits).unwrap(),
        vec![vec![("$normalmap/*x*/".into(), "dev//water_normal".into()),]]
    );
    assert_eq!(
        parse_flat_blocks_bounded("/* block */ {}", limits)
            .unwrap_err()
            .kind,
        ParseErrorKind::ExpectedOpenBrace
    );
    let ordinary =
        crate::vmt::Material::parse("Water { $normalmap/*x*/ dev//water_normal /* open").unwrap();
    assert_eq!(ordinary.get("$normalmap/*x*/"), Some("dev//water_normal"));
    assert!(ordinary.get("$normalmap").is_none());
}

#[test]
fn static_syntax_keeps_named_dialect_and_leaves_schema_to_its_caller() {
    let source = "Water { clé()' café\u{0001}value } Other {}";
    let expected = vec![
        Node::block("Water", vec![Node::string("clé()'", "café\u{0001}value")]),
        Node::block("Other", vec![]),
    ];
    assert_eq!(material(source).unwrap(), expected);
    assert_eq!(parse(source).unwrap(), expected);
    assert_eq!(
        material("Water { a\"b\" \"c\"d }").unwrap(),
        vec![Node::block(
            "Water",
            vec![Node::string("a", "b"), Node::string("c", "d"),]
        )]
    );
}
