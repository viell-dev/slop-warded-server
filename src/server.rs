use crate::{Error, Limits, Tls, listener, redirect, signal, tcp};
use axum::{extract::Request, http::HeaderValue, http::header::ALT_SVC, response::IntoResponse};
use axum_server::Control;
use std::{
    convert::Infallible,
    future::Future,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    sync::Arc,
    time::Duration,
};
use tokio::{sync::Semaphore, task::JoinSet};
use tokio_util::sync::CancellationToken;
use tower::{Service, util::MapResponseLayer};

/// How long clients may keep using HTTP/3 after the last response that advertised it.
const ALT_SVC_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// A server for one application over HTTP/1.1, HTTP/2 and HTTP/3.
///
/// HTTPS listens on one address: TCP for HTTP/1.1 and HTTP/2, and UDP on the same port for
/// HTTP/3, which HTTPS responses advertise. A second, cleartext listener is optional.
///
/// ```no_run
/// use warded_server::{Server, Tls, axum::{Router, routing::get}};
///
/// # async fn run() -> Result<(), warded_server::Error> {
/// let app = Router::new().route("/", get(|| async { "Hello" }));
/// Server::new(Tls::pem_files("fullchain.pem", "privkey.pem")?)
///     .https("[::]:443".parse().unwrap())
///     .redirect_http("[::]:80".parse().unwrap())
///     .serve(app)
///     .await
/// # }
/// ```
#[derive(Clone, Debug)]
#[must_use = "a server does nothing until it is served"]
pub struct Server {
    tls: Tls,
    https: SocketAddr,
    http: Option<(SocketAddr, Cleartext)>,
    http3: bool,
    public_port: Option<u16>,
    limits: Limits,
    grace_period: Duration,
}

#[derive(Clone, Copy, Debug)]
enum Cleartext {
    Redirect,
    Application,
}

impl Server {
    /// Start from the defaults: HTTPS on `127.0.0.1:8443`, HTTP/3 on, no cleartext listener.
    pub fn new(tls: Tls) -> Self {
        Self {
            tls,
            https: (Ipv4Addr::LOCALHOST, 8443).into(),
            http: None,
            http3: true,
            public_port: None,
            limits: Limits::default(),
            grace_period: Duration::from_secs(30),
        }
    }

    /// Listen for HTTPS on this address. `[::]` accepts IPv4 as well.
    ///
    /// Port 0 takes a free port; [`BoundServer::https_addr`] reports which.
    pub fn https(mut self, address: SocketAddr) -> Self {
        self.https = address;
        self
    }

    /// Listen for cleartext HTTP on this address and redirect every request to HTTPS.
    pub fn redirect_http(mut self, address: SocketAddr) -> Self {
        self.http = Some((address, Cleartext::Redirect));
        self
    }

    /// Listen for cleartext HTTP on this address and serve the application there too.
    ///
    /// Its requests carry the security context `Unsecure`, which Warded's `unsecure()` and
    /// `secured()` conditions read. Nothing about a reverse proxy is trusted.
    pub fn serve_http(mut self, address: SocketAddr) -> Self {
        self.http = Some((address, Cleartext::Application));
        self
    }

    /// Whether to accept and advertise HTTP/3. Without the `http3` feature this has no effect.
    pub fn http3(mut self, enabled: bool) -> Self {
        self.http3 = enabled;
        self
    }

    /// The port clients reach HTTPS on, when forwarding makes it differ from the bound one.
    ///
    /// Redirects and the HTTP/3 advertisement name it. It has to reach this server over UDP as
    /// well as TCP for HTTP/3 to work.
    pub fn public_port(mut self, port: u16) -> Self {
        self.public_port = Some(port);
        self
    }

    /// Replace the default [`Limits`].
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// How long requests in progress may take to finish after shutdown begins.
    ///
    /// Default: 30 seconds.
    pub fn grace_period(mut self, period: Duration) -> Self {
        self.grace_period = period;
        self
    }

    /// Bind every listener, without accepting anything yet.
    ///
    /// Nothing stays bound when one of them fails, or when the [`Limits`] are unusable. Call this
    /// inside a Tokio runtime.
    pub fn bind(self) -> Result<BoundServer, Error> {
        self.limits.validate()?;
        let (https, quic) = self.bind_https()?;
        let http = self
            .http
            .map(|(address, _)| listener::tcp(address, "HTTP over TCP"))
            .transpose()?;
        Ok(BoundServer {
            https_addr: local(&https, self.https)?,
            http_addr: match (&http, self.http) {
                (Some(listener), Some((address, _))) => Some(local(listener, address)?),
                _ => None,
            },
            server: self,
            https,
            http,
            quic,
        })
    }

