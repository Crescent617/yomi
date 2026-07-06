use rand::RngExt;

const BASE56_CHARS: &[u8] = b"23456789abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ";

/// Generate a random base56 ID of specified length
pub fn gen_base56_id(len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| {
            let idx = rng.random_range(0..BASE56_CHARS.len());
            BASE56_CHARS[idx] as char
        })
        .collect()
}

#[cfg(test)]
#[path = "id_test.rs"]
mod tests;
