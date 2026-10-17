use super::*;

#[test]
fn bounded_load_preserves_relative_includes_extensions_and_canonical_aliases() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("real/root.fgd");
    let base = directory.path().join("real/defs/base.fgd");
    std::fs::create_dir_all(base.parent().unwrap()).unwrap();
    std::fs::write(
        &base,
        "@mapsize(64, -64) @BaseClass = Base [ count(integer) : \"Count\" : 2 ]",
    )
    .unwrap();
    std::fs::write(&root, "@include \"defs\\base.fgd\" @include \"defs/base.fgd\" @PointClass base(Base) = thing [] @ExtendClass = Base [ added(string) ]").unwrap();
    let loaded = Catalog::load(&root).unwrap();
    assert_eq!(loaded.sources.len(), 2);
    assert_eq!(loaded.class("thing").unwrap().properties.len(), 2);
    assert_eq!(
        loaded.map_size.as_ref().unwrap().source.as_ref(),
        Some(&base.canonicalize().unwrap())
    );
    let hard = directory.path().join("real/hard.fgd");
    std::fs::hard_link(&root, &hard).unwrap();
    assert_eq!(
        Catalog::load(&hard).unwrap().class("thing"),
        loaded.class("thing")
    );
    #[cfg(unix)]
    {
        let alias = directory.path().join("alias.fgd");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let via_alias = Catalog::load(&alias).unwrap();
        assert_eq!(via_alias.sources, loaded.sources);
        assert_eq!(via_alias.class("thing"), loaded.class("thing"));
        let alias_dir = directory.path().join("alias-dir");
        std::os::unix::fs::symlink(root.parent().unwrap(), &alias_dir).unwrap();
        assert_eq!(
            Catalog::load(&alias_dir.join("root.fgd")).unwrap().sources,
            loaded.sources
        );
    }
}

#[test]
fn oversized_regular_sources_and_included_special_files_reject_before_reading() {
    let directory = tempfile::tempdir().unwrap();
    let large = directory.path().join("large.fgd");
    std::fs::File::create(&large)
        .unwrap()
        .set_len(MAX_FILE_BYTES as u64 + 1)
        .unwrap();
    assert!(Catalog::load(&large).unwrap_err().contains("regular file"));
    assert!(
        Catalog::load(directory.path())
            .unwrap_err()
            .contains("regular file")
    );
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let fifo = directory.path().join("pipe.fgd");
        let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(
            load_without_blocking(&fifo)
                .unwrap_err()
                .contains("regular file")
        );
        let root = directory.path().join("root.fgd");
        std::fs::write(&root, "@include \"pipe.fgd\"").unwrap();
        assert!(
            load_without_blocking(&root)
                .unwrap_err()
                .contains("regular file")
        );
    }
}

#[cfg(unix)]
fn load_without_blocking(path: &std::path::Path) -> Result<Catalog, String> {
    let path = path.to_owned();
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(Catalog::load(&path));
    });
    receive
        .recv_timeout(std::time::Duration::from_secs(3))
        .expect("FGD special-file admission blocked")
}

#[test]
fn cancellation_reaches_long_lexer_and_punctuation_only_parser_loops() {
    let mut calls = 0;
    let mut check = || {
        calls += 1;
        if calls == 3 {
            Err("lexer cancelled".into())
        } else {
            Ok(())
        }
    };
    let mut budget = Budget::new(&mut check);
    let comment = format!("/*{}*/", " ".repeat(32 * 1024));
    assert!(
        Parser::new(&comment, &mut budget)
            .err()
            .unwrap()
            .contains("lexer cancelled")
    );
    for body in [
        format!("@PointClass helper({}) = thing []", ",".repeat(10_000)),
        format!(
            "@PointClass = thing [ p(string) {} ]",
            "readonly ".repeat(10_000)
        ),
        format!("@PointClass = thing [ p(string) {} ]", ":".repeat(10_000)),
    ] {
        let parsing = std::cell::Cell::new(false);
        let checks = std::cell::Cell::new(0);
        let mut check = || {
            if parsing.get() {
                checks.set(checks.get() + 1);
                if checks.get() == 8 {
                    return Err("parser cancelled".into());
                }
            }
            Ok(())
        };
        let mut budget = Budget::new(&mut check);
        let parser = Parser::new(&body, &mut budget).unwrap();
        parsing.set(true);
        assert!(parser.parse().err().unwrap().contains("parser cancelled"));
    }
}

#[test]
fn cancellation_reaches_inherited_member_copy_before_catalog_publication() {
    let source = format!(
        "@BaseClass = Base [ {} ] @PointClass base(Base) = thing []",
        (0..100)
            .map(|index| format!("p{index}(string) "))
            .collect::<String>()
    );
    let resolving = std::cell::Cell::new(false);
    let calls = std::cell::Cell::new(0);
    let mut check = || {
        if resolving.get() {
            calls.set(calls.get() + 1);
            if calls.get() == 20 {
                return Err("resolution cancelled".into());
            }
        }
        Ok(())
    };
    let mut budget = Budget::new(&mut check);
    let declarations = Parser::new(&source, &mut budget).unwrap().parse().unwrap();
    let mut raw = RawCatalog::default();
    for declaration in declarations {
        raw.insert(declaration, None);
    }
    resolving.set(true);
    assert!(
        Catalog::resolve(raw, Vec::new(), &mut budget)
            .unwrap_err()
            .contains("resolution cancelled")
    );
}