    /// Bind and serve until the process is asked to stop.
    ///
    /// That is Ctrl+C or, on Unix, `SIGTERM`. Asking again ends the grace period at once. On
    /// Unix, `SIGHUP` reloads the certificate files.
    /// Use [`Server::bind`] and [`BoundServer::serve_until`] to decide both yourself.
    pub async fn serve<S>(self, app: S) -> Result<(), Error>
    where
        S: Service<Request, Error = Infallible> + Clone + Send + Sync + 'static,
        S::Response: IntoResponse,
        S::Future: Send + 'static,
    {
        let tls = self.tls.clone();
        let bound = self.bind()?;
        // From here on, the signals no longer end the process by themselves.
        let mut signals = signal::ShutdownSignals::new();
        let (stop, force) = (CancellationToken::new(), CancellationToken::new());
        let serving = bound.run(app, stop.cancelled(), force.cancelled());
        let stopping = async {
            // The same listener for both, so that neither is lost.
            signals.next().await;
            stop.cancel();
            signals.next().await;
            force.cancel();
            std::future::pending::<()>().await;
        };
        tokio::select! {
            // Listen for signals before anything is served.
            biased;
            () = signal::reload_on_hangup(tls) => unreachable!("it never completes"),
            () = stopping => unreachable!("it never completes"),
            result = serving => result,
        }
    }

    #[cfg(feature = "http3")]
    fn bind_https(&self) -> Result<(TcpListener, Option<std::net::UdpSocket>), Error> {
        /// A free TCP port may be taken for UDP, so look for one that is free for both.
        const ATTEMPTS: usize = 16;

        let mut attempt = 1;
        loop {
            let tcp = listener::tcp(self.https, "HTTPS over TCP")?;
            if !self.http3 {
                return Ok((tcp, None));
            }
            match listener::udp(local(&tcp, self.https)?) {
                Ok(udp) => return Ok((tcp, Some(udp))),
                Err(_) if self.https.port() == 0 && attempt < ATTEMPTS => attempt += 1,
                Err(error) => return Err(error),
            }
        }
    }

    #[cfg(not(feature = "http3"))]
    fn bind_https(&self) -> Result<(TcpListener, Option<Infallible>), Error> {
        Ok((listener::tcp(self.https, "HTTPS over TCP")?, None))
    }
}

fn local(listener: &TcpListener, requested: SocketAddr) -> Result<SocketAddr, Error> {
    listener.local_addr().map_err(|source| Error::Bind {
        listener: "TCP",
        address: requested,
        source,
    })
}

/// A [`Server`] whose listeners are bound.
#[derive(Debug)]
#[must_use = "a bound server accepts nothing until it is served"]
pub struct BoundServer {
    server: Server,
    https: TcpListener,
    https_addr: SocketAddr,
    http: Option<TcpListener>,
    http_addr: Option<SocketAddr>,
    #[cfg(feature = "http3")]
    quic: Option<std::net::UdpSocket>,
    #[cfg(not(feature = "http3"))]
    #[expect(
        dead_code,
        reason = "there is no QUIC socket without the http3 feature"
    )]
    quic: Option<Infallible>,
}

impl BoundServer {
    /// The address HTTPS listens on, over TCP and UDP.
    #[must_use]
    pub fn https_addr(&self) -> SocketAddr {
        self.https_addr
    }

    /// The address of the cleartext listener, if there is one.
    #[must_use]
    pub fn http_addr(&self) -> Option<SocketAddr> {
        self.http_addr
    }

