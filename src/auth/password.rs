use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};

pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2.hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool, argon2::password_hash::Error> {
    let parsed_hash = PasswordHash::new(hash)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_hashing_roundtrip() {
        let password = "test-password-123!";
        let hash = hash_password(password).expect("should hash");
        assert!(verify_password(password, &hash).expect("should verify"));
    }

    #[test]
    fn test_password_hashing_wrong_password_fails() {
        let hash = hash_password("correct-password").expect("should hash");
        assert!(!verify_password("wrong-password", &hash).expect("should verify"));
    }

    #[test]
    fn test_password_hashes_are_unique() {
        let h1 = hash_password("same").expect("hash 1");
        let h2 = hash_password("same").expect("hash 2");
        assert_ne!(h1, h2, "salts should produce different hashes");
    }
}
