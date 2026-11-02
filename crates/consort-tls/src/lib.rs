//! The TLS pin of This network.
//!
//! A hub on This network has a self-signed certificate. A peer learns the
//! SHA-256 of that certificate from a pairing code or a pairing link, and
//! then accepts this one certificate and no other. The name and the dates in
//! the certificate do not count, because the pin already names it.
//!
//! The client and `consort-link` both use this crate, so each path to the hub
//! has one rule: the WebSocket, pairing and blobs.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::error::Error;
use std::sync::Arc;

use consort_proto::model::Sha256Hash;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms, aws_lc_rs};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, OtherError, SignatureScheme};
use sha2::{Digest, Sha256};

pub use rustls;

/// The error text of a certificate that is not the pinned one. A caller finds
/// it with [`certificate_problem`].
#[derive(Debug, thiserror::Error)]
#[error("the certificate of the hub is not the pinned certificate")]
pub struct PinMismatch;

/// The SHA-256 of a certificate in DER form.
pub fn certificate_sha256(der: &[u8]) -> Sha256Hash {
    let mut hasher = Sha256::new();
    hasher.update(der);
    Sha256Hash::from_bytes(hasher.finalize().into())
}

/// Makes the process use one crypto provider for TLS. Two dependencies can
/// turn on two providers in `rustls`, and then a client with no explicit
/// provider cannot start. Call it before a connection that does not use a pin.
pub fn use_default_provider() {
    // An error means that a provider is already there, which is the goal.
    let _ = aws_lc_rs::default_provider().install_default();
}

/// Accepts one certificate, by its SHA-256.
#[derive(Debug)]
pub struct PinnedVerifier {
    pin: [u8; 32],
    algorithms: WebPkiSupportedAlgorithms,
}

impl PinnedVerifier {
    /// A verifier for the certificate with this SHA-256.
    pub fn new(pin: &Sha256Hash) -> Self {
        Self {
            pin: pin.to_bytes(),
            algorithms: aws_lc_rs::default_provider().signature_verification_algorithms,
        }
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if certificate_sha256(end_entity.as_ref()).to_bytes() == self.pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(CertificateError::Other(
                OtherError(Arc::new(PinMismatch)),
            )))
        }
    }

    // The signatures still prove that the peer holds the private key of the
    // pinned certificate.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// A TLS client configuration that accepts only the certificate with this
/// SHA-256.
pub fn pinned_client_config(pin: &Sha256Hash) -> Result<ClientConfig, rustls::Error> {
    let provider = Arc::new(provider());
    Ok(ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedVerifier::new(pin)))
        .with_no_client_auth())
}

/// The crypto provider that Consort uses for its own TLS configurations.
pub fn provider() -> CryptoProvider {
    aws_lc_rs::default_provider()
}

/// What is wrong with the certificate of a hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CertificateProblem {
    /// It is not the certificate that the pin names.
    NotThePin,
    /// No trusted authority signed it. A self-signed hub needs a pin.
    Untrusted,
}

/// Finds a certificate problem in the chain of an error from `reqwest` or
/// `tungstenite`. Both libraries wrap the `rustls` error at a different depth,
/// so this reads the text of each level. `None` for any other failure.
pub fn certificate_problem(err: &(dyn Error + 'static)) -> Option<CertificateProblem> {
    let mut problem = None;
    let mut level = Some(err);
    while let Some(current) = level {
        // An `io::Error` hides the error that it holds from `source`.
        let inner = current
            .downcast_ref::<std::io::Error>()
            .and_then(|io| io.get_ref())
            .map(|inner| inner as &(dyn Error + 'static));
        for candidate in [Some(current), inner].into_iter().flatten() {
            let found = problem_in(candidate);
            // An outer level only shows the text of the inner error, so a
            // deeper level can say more.
            if found == Some(CertificateProblem::NotThePin) {
                return found;
            }
            problem = problem.or(found);
        }
        // `reqwest` holds an `io::Error` inside an `io::Error`, so the inner
        // error is the next level.
        level = inner.or_else(|| current.source());
    }
    problem
}

fn problem_in(err: &(dyn Error + 'static)) -> Option<CertificateProblem> {
    if let Some(rustls::Error::InvalidCertificate(certificate)) =
        err.downcast_ref::<rustls::Error>()
    {
        return Some(match certificate {
            CertificateError::Other(other) if other.0.downcast_ref::<PinMismatch>().is_some() => {
                CertificateProblem::NotThePin
            }
            _ => CertificateProblem::Untrusted,
        });
    }
    let text = err.to_string();
    if text.contains(&PinMismatch.to_string()) || text.contains("PinMismatch") {
        Some(CertificateProblem::NotThePin)
    } else if text.contains("invalid peer certificate") {
        Some(CertificateProblem::Untrusted)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verifier_answer(pin: &Sha256Hash, der: &[u8]) -> Result<ServerCertVerified, rustls::Error> {
        PinnedVerifier::new(pin).verify_server_cert(
            &CertificateDer::from(der.to_vec()),
            &[],
            &ServerName::try_from("hub.local").unwrap(),
            &[],
            UnixTime::now(),
        )
    }

    /// `reqwest` holds an `io::Error` inside an `io::Error`, and the text of the
    /// outer one does not name the pin.
    #[test]
    fn a_pin_error_inside_two_io_errors_is_found() {
        let rustls_error = rustls::Error::InvalidCertificate(CertificateError::Other(OtherError(
            Arc::new(PinMismatch),
        )));
        let inner = std::io::Error::new(std::io::ErrorKind::InvalidData, rustls_error);
        let outer = std::io::Error::other(inner);
        assert_eq!(
            certificate_problem(&outer),
            Some(CertificateProblem::NotThePin)
        );
    }

    #[test]
    fn the_verifier_accepts_only_the_pinned_certificate() {
        let pin = certificate_sha256(b"the certificate");
        assert!(verifier_answer(&pin, b"the certificate").is_ok());
        let err = verifier_answer(&pin, b"another certificate").unwrap_err();
        assert_eq!(
            certificate_problem(&err),
            Some(CertificateProblem::NotThePin)
        );
    }

    #[test]
    fn a_problem_is_found_through_an_io_error() {
        let wrapped = std::io::Error::other(rustls::Error::InvalidCertificate(
            CertificateError::UnknownIssuer,
        ));
        assert_eq!(
            certificate_problem(&wrapped),
            Some(CertificateProblem::Untrusted)
        );
        assert_eq!(
            certificate_problem(&std::io::Error::other("connection refused")),
            None
        );
    }
}
