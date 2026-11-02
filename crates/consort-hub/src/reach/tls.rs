//! The TLS listener of This network: a self-signed certificate that the hub
//! makes one time and keeps, and a second port for as long as This network is
//! on. It serves the same routes as the main port.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use consort_proto::model::Sha256Hash;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

use crate::error::{HubError, Result};
use crate::hub::Hub;

/// The private key of the certificate, in the data folder, with mode 0600.
pub const TLS_KEY_FILE: &str = "reach-tls.key";
/// The certificate, in the data folder.
pub const TLS_CERT_FILE: &str = "reach-tls.crt";

/// How long a TLS handshake may take.
const HANDSHAKE_LIMIT: Duration = Duration::from_secs(10);
/// The most TLS handshakes at the same time. This port is open to the network.
const MAX_HANDSHAKES: usize = 64;

/// The certificate and the key of This network.
pub(super) struct TlsIdentity {
    cert: Vec<u8>,
    key: Vec<u8>,
}

impl TlsIdentity {
    /// The SHA-256 that peers pin.
    pub(super) fn pin(&self) -> Sha256Hash {
        consort_tls::certificate_sha256(&self.cert)
    }

    pub(super) fn server_config(&self) -> Result<Arc<ServerConfig>> {
        let cert = CertificateDer::from(self.cert.clone());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone()));
        let tls_error = |err: rustls::Error| {
            HubError::internal(format!("the certificate of the hub cannot be used: {err}"))
        };
        let mut config = ServerConfig::builder_with_provider(Arc::new(consort_tls::provider()))
            .with_safe_default_protocol_versions()
            .map_err(tls_error)?
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .map_err(tls_error)?;
        // A WebSocket upgrade needs HTTP/1.1.
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }
}

/// The saved certificate, when both files are there.
pub(super) fn load_identity(data_dir: &Path) -> Option<TlsIdentity> {
    let cert = std::fs::read(data_dir.join(TLS_CERT_FILE)).ok()?;
    let key = std::fs::read(data_dir.join(TLS_KEY_FILE)).ok()?;
    Some(TlsIdentity { cert, key })
}

/// The saved certificate, or a new one. The pin stays the same for as long as
/// the files stay, also while This network is off.
pub(super) fn load_or_create_identity(data_dir: &Path, host: &str) -> Result<TlsIdentity> {
    if let Some(identity) = load_identity(data_dir) {
        return Ok(identity);
    }
    let made = rcgen::generate_simple_self_signed(vec![host.to_owned()]).map_err(|err| {
        HubError::internal(format!("the hub could not make a certificate: {err}"))
    })?;
    let identity = TlsIdentity {
        cert: made.cert.der().to_vec(),
        key: made.signing_key.serialize_der(),
    };
    crate::util::write_private_file(&data_dir.join(TLS_KEY_FILE), &identity.key)?;
    crate::util::write_private_file(&data_dir.join(TLS_CERT_FILE), &identity.cert)?;
    Ok(identity)
}

/// A stream that fails when This network turns off, so that open links end at
/// once and do not stay on a port that the user closed.
struct Stoppable<S> {
    inner: S,
    stopped: Pin<Box<WaitForCancellationFutureOwned>>,
}

impl<S> Stoppable<S> {
    fn new(inner: S, stop: &CancellationToken) -> Self {
        Self {
            inner,
            stopped: Box::pin(stop.clone().cancelled_owned()),
        }
    }

    fn is_stopped(&mut self, cx: &mut Context<'_>) -> bool {
        self.stopped.as_mut().poll(cx).is_ready()
    }
}

fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "This network turned off")
}

