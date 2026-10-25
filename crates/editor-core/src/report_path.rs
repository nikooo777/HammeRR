use std::path::Path;

const MAX_PATH_BYTES: usize = 16 * 1024;

pub(crate) fn checked(path: &Path) -> Result<&str, &'static str> {
    path.to_str()
        .filter(|path| path.len() <= MAX_PATH_BYTES)
        .ok_or("Path cannot be represented as UTF-8 JSON or exceeds the report path budget")
}
