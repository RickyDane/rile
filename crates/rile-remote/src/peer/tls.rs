//! Mutual TLS 1.3 between Rile devices. Certificates are self-signed, so the
//! verifiers only check that the peer owns the key of the certificate it
//! presented; whether that device is trusted is decided by its id
//! ([`super::identity::fingerprint`]) against the paired-devices list.

use std::sync::Arc;

use anyhow::Result;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CommonState, DigitallySignedStruct, DistinguishedName, SignatureScheme};

use super::identity::{Identity, fingerprint};

/// SNI name used for every peer; the verifiers ignore it.
pub(crate) const SERVER_NAME: &str = "rile.local";
const PAIR_LABEL: &[u8] = b"EXPORTER-rile-pair";

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

#[derive(Debug)]
struct AnyKeyHolder(WebPkiSupportedAlgorithms);

impl AnyKeyHolder {
    fn new() -> Arc<Self> {
        Arc::new(Self(provider().signature_verification_algorithms))
    }
}

impl ServerCertVerifier for AnyKeyHolder {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

impl ClientCertVerifier for AnyKeyHolder {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

pub(crate) fn server_config(identity: &Identity) -> Result<Arc<rustls::ServerConfig>> {
    let config = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(AnyKeyHolder::new())
        .with_single_cert(vec![identity.cert.clone()], identity.key())?;
    Ok(Arc::new(config))
}

pub(crate) fn client_config(identity: &Identity) -> Result<Arc<rustls::ClientConfig>> {
    let config = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(AnyKeyHolder::new())
        .with_client_auth_cert(vec![identity.cert.clone()], identity.key())?;
    Ok(Arc::new(config))
}

/// Id of the device on the other end of an established connection.
pub(crate) fn remote_id(conn: &CommonState) -> Option<String> {
    conn.peer_certificates().and_then(|c| c.first()).map(|c| fingerprint(c))
}

/// The 6-digit pairing code both ends show. It is derived from this TLS
/// session's secrets, so a device in the middle (two sessions) would make the
/// two screens show different codes.
pub(crate) fn pairing_code(exported: &[u8; 32], a: &str, b: &str) -> u32 {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, exported);
    let tag = ring::hmac::sign(&key, format!("{lo}\n{hi}").as_bytes());
    let n = u32::from_be_bytes(tag.as_ref()[..4].try_into().unwrap_or_default());
    n % 1_000_000
}

pub(crate) fn export(conn: &rustls::ConnectionCommon<impl Sized>) -> Result<[u8; 32]> {
    Ok(conn.export_keying_material([0u8; 32], PAIR_LABEL, None)?)
}

/// `123 456`
pub fn format_code(code: u32) -> String {
    format!("{:03} {:03}", code / 1000, code % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_code_is_symmetric_and_six_digits() {
        let secret = [7u8; 32];
        let a = pairing_code(&secret, "aaaa", "bbbb");
        assert_eq!(a, pairing_code(&secret, "bbbb", "aaaa"));
        assert!(a < 1_000_000);
        assert_ne!(a, pairing_code(&[8u8; 32], "aaaa", "bbbb"));
        assert_eq!(format_code(42), "000 042");
    }
}
