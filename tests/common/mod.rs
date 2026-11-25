//! A server with a private certificate authority, and real clients for each protocol.
#![allow(
    dead_code,
    reason = "each test binary compiles this module and uses a part of it"
)]

use axum::body::{Body, Bytes};
use http::{Request, Response, Version};
use http_body_util::{BodyExt, channel::Channel};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use rustls::{
    ClientConfig, RootCertStore,
    crypto::aws_lc_rs,
    pki_types::{CertificateDer, ServerName},
};
use std::{
    future::Future,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::{TcpStream, UdpSocket},
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::timeout,
};
use tokio_rustls::{TlsConnector, client::TlsStream};
use warded_server::{BoundServer, Error, Server, Tls};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub const VERSIONS: [Version; 3] = [Version::HTTP_11, Version::HTTP_2, Version::HTTP_3];

/// Fail a test that hangs instead of blocking the run.
pub async fn soon<F: Future>(future: F) -> F::Output {
    timeout(Duration::from_secs(20), future)
        .await
        .expect("the operation finishes in time")
}

/// A certificate authority that the clients trust, and the files the server reads.
pub struct Authority {
    issuer: CertifiedIssuer<'static, KeyPair>,
    directory: tempfile::TempDir,
}

impl Authority {
    pub fn new() -> Self {
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let key = KeyPair::generate().unwrap();
        let authority = Self {
            issuer: CertifiedIssuer::self_signed(params, key).unwrap(),
            directory: tempfile::tempdir().unwrap(),
        };
        authority.issue(1);
        authority
    }

    pub fn chain(&self) -> PathBuf {
        self.directory.path().join("chain.pem")
    }

    pub fn key(&self) -> PathBuf {
        self.directory.path().join("key.pem")
    }

    /// Replace the server's files with a certificate that has this serial number.
    pub fn issue(&self, serial: u64) {
        let mut params = CertificateParams::new(["localhost".to_owned()]).unwrap();
        params.serial_number = Some(serial.into());
        let key = KeyPair::generate().unwrap();
        let certificate = params.signed_by(&key, &self.issuer).unwrap();
        std::fs::write(self.chain(), certificate.pem()).unwrap();
        std::fs::write(self.key(), key.serialize_pem()).unwrap();
    }

    pub fn tls(&self) -> Tls {
        Tls::pem_files(self.chain(), self.key()).unwrap()
    }

    pub fn client_config(&self, protocol: &[u8]) -> ClientConfig {
        let mut roots = RootCertStore::empty();
        roots.add(self.issuer.der().clone()).unwrap();
        let provider = Arc::new(aws_lc_rs::default_provider());
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![protocol.to_vec()];
        config
    }
}

/// A server with free ports on loopback.
pub fn server(authority: &Authority) -> Server {
    Server::new(authority.tls()).https((Ipv4Addr::LOCALHOST, 0).into())
}

/// A served application. Dropping it aborts the server.
pub struct Running {
    pub https: SocketAddr,
    pub http: Option<SocketAddr>,
    pub task: JoinHandle<Result<(), Error>>,
    stop: Option<oneshot::Sender<()>>,
}

impl Running {
    pub fn start(server: Server, app: axum::Router) -> Self {
        Self::start_bound(server.bind().unwrap(), app)
    }

    pub fn start_bound(bound: BoundServer, app: axum::Router) -> Self {
        let (stop, stopped) = oneshot::channel();
        Self {
            https: bound.https_addr(),
            http: bound.http_addr(),
            stop: Some(stop),
            task: tokio::spawn(bound.serve_until(app, async {
                let _ = stopped.await;
            })),
        }
    }

    /// Begin shutdown.
    pub fn stop(&mut self) {
        let _ = self.stop.take().expect("stopped once").send(());
    }

