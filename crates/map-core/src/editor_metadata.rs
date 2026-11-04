pub fn is_storage_key(key: &str) -> bool {
    crate::context::is_storage_key(key) || crate::instance_order::is_storage_key(key)
}
