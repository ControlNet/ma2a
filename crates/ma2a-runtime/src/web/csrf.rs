use std::fmt::Write as _;

use blake3::Hasher;
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

use super::AuthFailure;

const BEARER_DOMAIN: &[u8] = b"ma2a-web-bearer-v1\0";
const CSRF_DOMAIN: &[u8] = b"ma2a-web-csrf-v1\0";
const TOKEN_BYTES: usize = 32;
const TOKEN_HEX_BYTES: usize = TOKEN_BYTES * 2;

pub(super) struct TokenPair {
    pub(super) bearer: Zeroizing<String>,
    pub(super) bearer_digest: [u8; 32],
    pub(super) csrf: Zeroizing<String>,
    pub(super) csrf_digest: [u8; 32],
}

pub(super) fn generate_pair() -> Result<TokenPair, AuthFailure> {
    let mut bearer = Zeroizing::new([0_u8; TOKEN_BYTES]);
    let mut csrf = Zeroizing::new([0_u8; TOKEN_BYTES]);
    getrandom::fill(bearer.as_mut()).map_err(|_| AuthFailure::Internal)?;
    getrandom::fill(csrf.as_mut()).map_err(|_| AuthFailure::Internal)?;
    Ok(TokenPair {
        bearer: Zeroizing::new(encode_hex(&bearer)),
        bearer_digest: digest(BEARER_DOMAIN, &bearer),
        csrf: Zeroizing::new(encode_hex(&csrf)),
        csrf_digest: digest(CSRF_DOMAIN, &csrf),
    })
}

pub(super) fn bearer_digest(encoded: &str) -> Result<[u8; 32], AuthFailure> {
    decode_hex(encoded).map(|token| digest(BEARER_DOMAIN, &token))
}

pub(super) fn csrf_matches(encoded: &str, expected: &[u8; 32]) -> bool {
    decode_hex(encoded)
        .map(|token| bool::from(digest(CSRF_DOMAIN, &token).ct_eq(expected)))
        .unwrap_or(false)
}

fn digest(domain: &[u8], token: &[u8; TOKEN_BYTES]) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(domain);
    hasher.update(token);
    *hasher.finalize().as_bytes()
}

fn encode_hex(bytes: &[u8; TOKEN_BYTES]) -> String {
    let mut encoded = String::with_capacity(TOKEN_HEX_BYTES);
    for byte in bytes {
        let _result = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn decode_hex(encoded: &str) -> Result<[u8; TOKEN_BYTES], AuthFailure> {
    if encoded.len() != TOKEN_HEX_BYTES
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AuthFailure::Unauthorized);
    }
    let mut token = [0_u8; TOKEN_BYTES];
    for (destination, pair) in token.iter_mut().zip(encoded.as_bytes().chunks_exact(2)) {
        let pair = std::str::from_utf8(pair).map_err(|_| AuthFailure::Unauthorized)?;
        *destination = u8::from_str_radix(pair, 16).map_err(|_| AuthFailure::Unauthorized)?;
    }
    Ok(token)
}
