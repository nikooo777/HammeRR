use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::isolation::Identity;
use crate::util::{self, invalid};

pub(super) const PROFILE: &str = "canonical_dxvk_sampler_942";
const CONFIG_BYTES: &[u8] =
    b"d3d9.samplerAnisotropy = -1\nd3d9.samplerLodBias = 0\nd3d9.clampNegativeLodBias = false\n";
const OPTIONS: [(&str, &str); 3] = [
    ("d3d9.samplerAnisotropy", "-1"),
    ("d3d9.samplerLodBias", "0"),
    ("d3d9.clampNegativeLodBias", "false"),
];
const ENVIRONMENT: [&str; 4] = [
    "DXVK_CONFIG_FILE",
    "DXVK_CONFIG",
    "DXVK_LOG_LEVEL",
    "DXVK_LOG_PATH",
];
const MAX_LOG: usize = 1024 * 1024;
const MAX_ENVIRONMENT: usize = 1024 * 1024;
const MAX_DESCRIPTORS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

pub(super) struct Prepared {
    directory: PathBuf,
    config: PathBuf,
    logs: PathBuf,
    config_identity: FileIdentity,
    config_sha256_before: String,
}

#[derive(Serialize)]
pub(super) struct Ready {
    native_pid: u32,
    native_start: String,
    config_sha256_ready: String,
    loaded_maps_sha256: String,
    environment: BTreeMap<String, Option<String>>,
    log: LogObservation,
}

#[derive(Serialize)]
struct LogObservation {
    path: PathBuf,
    descriptor: u32,
    identity: FileIdentity,
    ready_bytes: usize,
    ready_sha256: String,
}

#[derive(Serialize)]
pub(super) struct SamplerDelivery {
    profile: &'static str,
    config_file: PathBuf,
    config_sha256_before: String,
    config_sha256_after: String,
    config_identity: FileIdentity,
    ready: Ready,
    log_sha256_after: String,
    effective_options: BTreeMap<String, String>,
    loaded_backend: BTreeMap<&'static str, &'static str>,
    live_sampler_observed: bool,
}

fn owned_file(path: &Path, limit: usize) -> io::Result<(FileIdentity, Vec<u8>)> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.len() > limit as u64
    {
        return Err(invalid(
            "DXVK evidence requires a bounded owned regular file",
        ));
    }
    let identity = FileIdentity::from_metadata(&metadata);
    let mut bytes = Vec::new();
    (&mut file).take(limit as u64 + 1).read_to_end(&mut bytes)?;
    let current = fs::symlink_metadata(path)?;
    if bytes.len() > limit
        || !current.is_file()
        || FileIdentity::from_metadata(&current) != identity
    {
        return Err(invalid(
            "DXVK evidence file changed identity or exceeded its budget",
        ));
    }
    Ok((identity, bytes))
}

fn process_start(native: &Identity) -> io::Result<()> {
    if crate::process::start_time(native.pid)? != native.start {
        return Err(invalid("DXVK observation native process start changed"));
    }
    Ok(())
}

fn environment_values(bytes: &[u8]) -> io::Result<BTreeMap<String, Option<String>>> {
    if !bytes.ends_with(&[0]) {
        return Err(invalid(
            "Incomplete native environment for DXVK observation",
        ));
    }
    let mut result: BTreeMap<_, _> = ENVIRONMENT
        .into_iter()
        .map(|name| (name.to_owned(), None))
        .collect();
    let mut seen = BTreeSet::new();
    for entry in bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let separator = entry.iter().position(|byte| *byte == b'=');
        let name = &entry[..separator.unwrap_or(entry.len())];
        let Some(expected) = ENVIRONMENT.into_iter().find(|key| key.as_bytes() == name) else {
            continue;
        };
        if !seen.insert(expected) {
            return Err(invalid(format!(
                "Duplicate native environment key {expected}"
            )));
        }
        let separator = separator.ok_or_else(|| invalid("Invalid DXVK environment assignment"))?;
        let value = std::str::from_utf8(&entry[separator + 1..]).map_err(io::Error::other)?;
        result.insert(expected.to_owned(), Some(value.to_owned()));
    }
    Ok(result)
}