    /// Wait for the server to end.
    pub async fn ended(mut self) -> Result<(), Error> {
        soon(&mut self.task).await.unwrap()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A connection of one protocol version.
pub enum Client {
    Http1(hyper::client::conn::http1::SendRequest<Body>),
    Http2(hyper::client::conn::http2::SendRequest<Body>),
    Http3(Http3Client),
}

pub struct Http3Client {
    pub quic: quinn::Connection,
    pub send: h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
    pub driver: JoinHandle<h3::error::ConnectionError>,
    endpoint: quinn::Endpoint,
}

impl Client {
    pub async fn connect(
        version: Version,
        address: SocketAddr,
        authority: &Authority,
    ) -> Result<Self, BoxError> {
        soon(async {
            Ok(match version {
                Version::HTTP_11 => {
                    let stream = tls(address, authority, b"http/1.1").await?;
                    let (send, connection) =
                        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
                    tokio::spawn(connection.with_upgrades());
                    Self::Http1(send)
                }
                Version::HTTP_2 => {
                    let stream = tls(address, authority, b"h2").await?;
                    let (send, connection) = hyper::client::conn::http2::handshake(
                        TokioExecutor::new(),
                        TokioIo::new(stream),
                    )
                    .await?;
                    tokio::spawn(connection);
                    Self::Http2(send)
                }
                Version::HTTP_3 => Self::Http3(http3(address, authority).await?),
                other => panic!("no client for {other:?}"),
            })
        })
        .await
    }

    /// Send a request and wait for the response head. Bodies are streamed both ways.
    pub async fn send(&mut self, request: Request<Body>) -> Result<Response<Body>, BoxError> {
        soon(async {
            match self {
                Self::Http1(send) => {
                    // HTTP/1.1 puts the host in a field and only the path in its target.
                    let mut request = request;
                    let host = request.uri().authority().unwrap().as_str().try_into()?;
                    request.headers_mut().insert("host", host);
                    *request.uri_mut() = request.uri().path_and_query().unwrap().clone().into();
                    send.ready().await?;
                    Ok(send.send_request(request).await?.map(Body::new))
                }
                Self::Http2(send) => {
                    send.ready().await?;
                    let mut request = request;
                    *request.version_mut() = Version::HTTP_2;
                    Ok(send.send_request(request).await?.map(Body::new))
                }
                Self::Http3(client) => send_http3(&mut client.send, request).await,
            }
        })
        .await
    }

    pub async fn get(&mut self, path: &str) -> Result<Response<Body>, BoxError> {
        self.send(get(path)).await
    }
}

/// Send a request on an HTTP/3 connection and wait for the response head.
pub async fn send_http3(
    connection: &mut h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
    request: Request<Body>,
) -> Result<Response<Body>, BoxError> {
    let (parts, mut body) = request.into_parts();
    let stream = connection
        .send_request(Request::from_parts(parts, ()))
        .await?;
    let (mut sender, mut receiver) = stream.split();
    tokio::spawn(async move {
        while let Some(frame) = body.frame().await {
            match frame.unwrap().into_data() {
                Ok(data) => sender.send_data(data).await?,
                Err(frame) => sender.send_trailers(frame.into_trailers().unwrap()).await?,
            }
        }
        sender.finish().await
    });
    let response = receiver.recv_response().await?;
    let (mut send, body) = Channel::<Bytes, BoxError>::new(1);
    tokio::spawn(async move {
        loop {
            match receiver.recv_data().await {
                Ok(Some(mut data)) => {
                    use bytes::Buf;
                    let data = data.copy_to_bytes(data.remaining());
                    if send.send_data(data).await.is_err() {
                        return;
                    }
                }
                Ok(None) => break,
                Err(error) => return send.abort(error.into()),
            }
        }
        match receiver.recv_trailers().await {
            Ok(Some(trailers)) => {
                let _ = send.send_trailers(trailers).await;
            }
            Ok(None) => {}
            Err(error) => send.abort(error.into()),
        }
    });
    Ok(response.map(|()| Body::new(body)))
}

impl Http3Client {
    /// Close the connection the way a client that received everything does.
    pub async fn close(self) {
        self.quic.close(0x100_u32.into(), b"done");
        self.endpoint.wait_idle().await;
    }
}

pub async fn tls(
    address: SocketAddr,
    authority: &Authority,
    protocol: &[u8],
) -> Result<TlsStream<TcpStream>, BoxError> {
    let connector = TlsConnector::from(Arc::new(authority.client_config(protocol)));
    let stream = TcpStream::connect(address).await?;
    let stream = connector
        .connect(ServerName::try_from("localhost")?, stream)
        .await?;
    assert_eq!(stream.get_ref().1.alpn_protocol(), Some(protocol));
    Ok(stream)
}

pub async fn quic(
    address: SocketAddr,
    authority: &Authority,
) -> Result<(quinn::Endpoint, quinn::Connection), BoxError> {
    let config = authority.client_config(b"h3");
    let config = quinn::crypto::rustls::QuicClientConfig::try_from(config)?;
    let endpoint = quinn::Endpoint::client((Ipv4Addr::LOCALHOST, 0).into())?;
    let config = quinn::ClientConfig::new(Arc::new(config));
    let connection = endpoint.connect_with(config, address, "localhost")?.await?;
    Ok((endpoint, connection))
}

pub async fn http3(address: SocketAddr, authority: &Authority) -> Result<Http3Client, BoxError> {
    let (endpoint, quic) = quic(address, authority).await?;
    let (mut connection, send) = h3::client::new(h3_quinn::Connection::new(quic.clone())).await?;
    let driver =
        tokio::spawn(async move { std::future::poll_fn(|cx| connection.poll_close(cx)).await });
    Ok(Http3Client {
        quic,
        send,
        driver,
        endpoint,
    })
}

/// An address that leads to this one, and the first byte of the first packet that comes back.
pub async fn first_reply(server: SocketAddr) -> (SocketAddr, oneshot::Receiver<u8>) {
    let outside = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let inside = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    inside.connect(server).await.unwrap();
    let address = outside.local_addr().unwrap();
    let (first, reply) = oneshot::channel();
    tokio::spawn(async move {
        let (mut sent, mut received) = ([0; 65_536], [0; 65_536]);
        let (mut first, mut client) = (Some(first), None);
        loop {
            tokio::select! {
                Ok((length, from)) = outside.recv_from(&mut sent) => {
                    client = Some(from);
                    let _ = inside.send(&sent[..length]).await;
                }
                Ok(length) = inside.recv(&mut received) => {
                    if let Some(first) = first.take() {
                        let _ = first.send(received[0]);
                    }
                    let _ = outside.send_to(&received[..length], client.unwrap()).await;
                }
                else => return,
            }
        }
    });
    (address, reply)
}

/// An address that leads to this one over a link with limited bandwidth back to the client.
pub async fn slow_link(server: SocketAddr, bytes_per_second: usize) -> SocketAddr {
    let outside = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap());
    let inside = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap());
    inside.connect(server).await.unwrap();
    let address = outside.local_addr().unwrap();
    let (queue, mut queued) = mpsc::unbounded_channel::<Vec<u8>>();
    let (met, mut client) = watch::channel(None);
    tokio::spawn({
        let (outside, inside) = (outside.clone(), inside.clone());
        async move {
            let mut packet = [0; 65_536];
            while let Ok((length, client)) = outside.recv_from(&mut packet).await {
                met.send_replace(Some(client));
                let _ = inside.send(&packet[..length]).await;
            }
        }
    });
    tokio::spawn(async move {
        let mut packet = [0; 65_536];
        while let Ok(length) = inside.recv(&mut packet).await {
            let _ = queue.send(packet[..length].to_vec());
        }
    });
    tokio::spawn(async move {
        while let Some(packet) = queued.recv().await {
            let pause = Duration::from_secs_f64(packet.len() as f64 / bytes_per_second as f64);
            tokio::time::sleep(pause).await;
            let client = client.wait_for(Option::is_some).await.unwrap().unwrap();
            let _ = outside.send_to(&packet, client).await;
        }
    });
    address
}

