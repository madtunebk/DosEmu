use std::error::Error;
use std::future::Future;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// QEMU claims its bus name while starting; give it a moment.
pub const REGISTER_TIMEOUT: Duration = Duration::from_secs(5);

/// Run `setup` on a thread with its own tokio runtime and wait until it succeeds or fails.
/// On success what it returns (the D-Bus connections) is kept alive there for the VM's life,
/// so QEMU's calls keep being dispatched.
pub fn run_on_own_thread<F, Fut, T>(what: &'static str, setup: F) -> Result<(), Box<dyn Error>>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = zbus::Result<T>>,
    T: 'static,
{
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(err) => return drop(ready_tx.send(Err(err.to_string()))),
        };
        runtime.block_on(async move {
            match setup().await {
                Ok(keep) => {
                    let _ = ready_tx.send(Ok(()));
                    let _keep = keep;
                    std::future::pending::<()>().await;
                }
                Err(err) => drop(ready_tx.send(Err(err.to_string()))),
            }
        });
    });
    match ready_rx.recv_timeout(REGISTER_TIMEOUT + Duration::from_secs(1)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
        Err(_) => Err(format!("timed out registering the D-Bus {what} listener").into()),
    }
}

/// Serve `listener` at `listener_path` on a private peer-to-peer connection and hand its other
/// end to QEMU by calling `method(fd)` on `path`/`interface` (e.g. Console.RegisterListener).
/// Returns the peer connection, which dispatches QEMU's calls while it is held.
pub async fn register_p2p<I: zbus::object_server::Interface>(
    bus: &zbus::Connection,
    path: &str,
    interface: &str,
    method: &str,
    listener_path: &str,
    listener: I,
) -> zbus::Result<zbus::Connection> {
    // QEMU authenticates the socket inside the register call, so the call and our side of the
    // handshake must run together.
    let (ours, theirs) = UnixStream::pair().map_err(zbus::Error::from)?;
    ours.set_nonblocking(true).map_err(zbus::Error::from)?;
    let ours = tokio::net::UnixStream::from_std(ours).map_err(zbus::Error::from)?;
    let peer = zbus::connection::Builder::unix_stream(ours)
        .p2p()
        .serve_at(listener_path, listener)?
        .build();

    let deadline = Instant::now() + REGISTER_TIMEOUT;
    let call = async {
        loop {
            let result = bus
                .call_method(Some("org.qemu"), path, Some(interface), method, &(zbus::zvariant::Fd::from(&theirs),))
                .await;
            match result {
                // The name isn't on the bus until QEMU's display backend is up.
                Err(zbus::Error::MethodError(name, ..))
                    if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" && Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                other => return other.map(drop),
            }
        }
    };
    let (registered, peer) = tokio::join!(call, peer);
    registered?;
    peer
}
