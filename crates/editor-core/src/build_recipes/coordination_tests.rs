#![cfg(target_os = "linux")]

use super::*;
use crate::preference_file::WriteLock;
use crate::session::compilation::BuildSettingsUpdate;
use crate::session::{Actor, Session};
use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CHILD_ROOT: &str = "HAMMERR_RECIPE_COORDINATION_FIXTURE";
const CHILD_TEST: &str = "build_recipes::coordination_tests::independent_recipe_writer_fixture";
const CUSTOM_BASENAME: &[u8] = b"custom-recipes-\xff.store";
const WAIT: Duration = Duration::from_secs(10);

struct Writer {
    child: Child,
    channel: Option<BufReader<UnixStream>>,
    log: PathBuf,
}

impl Writer {
    fn start(directory: &Path) -> Self {
        let listener = UnixListener::bind(directory.join("control.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let log = directory.join("child.log");
        let output = fs::File::create(&log).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--ignored", "--nocapture"])
            .env(CHILD_ROOT, directory)
            .stdin(Stdio::null())
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .spawn()
            .unwrap();
        let mut writer = Self {
            child,
            channel: None,
            log,
        };
        let deadline = Instant::now() + WAIT;
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_read_timeout(Some(WAIT)).unwrap();
                    stream.set_write_timeout(Some(WAIT)).unwrap();
                    writer.channel = Some(BufReader::new(stream));
                    return writer;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("Recipe child connection failed: {error}"),
            }
            assert!(
                writer.child.try_wait().unwrap().is_none() && Instant::now() < deadline,
                "Recipe child did not connect: {}",
                fs::read_to_string(&writer.log).unwrap()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn expect(&mut self, expected: &str) {
        let mut line = String::new();
        let result = self.channel.as_mut().unwrap().read_line(&mut line);
        assert!(
            result.is_ok(),
            "Recipe child barrier failed: {result:?}; {}",
            fs::read_to_string(&self.log).unwrap()
        );
        assert_eq!(
            line.trim_end(),
            expected,
            "Recipe child barrier mismatch: {}",
            fs::read_to_string(&self.log).unwrap()
        );
    }

    fn send(&mut self, command: &str) {
        writeln!(self.channel.as_mut().unwrap().get_mut(), "{command}").unwrap();
    }

    fn finish(&mut self) {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "Recipe child failed: {}",
                    fs::read_to_string(&self.log).unwrap()
                );
                return;
            }
            assert!(Instant::now() < deadline, "Recipe child did not exit");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn attached(path: &Path) -> (Authority, Session) {
    let authority = Authority::default();
    authority.configure_at(Ok(path.to_owned())).unwrap();
    let session = Session::with_build_recipes(Err("no assets needed".into()), authority.clone());
    (authority, session)
}

fn protected(session: &Session, authority: &Authority) -> Value {
    let state = session.build_recipes();
    let saved = authority.snapshot();
    json!({
        "catalog": saved.file,
        "registry_revision": saved.revision,
        "form_revision": state["revision"],
        "active": state["active"],
        "options": Settings::from(session.compile_options()),
        "source": session.document().vmf().write(&Default::default()),
        "history": session.history_snapshot(),
    })
}

fn active_id(state: &Value) -> &str {
    state["active"]["id"].as_str().unwrap()
}

fn assert_catalog(file: &File) {
    assert_eq!(file.next_id, 4);
    let identities = file
        .recipes
        .iter()
        .map(|recipe| (recipe.id.as_str(), recipe.name.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        identities,
        [
            ("custom:1", "Original"),
            ("custom:2", "Independent writer"),
            ("custom:3", "Reloaded retry"),
        ]
    );
    assert_eq!(
        file.recipes[0].settings.options().unwrap(),
        parent_options()
    );
    assert_eq!(
        file.recipes[2].settings.options().unwrap(),
        parent_options()
    );
    assert_eq!(
        file.recipes[1].settings.options().unwrap(),
        compiler::Options {
            lighting: compiler::LightingMode::DirectLdr,
            ..Default::default()
        }
    );
}

fn parent_options() -> compiler::Options {
    compiler::Options {
        lighting: compiler::LightingMode::DirectLdrHdr,
        bounces: 2,
        pack_sky_assets: true,
        ..Default::default()
    }
}

#[test]
fn independent_writer_lock_and_stale_source_preserve_state_until_reload_retry() {
    let directory = tempfile::tempdir().unwrap();
    let physical = directory.path().join("physical");
    let requested = directory.path().join("requested");
    fs::create_dir(&physical).unwrap();
    fs::create_dir(&requested).unwrap();
    let path = physical.join(OsStr::from_bytes(CUSTOM_BASENAME));
    let lossy = physical.join(String::from_utf8_lossy(CUSTOM_BASENAME).as_ref());
    fs::write(&path, serde_json::to_vec(&File::default()).unwrap()).unwrap();
    let alias = requested.join("recipe-link.json");
    symlink(&path, &alias).unwrap();
    let (authority, mut session) = attached(&alias);
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                lighting: Some("direct_ldr_hdr"),
                bounces: Some(2),
                pack_sky_assets: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(session.compile_options(), parent_options());
    let original = session
        .save_build_recipe(Actor::User, "Original", None, None)
        .unwrap();
    assert_eq!(active_id(&original), "custom:1");
    assert!(!lossy.exists());
    let before = protected(&session, &authority);
    let bytes = fs::read(&path).unwrap();
    let revision = original["revision"].as_u64().unwrap();

    let mut writer = Writer::start(directory.path());
    writer.expect("locked");
    assert_ne!(writer.child.id(), std::process::id());
    let error = session
        .save_build_recipe(Actor::Agent, "Blocked save", None, Some(revision))
        .unwrap_err();
    assert!(
        error.contains("busy") && error.contains("locked"),
        "{error}"
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(protected(&session, &authority), before);
    assert!(
        session.build_recipes()["error"]
            .as_str()
            .unwrap()
            .contains("busy")
    );
    assert!(physical.join(LOCK_FILE_NAME).is_file());
    assert!(!requested.join(LOCK_FILE_NAME).exists());
    assert!(fs::symlink_metadata(&alias).unwrap().is_symlink());

    writer.send("publish");
    writer.expect("published custom:2");
    let independent_bytes = fs::read(&path).unwrap();
    assert_ne!(independent_bytes, bytes);
    let independent: File = serde_json::from_slice(&independent_bytes).unwrap();
    assert_eq!(independent.next_id, 3);
    assert_eq!(independent.recipes.len(), 2);
    let error = session
        .save_build_recipe(Actor::User, "Stale save", None, Some(revision))
        .unwrap_err();
    assert!(
        error.contains("changed") && error.contains("Reload recipes"),
        "{error}"
    );
    assert_eq!(fs::read(&path).unwrap(), independent_bytes);
    assert_eq!(protected(&session, &authority), before);

    let options = session.compile_options();
    let reloaded = session
        .reload_build_recipes(Actor::Agent, Some(revision))
        .unwrap();
    assert!(reloaded["error"].is_null());
    assert_eq!(reloaded["persistence"], "persistent");
    assert_eq!(reloaded["active"], original["active"]);
    assert_eq!(session.compile_options(), options);
    assert_eq!(authority.snapshot().file.next_id, 3);
    assert_eq!(authority.snapshot().file.recipes.len(), 2);
    let retried = session
        .save_build_recipe(
            Actor::Agent,
            "Reloaded retry",
            None,
            reloaded["revision"].as_u64(),
        )
        .unwrap();
    assert_eq!(active_id(&retried), "custom:3");
    assert_eq!(session.compile_options(), options);
    let final_bytes = fs::read(&path).unwrap();
    let saved: File = serde_json::from_slice(&final_bytes).unwrap();
    assert_catalog(&saved);
    assert_catalog(&authority.snapshot().file);
    assert!(!physical.join(FILE_NAME).exists());
    assert!(!requested.join(FILE_NAME).exists());
    assert!(!lossy.exists());
    assert!(fs::symlink_metadata(&alias).unwrap().is_symlink());
    writer.send("verify");
    writer.expect("verified");
    writer.finish();
    assert_eq!(fs::read(&path).unwrap(), final_bytes);
}

#[test]
#[ignore = "independent child fixture, invoked by the coordination regression"]
fn independent_recipe_writer_fixture() {
    let directory = PathBuf::from(std::env::var_os(CHILD_ROOT).expect("child fixture path"));
    let path = directory
        .join("physical")
        .join(OsStr::from_bytes(CUSTOM_BASENAME));
    let authority = Authority::default();
    authority.configure_at(Ok(path.clone())).unwrap();
    assert_eq!(authority.snapshot().file.next_id, 2);
    let guard = WriteLock::acquire(
        path.parent().unwrap(),
        LOCK_FILE_NAME,
        "Build recipes",
        "Reload recipes",
    )
    .unwrap();
    let stream = UnixStream::connect(directory.join("control.sock")).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.set_write_timeout(Some(WAIT)).unwrap();
    let mut channel = BufReader::new(stream);
    writeln!(channel.get_mut(), "locked").unwrap();
    let mut command = String::new();
    channel.read_line(&mut command).unwrap();
    assert_eq!(command, "publish\n");
    drop(guard);
    let (published, _) = authority
        .edit(
            None,
            Edit::Save {
                name: "Independent writer",
                replace: None,
                settings: compiler::Options {
                    lighting: compiler::LightingMode::DirectLdr,
                    ..Default::default()
                }
                .into(),
            },
        )
        .unwrap();
    assert_eq!(published.id, "custom:2");
    writeln!(channel.get_mut(), "published custom:2").unwrap();
    command.clear();
    channel.read_line(&mut command).unwrap();
    assert_eq!(command, "verify\n");
    authority.reload(None).unwrap();
    assert_catalog(&authority.snapshot().file);
    writeln!(channel.get_mut(), "verified").unwrap();
}
