// This key may be used for easy testing during development.
// A dedicated feature has to be activated for this key to work
// in order to prevent accidentally shipping this key in real builds.
//
// Private key:
// -----BEGIN PRIVATE KEY-----
// MC4CAQAwBQYDK2VwBCIEIJD+MVAP52Ml4MgPNSeZdR5tU8j8dwUFG+JCbpaUS3Tu
// -----END PRIVATE KEY-----
#[cfg(feature = "insecure-trust-test-key")]
pub const TEST_PUBLIC_KEY: &[u8; 32] = b"\xDA\xA2\x09\x58\xE0\x66\x21\xAE\x7A\x71\xB1\x04\xE1\xEC\xF2\x05\xD9\xB8\x6A\x1D\x12\xBE\x2D\xA3\x84\x8D\x0C\x36\xD8\x74\xFA\x34";