fn complete_line_prefix(bytes: &[u8]) -> io::Result<&[u8]> {
    let end = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .ok_or_else(|| invalid("DXVK READY log has no complete records"))?;
    Ok(&bytes[..=end])
}

fn effective_options(bytes: &[u8], config: &Path) -> io::Result<BTreeMap<String, String>> {
    if !bytes.ends_with(b"\n") {
        return Err(invalid("DXVK log has an incomplete final record"));
    }
    let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
    let expected_source = format!("info:  Found config file: {}", config.display());
    let mut source_count = 0;
    let mut map_count = 0;
    let mut in_map = false;
    let mut options = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with("info:  Found config file:") {
            source_count += 1;
            if line != expected_source {
                return Err(invalid("DXVK read an unexpected configuration file"));
            }
        }
        if line.starts_with("info:  Found config env:") {
            return Err(invalid("DXVK inherited an inline configuration override"));
        }
        if line == "info:  Effective configuration:" {
            if source_count != 1 {
                return Err(invalid(
                    "DXVK effective map preceded its exact configuration source",
                ));
            }
            map_count += 1;
            in_map = true;
            continue;
        }
        if !in_map
            && line
                .strip_prefix("info:    ")
                .is_some_and(|entry| OPTIONS.iter().any(|(name, _)| entry.starts_with(name)))
        {
            return Err(invalid(
                "DXVK sampler assignment appeared outside its effective map",
            ));
        }
        if !in_map {
            continue;
        }
        let Some(assignment) = line.strip_prefix("info:    ") else {
            in_map = false;
            continue;
        };
        let (name, value) = assignment
            .split_once(" = ")
            .ok_or_else(|| invalid("Malformed DXVK effective-map assignment"))?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_'))
            || value.is_empty()
            || options.insert(name.to_owned(), value.to_owned()).is_some()
        {
            return Err(invalid("Duplicate or empty DXVK effective-map assignment"));
        }
    }
    if source_count != 1 || map_count != 1 {
        return Err(invalid(
            "DXVK requires one exact configuration source and effective map",
        ));
    }
    for (name, expected) in OPTIONS {
        if options.get(name).map(String::as_str) != Some(expected) {
            return Err(invalid(format!(
                "DXVK did not deliver canonical {name}={expected}"
            )));
        }
    }
    Ok(options)
}