/// `GET` or `POST` `https://localhost/…`, with any more encoded fields, as one HEADERS frame.
pub fn headers_frame(method: &str, path: &str, more: &[u8]) -> Vec<u8> {
    // The QPACK prefix for no dynamic table, then :scheme https from the static table.
    let mut block = vec![0, 0, 0xd7];
    block.push(match method {
        "GET" => 0xd1,
        "POST" => 0xd4,
        _ => unimplemented!(),
    });
    // :authority and :path with their names from the static table and literal values.
    for (name, value) in [(0x50, "localhost"), (0x51, path)] {
        block.push(name);
        block.push(value.len().try_into().unwrap());
        block.extend_from_slice(value.as_bytes());
    }
    block.extend_from_slice(more);
    frame(1, block.len() as u64, &block)
}

/// An HTTP/3 frame that declares this length, whatever its payload holds.
pub fn frame(kind: u8, length: u64, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![kind];
    if length < 64 {
        frame.push(length as u8);
    } else {
        frame.extend_from_slice(&(length as u32 | 0x8000_0000).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    frame
}

pub const DATA: u8 = 0;

/// The server's certificate on a new connection.
pub async fn certificate(
    version: Version,
    address: SocketAddr,
    authority: &Authority,
) -> CertificateDer<'static> {
    match version {
        Version::HTTP_3 => {
            let (_endpoint, connection) = soon(quic(address, authority)).await.unwrap();
            let identity = connection.peer_identity().unwrap();
            let chain = identity.downcast::<Vec<CertificateDer<'static>>>().unwrap();
            chain[0].clone()
        }
        _ => {
            let stream = soon(tls(address, authority, b"h2")).await.unwrap();
            stream.get_ref().1.peer_certificates().unwrap()[0]
                .clone()
                .into_owned()
        }
    }
}

pub fn get(path: &str) -> Request<Body> {
    request("GET", path, Body::empty())
}

pub fn request(method: &str, path: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(format!("https://localhost{path}"))
        .body(body)
        .unwrap()
}

/// A body that the test writes while the request or response is in progress.
pub fn channel() -> (http_body_util::channel::Sender<Bytes, BoxError>, Body) {
    let (sender, body) = Channel::new(1);
    (sender, Body::new(body))
}

pub async fn text(response: Response<Body>) -> Result<String, BoxError> {
    let body = soon(response.into_body().collect()).await?.to_bytes();
    Ok(String::from_utf8(body.to_vec())?)
}

/// The next piece of a streamed body.
pub async fn chunk(body: &mut Body) -> Result<Bytes, BoxError> {
    let frame = soon(body.frame()).await.ok_or("the body ended")??;
    frame.into_data().map_err(|_| "not data".into())
}

/// Plain HTTP/1.1 to an address, returning everything until the server closes.
pub async fn cleartext(address: SocketAddr, request: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    soon(stream.read_to_string(&mut response)).await.unwrap();
    response
}
