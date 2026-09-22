//! Tekil istek ID üretimi. PLAN §3: ID + ses_hash idempotency anahtarıdır,
//! aynı ID farklı sesle gelirse yeni istek sayılır.

use uuid::Uuid;

/// Yeni tekil istek ID'si (uuid v4, hex).
pub fn new_request_id() -> String {
    Uuid::new_v4().simple().to_string()
}

/// ID biçimi geçerli mi (32 hex karakter)?
pub fn is_valid(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn request_ids_are_unique() {
        let n = 10_000;
        let mut set = HashSet::with_capacity(n);
        for _ in 0..n {
            let id = new_request_id();
            assert!(is_valid(&id), "gecersiz bicim: {id}");
            assert!(set.insert(id), "tekil ID cakisti");
        }
        assert_eq!(set.len(), n);
    }

    #[test]
    fn rejects_malformed_ids() {
        assert!(!is_valid(""));
        assert!(!is_valid("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"));
        assert!(!is_valid("abc"));
    }
}