fn log_descriptor(
    logs: &Path,
    path: &Path,
    identity: &FileIdentity,
    native_pid: u32,
) -> io::Result<u32> {
    let mut descriptor = None;
    for (index, entry) in fs::read_dir(format!("/proc/{native_pid}/fd"))?.enumerate() {
        if index >= MAX_DESCRIPTORS {
            return Err(invalid(
                "Native DXVK descriptor observation exceeds its budget",
            ));
        }
        let entry = entry?;
        let target = match fs::read_link(entry.path()) {
            Ok(target) => target,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !target.starts_with(logs) {
            continue;
        }
        let actual = fs::metadata(entry.path())?;
        if !actual.is_file()
            || target != path
            || FileIdentity::from_metadata(&actual) != *identity
            || descriptor.is_some()
        {
            return Err(invalid(
                "Native DXVK log descriptor is duplicate or conflicting",
            ));
        }
        descriptor = Some(
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
                .ok_or_else(|| invalid("Invalid native DXVK log descriptor"))?,
        );
    }
    descriptor.ok_or_else(|| invalid("Native PID does not hold the owned DXVK log"))
}

impl Prepared {
    pub fn prepare(directory: &Path, command: &mut Command) -> io::Result<Self> {
        let directory = directory.canonicalize()?;
        let config = directory.join("dxvk-sampler.conf");
        if config
            .to_str()
            .is_none_or(|path| path.contains(['\n', '\r']))
        {
            return Err(invalid("DXVK configuration path cannot be logged exactly"));
        }
        let metadata = fs::symlink_metadata(&directory)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(invalid(
                "DXVK staging requires the existing private owned capture directory",
            ));
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&config)?;
        file.write_all(CONFIG_BYTES)?;
        file.sync_all()?;
        let logs = directory.join("dxvk-logs");
        fs::create_dir(&logs)?;
        fs::set_permissions(&logs, fs::Permissions::from_mode(0o700))?;
        let (config_identity, bytes) = owned_file(&config, CONFIG_BYTES.len())?;
        if bytes != CONFIG_BYTES {
            return Err(invalid("DXVK staged configuration bytes differ"));
        }
        command
            .env("DXVK_CONFIG_FILE", &config)
            .env_remove("DXVK_CONFIG")
            .env("DXVK_LOG_LEVEL", "info")
            .env("DXVK_LOG_PATH", &logs);
        Ok(Self {
            directory,
            config,
            logs,
            config_identity,
            config_sha256_before: util::hash(&bytes),
        })
    }

    pub fn expected_environment(&self) -> BTreeMap<String, Option<String>> {
        BTreeMap::from([
            (
                "DXVK_CONFIG_FILE".into(),
                Some(self.config.to_string_lossy().into_owned()),
            ),
            ("DXVK_CONFIG".into(), None),
            ("DXVK_LOG_LEVEL".into(), Some("info".into())),
            (
                "DXVK_LOG_PATH".into(),
                Some(self.logs.to_string_lossy().into_owned()),
            ),
        ])
    }

    fn verify_config(&self) -> io::Result<String> {
        let (identity, bytes) = owned_file(&self.config, CONFIG_BYTES.len())?;
        if identity != self.config_identity || bytes != CONFIG_BYTES {
            return Err(invalid("DXVK pinned configuration changed"));
        }
        Ok(util::hash(&bytes))
    }

    fn log_file(&self) -> io::Result<PathBuf> {
        let metadata = fs::symlink_metadata(&self.logs)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(invalid(
                "DXVK log directory lost its private owned identity",
            ));
        }
        let mut entries = fs::read_dir(&self.logs)?;
        let path = entries
            .next()
            .ok_or_else(|| invalid("DXVK did not create its owned log"))??
            .path();
        if entries.next().is_some() {
            return Err(invalid("DXVK created multiple or conflicting log files"));
        }
        Ok(path)
    }

    pub fn observe_ready(&self, root: &Path, native: &Identity) -> io::Result<Ready> {
        process_start(native)?;
        let config_sha256_ready = self.verify_config()?;
        let bytes = util::read(
            &PathBuf::from(format!("/proc/{}/environ", native.pid)),
            MAX_ENVIRONMENT,
        )?;
        let environment = environment_values(&bytes)?;
        if environment != self.expected_environment() {
            return Err(invalid(
                "Actual native DXVK environment differs from the private profile",
            ));
        }
        let maps = super::hardware::loaded_observer(&self.directory, root, native)?;
        util::atomic_write(
            &self.directory.join("sampler-ready-maps.txt"),
            maps.as_bytes(),
        )?;
        let path = self.log_file()?;
        let (identity, bytes) = owned_file(&path, MAX_LOG)?;
        let prefix = complete_line_prefix(&bytes)?;
        effective_options(prefix, &self.config)?;
        let descriptor = log_descriptor(&self.logs, &path, &identity, native.pid)?;
        process_start(native)?;
        let ready = Ready {
            native_pid: native.pid,
            native_start: native.start.clone(),
            config_sha256_ready,
            loaded_maps_sha256: util::hash(maps.as_bytes()),
            environment,
            log: LogObservation {
                path,
                descriptor,
                identity,
                ready_bytes: prefix.len(),
                ready_sha256: util::hash(prefix),
            },
        };
        util::json_write(&self.directory.join("sampler-ready.json"), &ready)?;
        Ok(ready)
    }

    pub fn finish(self, ready: Ready) -> io::Result<SamplerDelivery> {
        let config_sha256_after = self.verify_config()?;
        let path = self.log_file()?;
        let (identity, bytes) = owned_file(&path, MAX_LOG)?;
        if path != ready.log.path
            || identity != ready.log.identity
            || bytes
                .get(..ready.log.ready_bytes)
                .is_none_or(|prefix| util::hash(prefix) != ready.log.ready_sha256)
        {
            return Err(invalid(
                "DXVK final log replaced or rewrote its observed source",
            ));
        }
        let effective_options = effective_options(&bytes, &self.config)?;
        let loaded_backend = super::profile::INSTALLED_FILES
            .iter()
            .copied()
            .filter(|(path, _)| {
                matches!(
                    *path,
                    "bin/linux64/shaderapivk.so" | "bin/linux64/libdxvk_d3d9.so"
                )
            })
            .collect();
        Ok(SamplerDelivery {
            profile: PROFILE,
            config_file: self.config,
            config_sha256_before: self.config_sha256_before,
            config_sha256_after,
            config_identity: self.config_identity,
            ready,
            log_sha256_after: util::hash(&bytes),
            effective_options,
            loaded_backend,
            live_sampler_observed: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::os::fd::AsRawFd;

    fn private_directory() -> tempfile::TempDir {
        tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap()
    }

    fn prepared(directory: &Path) -> Prepared {
        Prepared::prepare(directory, &mut Command::new("unused-native-program")).unwrap()
    }

    fn log(config: &Path) -> Vec<u8> {
        format!("info:  DXVK: v2.3\ninfo:  Found config file: {}\ninfo:  Effective configuration:\ninfo:    d3d9.samplerAnisotropy = -1\ninfo:    d3d9.samplerLodBias = 0\ninfo:    d3d9.clampNegativeLodBias = false\ninfo:  Enabled instance extensions:\ninfo:    VK_KHR_surface\n", config.display()).into_bytes()
    }

    #[test]
    fn private_profile_overrides_inline_configuration_and_pins_exact_bytes() {
        let directory = private_directory();
        let mut command = Command::new("unused-native-program");
        command.env("DXVK_CONFIG", "d3d9.samplerLodBias=1");
        let profile = Prepared::prepare(directory.path(), &mut command).unwrap();
        let selected: BTreeMap<_, _> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_str().unwrap().to_owned(),
                    value.map(|value| value.to_str().unwrap().to_owned()),
                )
            })
            .collect();
        assert_eq!(selected, profile.expected_environment());
        assert_eq!(fs::read(&profile.config).unwrap(), CONFIG_BYTES);
        assert_eq!(profile.verify_config().unwrap(), util::hash(CONFIG_BYTES));
        let exposed = tempfile::tempdir().unwrap();
        fs::set_permissions(exposed.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Prepared::prepare(exposed.path(), &mut command).is_err());
        assert!(Prepared::prepare(directory.path(), &mut command).is_err());
        fs::write(&profile.config, b"d3d9.samplerLodBias = 1\n").unwrap();
        assert!(profile.verify_config().is_err());
        fs::remove_file(&profile.config).unwrap();
        std::os::unix::fs::symlink("missing", &profile.config).unwrap();
        assert!(profile.verify_config().is_err());
    }

    #[test]
    fn effective_map_requires_unique_canonical_assignments_and_source() {
        let config = Path::new("/owned/dxvk-sampler.conf");
        let valid = String::from_utf8(log(config)).unwrap();
        let options = effective_options(valid.as_bytes(), config).unwrap();
        assert_eq!(options.len(), OPTIONS.len());
        for invalid in [
            valid.replace("info:  Effective configuration:\n", ""),
            valid.replace("d3d9.samplerAnisotropy = -1", "samplerAnisotropy : 1"),
            valid.replace(
                "d3d9.clampNegativeLodBias = false",
                "d3d9.clampNegativeLodBias = 0",
            ),
            valid.replace("info:  Found config file:", "info:  Found config env:"),
            format!("{valid}{valid}"),
            format!("{valid}info:    d3d9.samplerLodBias = 1\n"),
            valid.replace(
                "info:    d3d9.samplerLodBias = 0\n",
                "info:    d3d9.samplerLodBias = 0\ninfo:    d3d9.samplerLodBias = 0\n",
            ),
        ] {
            assert!(effective_options(invalid.as_bytes(), config).is_err());
        }
        assert!(effective_options(valid.trim_end().as_bytes(), config).is_err());
        assert!(complete_line_prefix(b"info:  Pending first line").is_err());
        let incomplete_option = &valid.as_bytes()[..valid.find("false").unwrap() + 2];
        assert!(
            effective_options(complete_line_prefix(incomplete_option).unwrap(), config).is_err()
        );
        assert!(
            effective_options(valid.as_bytes(), Path::new("/other/dxvk-sampler.conf")).is_err()
        );
    }

    #[test]
    fn native_environment_keeps_only_whitelisted_values_and_rejects_duplicates() {
        let selected = environment_values(b"PRIVATE_TOKEN=not-retained\0DXVK_CONFIG_FILE=/owned/config\0DXVK_LOG_LEVEL=info\0DXVK_LOG_PATH=/owned/logs\0").unwrap();
        assert_eq!(selected.len(), ENVIRONMENT.len());
        assert_eq!(selected["DXVK_CONFIG"], None);
        assert_eq!(selected["DXVK_LOG_LEVEL"].as_deref(), Some("info"));
        assert!(environment_values(b"DXVK_LOG_LEVEL=info\0DXVK_LOG_LEVEL=info\0").is_err());
        assert!(environment_values(b"DXVK_LOG_LEVEL=info").is_err());
        let override_present = environment_values(b"DXVK_CONFIG=\0").unwrap();
        assert_eq!(override_present["DXVK_CONFIG"].as_deref(), Some(""));
    }

    #[test]
    fn log_requires_one_actual_process_descriptor_and_unchanged_final_prefix() {
        let directory = private_directory();
        let profile = prepared(directory.path());
        let path = profile.logs.join("native_d3d9.log");
        let mut writer = File::create(&path).unwrap();
        let bytes = log(&profile.config);
        writer.write_all(&bytes).unwrap();
        let (identity, _) = owned_file(&path, MAX_LOG).unwrap();
        let actual = log_descriptor(&profile.logs, &path, &identity, std::process::id()).unwrap();
        assert_eq!(actual, writer.as_raw_fd() as u32);
        let duplicate = writer.try_clone().unwrap();
        assert!(log_descriptor(&profile.logs, &path, &identity, std::process::id()).is_err());
        drop(duplicate);
        drop(writer);
        assert!(log_descriptor(&profile.logs, &path, &identity, std::process::id()).is_err());
        let ready = Ready {
            native_pid: std::process::id(),
            native_start: "synthetic-test".into(),
            config_sha256_ready: util::hash(CONFIG_BYTES),
            loaded_maps_sha256: "synthetic-test".into(),
            environment: profile.expected_environment(),
            log: LogObservation {
                path: path.clone(),
                descriptor: actual,
                identity,
                ready_bytes: bytes.len(),
                ready_sha256: util::hash(&bytes),
            },
        };
        let mut changed = bytes;
        changed[0] = b'x';
        fs::write(path, changed).unwrap();
        assert!(profile.finish(ready).is_err());

        let complete = private_directory();
        let profile = prepared(complete.path());
        let path = profile.logs.join("native_d3d9.log");
        let bytes = log(&profile.config);
        let mut streaming = bytes.clone();
        streaming.extend_from_slice(b"info:  D3D9: Resources");
        fs::write(&path, &streaming).unwrap();
        let (identity, _) = owned_file(&path, MAX_LOG).unwrap();
        let prefix = complete_line_prefix(&streaming).unwrap();
        assert_eq!(prefix, bytes.as_slice());
        assert!(effective_options(&streaming, &profile.config).is_err());
        effective_options(prefix, &profile.config).unwrap();
        let ready = Ready {
            native_pid: std::process::id(),
            native_start: "synthetic-test".into(),
            config_sha256_ready: util::hash(CONFIG_BYTES),
            loaded_maps_sha256: "synthetic-test".into(),
            environment: profile.expected_environment(),
            log: LogObservation {
                path: path.clone(),
                descriptor: 0,
                identity,
                ready_bytes: prefix.len(),
                ready_sha256: util::hash(prefix),
            },
        };
        streaming.extend_from_slice(b" released\n");
        fs::write(path, streaming).unwrap();
        let delivery = profile.finish(ready).unwrap();
        assert_eq!(delivery.config_sha256_before, delivery.config_sha256_after);
        assert_eq!(delivery.effective_options.len(), OPTIONS.len());
        assert!(!delivery.live_sampler_observed);
    }
}