    /// Serve until `shutdown` completes, then let requests in progress finish.
    ///
    /// Connections with requests in progress when the grace period ends are closed. WebSockets and
    /// other upgraded connections do not extend it: once everything else has finished, reading
    /// from them or writing to them fails. Their handlers own them, so they stay open until these
    /// notice and drop them. To end them properly, tell the handlers from `shutdown`. Dropping the future does the same to all connections at once. Returns early
    /// with the error if a listener fails.
    pub async fn serve_until<S>(
        self,
        app: S,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<(), Error>
    where
        S: Service<Request, Error = Infallible> + Clone + Send + Sync + 'static,
        S::Response: IntoResponse,
        S::Future: Send + 'static,
    {
        self.run(app, shutdown, std::future::pending()).await
    }

    /// Serve until `shutdown` completes, and drain until `force` completes.
    async fn run<S>(
        self,
        app: S,
        shutdown: impl Future<Output = ()> + Send,
        force: impl Future<Output = ()> + Send,
    ) -> Result<(), Error>
    where
        S: Service<Request, Error = Infallible> + Clone + Send + Sync + 'static,
        S::Response: IntoResponse,
        S::Future: Send + 'static,
    {
        let Server {
            tls,
            limits,
            grace_period,
            ..
        } = self.server;
        // Axum's router gives every transport the same HEAD and Content-Length handling,
        // whatever kind of service the application is.
        let app = axum::Router::new().fallback_service(app);
        let public_port = self.server.public_port.unwrap_or(self.https_addr.port());
        #[cfg(feature = "http3")]
        let advertise = self.quic.is_some();
        #[cfg(not(feature = "http3"))]
        let advertise = false;

        let slots = Arc::new(Semaphore::new(
            limits.connections.min(Semaphore::MAX_PERMITS),
        ));
        let stopping = Stopping::default();
        let shared = Arc::new(tcp::Shared {
            limits: limits.clone(),
            slots: slots.clone(),
            control: stopping.tcp.clone(),
            abort: stopping.abort.clone(),
        });
        let mut listeners = JoinSet::new();

        if let (Some(listener), Some((_, cleartext))) = (self.http, self.server.http) {
            let app = match cleartext {
                Cleartext::Redirect => redirect::to_https(public_port),
                Cleartext::Application => app.clone(),
            };
            let shared = shared.clone();
            listeners.spawn(async move { tcp::serve_http(listener, app, &shared).await });
        }

        // HTTP/3 clients also need to hear that it is still there.
        let app = if advertise {
            advertise_http3(app, public_port)
        } else {
            app
        };
        #[cfg(feature = "http3")]
        if let Some(socket) = self.quic {
            let http3 = crate::http3::Http3 {
                endpoint: crate::http3::endpoint(socket, &tls, &limits)?,
                limits: limits.clone(),
                slots: slots.clone(),
            };
            let (stop, abort) = (stopping.quic.clone(), stopping.abort.clone());
            listeners.spawn(http3.serve(app.clone(), stop, abort));
        }
        let https = self.https;
        listeners.spawn(async move { tcp::serve_https(https, &tls, app, &shared).await });

        tracing::info!(
            https = %self.https_addr,
            http = self.http_addr.map(tracing::field::display),
            http3 = advertise,
            "serving"
        );
        let result = tokio::select! {
            () = shutdown => Ok(()),
            Some(ended) = listeners.join_next() => ended.unwrap_or_else(|panic| {
                std::panic::resume_unwind(panic.into_panic())
            }),
        };

        tracing::info!(?grace_period, "shutting down");
        stopping.begin();
        let drained = tokio::select! {
            () = async { while listeners.join_next().await.is_some() {} } => true,
            () = tokio::time::sleep(grace_period) => false,
            () = force => false,
        };
        if !drained {
            let open = limits.connections.saturating_sub(slots.available_permits());
            tracing::warn!(open, "closing connections that are still open");
        }
        stopping.abort();
        while listeners.join_next().await.is_some() {}
        result
    }
}

/// Tells every listener to stop, and closes what is left when the server is dropped.
#[derive(Default)]
struct Stopping {
    tcp: Control,
    quic: CancellationToken,
    abort: CancellationToken,
}

impl Stopping {
    fn begin(&self) {
        // The grace period is timed once, for all listeners.
        self.tcp.graceful_shutdown(None);
        self.quic.cancel();
    }

    fn abort(&self) {
        self.tcp.shutdown();
        self.abort.cancel();
    }
}

impl Drop for Stopping {
    fn drop(&mut self) {
        self.begin();
        self.abort();
    }
}

/// Tell HTTPS clients that the same origin answers over HTTP/3 on this port.
fn advertise_http3(app: axum::Router, port: u16) -> axum::Router {
    let value = format!("h3=\":{port}\"; ma={}", ALT_SVC_MAX_AGE.as_secs());
    let value = HeaderValue::try_from(value).expect("a port and a number are a valid value");
    app.layer(MapResponseLayer::new(
        move |mut response: axum::response::Response| {
            response
                .headers_mut()
                .entry(ALT_SVC)
                .or_insert_with(|| value.clone());
            response
        },
    ))
}
