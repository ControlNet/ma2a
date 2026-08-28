//! Full-domain and adversarial protocol-version decoder coverage.

use ma2a_core::{ProtocolError, decode_request, decode_response};

const REQUEST: &[u8] = include_bytes!("../testdata/wire-v1/echo-request.cbor");
const RESPONSE: &[u8] = include_bytes!("../testdata/wire-v1/echo-response.cbor");

#[test]
fn request_canonical_boundary_versions_are_version_mismatch() -> Result<(), ProtocolError> {
    // Given
    let versions = [(24, 0), (255, 0), (1, 24), (1, 255)];

    for (major, minor) in versions {
        let bytes = with_version(REQUEST, major, minor)?;

        // When
        let result = decode_request(&bytes);

        // Then
        assert_eq!(result, Err(ProtocolError::VERSION_MISMATCH));
    }
    Ok(())
}

#[test]
fn response_canonical_boundary_versions_are_version_mismatch() -> Result<(), ProtocolError> {
    // Given
    let versions = [(24, 0), (255, 0), (1, 24), (1, 255)];

    for (major, minor) in versions {
        let bytes = with_version(RESPONSE, major, minor)?;

        // When
        let result = decode_response(&bytes);

        // Then
        assert_eq!(result, Err(ProtocolError::VERSION_MISMATCH));
    }
    Ok(())
}

#[test]
fn request_versions_classify_the_entire_u8_domain() -> Result<(), ProtocolError> {
    for major in u8::MIN..=u8::MAX {
        for minor in u8::MIN..=u8::MAX {
            // Given
            let bytes = with_version(REQUEST, major, minor)?;

            // When
            let result = decode_request(&bytes).map(|_| ());

            // Then
            let expected = if (major, minor) == (1, 0) {
                Ok(())
            } else {
                Err(ProtocolError::VERSION_MISMATCH)
            };
            assert_eq!(result, expected, "version {major}.{minor}");
        }
    }
    Ok(())
}

#[test]
fn response_versions_classify_the_entire_u8_domain() -> Result<(), ProtocolError> {
    for major in u8::MIN..=u8::MAX {
        for minor in u8::MIN..=u8::MAX {
            // Given
            let bytes = with_version(RESPONSE, major, minor)?;

            // When
            let result = decode_response(&bytes).map(|_| ());

            // Then
            let expected = if (major, minor) == (1, 0) {
                Ok(())
            } else {
                Err(ProtocolError::VERSION_MISMATCH)
            };
            assert_eq!(result, expected, "version {major}.{minor}");
        }
    }
    Ok(())
}

#[test]
fn request_malformed_version_values_are_invalid_input() -> Result<(), ProtocolError> {
    for value in malformed_versions() {
        for version in [raw_version(&value, &[0]), raw_version(&[1], &value)] {
            // Given
            let bytes = replace_version(REQUEST, &version)?;

            // When
            let result = decode_request(&bytes);

            // Then
            assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
        }
    }
    Ok(())
}

#[test]
fn response_malformed_version_values_are_invalid_input() -> Result<(), ProtocolError> {
    for value in malformed_versions() {
        for version in [raw_version(&value, &[0]), raw_version(&[1], &value)] {
            // Given
            let bytes = replace_version(RESPONSE, &version)?;

            // When
            let result = decode_response(&bytes);

            // Then
            assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
        }
    }
    Ok(())
}

fn with_version(golden: &[u8], major: u8, minor: u8) -> Result<Vec<u8>, ProtocolError> {
    replace_version(
        golden,
        &raw_version(&canonical_u8(major), &canonical_u8(minor)),
    )
}

fn canonical_u8(value: u8) -> Vec<u8> {
    if value <= 23 {
        vec![value]
    } else {
        vec![0x18, value]
    }
}

fn malformed_versions() -> Vec<Vec<u8>> {
    let mut values = (0u8..=23)
        .map(|value| vec![0x18, value])
        .collect::<Vec<_>>();
    values.extend([
        vec![0x18],
        vec![0x19, 0x00, 0x18],
        vec![0x1a, 0x00, 0x00, 0x00, 0x18],
        vec![0x1b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18],
        vec![0x20],
        vec![0xf9, 0x00, 0x00],
        vec![0xc0, 0x00],
        vec![0x60],
        vec![0x80],
    ]);
    values
}

fn raw_version(major: &[u8], minor: &[u8]) -> Vec<u8> {
    let mut version = Vec::with_capacity(major.len() + minor.len());
    version.extend_from_slice(major);
    version.extend_from_slice(minor);
    version
}

fn replace_version(golden: &[u8], version: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let prefix = golden.get(..3).ok_or(ProtocolError::INVALID_INPUT)?;
    let suffix = golden.get(5..).ok_or(ProtocolError::INVALID_INPUT)?;
    let mut bytes = Vec::with_capacity(golden.len() + version.len());
    bytes.extend_from_slice(prefix);
    bytes.extend_from_slice(version);
    bytes.extend_from_slice(suffix);
    Ok(bytes)
}