impl<S: AsyncRead + Unpin> AsyncRead for Stoppable<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.is_stopped(cx) {
            return Poll::Ready(Err(closed()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Stoppable<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.is_stopped(cx) {
            return Poll::Ready(Err(closed()));
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

type TlsStream = Stoppable<tokio_rustls::server::TlsStream<TcpStream>>;

/// The connections that finished the TLS handshake.
struct Accepted {
    ready: mpsc::Receiver<(TlsStream, SocketAddr)>,
    local: SocketAddr,
}

impl axum::serve::Listener for Accepted {
    type Io = TlsStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(ready) => ready,
            // The accept loop ended, so the listener stopped. Only the
            // graceful shutdown ends the wait.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// Gives each connection its peer address, as the main listener does. The
/// pairing limit needs it. axum has this only for a plain TCP listener.
#[derive(Clone)]
struct WithPeerAddress(axum::Router);

impl tower_service::Service<axum::serve::IncomingStream<'_, Accepted>> for WithPeerAddress {
    type Response = axum::Router;
    type Error = std::convert::Infallible;
    type Future = std::future::Ready<std::result::Result<axum::Router, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, stream: axum::serve::IncomingStream<'_, Accepted>) -> Self::Future {
        let peer = axum::extract::ConnectInfo(*stream.remote_addr());
        std::future::ready(Ok(self.0.clone().layer(axum::Extension(peer))))
    }
}

/// A TLS port that is open now.
pub(super) struct NetworkListener {
    stop: CancellationToken,
    server: JoinHandle<()>,
    accept: JoinHandle<()>,
    pub(super) port: u16,
}

impl NetworkListener {
    /// Opens the port on every interface. Port 0 takes a free port.
    pub(super) async fn start(hub: &Hub, identity: &TlsIdentity, port: u16) -> Result<Self> {
        let tcp = TcpListener::bind(("0.0.0.0", port)).await.map_err(|err| {
            HubError::Startup(format!(
                "could not listen on port {port}: {err}. Turn This network off and on, or stop the program that uses the port."
            ))
        })?;
        let local = tcp.local_addr()?;
        let acceptor = tokio_rustls::TlsAcceptor::from(identity.server_config()?);
        let stop = hub.shutdown.child_token();
        let (send, ready) = mpsc::channel(MAX_HANDSHAKES);
        let accept = tokio::spawn(accept_loop(tcp, acceptor, send, stop.clone()));
        let app = crate::net::router(hub.clone());
        let server = tokio::spawn({
            let stop = stop.clone();
            async move {
                let served = axum::serve(Accepted { ready, local }, WithPeerAddress(app))
                    .with_graceful_shutdown(stop.cancelled_owned())
                    .await;
                if let Err(err) = served {
                    tracing::warn!(error = %err, "the TLS listener stopped");
                }
            }
        });
        Ok(Self {
            stop,
            server,
            accept,
            port: local.port(),
        })
    }

    /// Closes the port and every connection on it.
    pub(super) async fn stop(self) {
        self.stop.cancel();
        let _ = self.accept.await;
        if tokio::time::timeout(Duration::from_secs(5), self.server)
            .await
            .is_err()
        {
            tracing::warn!("the TLS listener did not stop in 5 seconds");
        }
    }
}

async fn accept_loop(
    tcp: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    ready: mpsc::Sender<(TlsStream, SocketAddr)>,
    stop: CancellationToken,
) {
    let slots = Arc::new(Semaphore::new(MAX_HANDSHAKES));
    loop {
        let accepted = tokio::select! {
            () = stop.cancelled() => return,
            accepted = tcp.accept() => accepted,
        };
        let (socket, peer) = match accepted {
            Ok(accepted) => accepted,
            Err(err) => {
                tracing::debug!(error = %err, "the TLS listener could not accept");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let (acceptor, ready, stop) = (acceptor.clone(), ready.clone(), stop.clone());
        tokio::spawn(async move {
            let _slot = slot;
            if let Ok(Ok(stream)) =
                tokio::time::timeout(HANDSHAKE_LIMIT, acceptor.accept(socket)).await
            {
                let _ = ready.send((Stoppable::new(stream, &stop), peer)).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_certificate_stays_and_its_key_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create_identity(dir.path(), "mac.local").unwrap();
        let again = load_or_create_identity(dir.path(), "other.local").unwrap();
        assert_eq!(first.pin(), again.pin());
        assert!(first.server_config().is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.path().join(TLS_KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_new_certificate_comes_when_one_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create_identity(dir.path(), "mac.local").unwrap();
        std::fs::remove_file(dir.path().join(TLS_CERT_FILE)).unwrap();
        let second = load_or_create_identity(dir.path(), "mac.local").unwrap();
        assert_ne!(first.pin(), second.pin());
    }
}
