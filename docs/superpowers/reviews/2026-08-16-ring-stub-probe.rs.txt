//! Signature-only `ring` stub. PROBE ONLY — never committed, never linked into a real build.
//! Exists solely so a windows-msvc `cargo check` can cross-compile without ring's C build script.
#[derive(Debug)]
pub struct Unspecified;

pub mod error {
    pub use super::Unspecified;
}

pub mod rand {
    pub trait SecureRandom {
        fn fill(&self, dest: &mut [u8]) -> Result<(), super::Unspecified>;
    }
    pub struct SystemRandom;
    impl SystemRandom {
        pub fn new() -> Self {
            Self
        }
    }
    impl Default for SystemRandom {
        fn default() -> Self {
            Self::new()
        }
    }
    impl SecureRandom for SystemRandom {
        fn fill(&self, _dest: &mut [u8]) -> Result<(), super::Unspecified> {
            Ok(())
        }
    }
}

pub mod digest {
    pub struct Algorithm;
    pub static SHA256: Algorithm = Algorithm;

    pub struct Digest([u8; 32]);
    impl AsRef<[u8]> for Digest {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }

    pub fn digest(_algorithm: &'static Algorithm, _data: &[u8]) -> Digest {
        Digest([0_u8; 32])
    }

    pub struct Context;
    impl Context {
        pub fn new(_algorithm: &'static Algorithm) -> Self {
            Self
        }
        pub fn update(&mut self, _data: &[u8]) {}
        pub fn finish(self) -> Digest {
            Digest([0_u8; 32])
        }
    }
}

pub mod hmac {
    #[derive(Clone, Copy)]
    pub struct Algorithm;
    pub static HMAC_SHA256: Algorithm = Algorithm;

    pub struct Key;
    impl Key {
        pub fn new(_algorithm: Algorithm, _value: &[u8]) -> Self {
            Self
        }
    }

    pub struct Tag([u8; 32]);
    impl AsRef<[u8]> for Tag {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }

    pub fn sign(_key: &Key, _data: &[u8]) -> Tag {
        Tag([0_u8; 32])
    }
}
