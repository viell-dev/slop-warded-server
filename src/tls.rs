use crate::Error;
use arc_swap::ArcSwap;
use rustls::{
    ServerConfig,
    crypto::{CryptoProvider, aws_lc_rs},
    pki_types::{
        CertificateDer, PrivateKeyDer,
        pem::{self, PemObject},
    },
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};
use std::{fmt, path::PathBuf, sync::Arc};

/// The certificate and private key that every listener presents.
///
/// Clones share the same certificate, so [`Tls::reload`] on one of them changes what a running
/// server presents to new TLS and QUIC connections. Established connections keep theirs.
#[derive(Clone)]
pub struct Tls {
    current: Arc<Current>,
    files: Option<Arc<Files>>,
}

struct Files {
    chain: PathBuf,
    key: PathBuf,
}

/// Presents whichever certificate was loaded last, to TCP and QUIC handshakes alike.
#[derive(Debug)]
struct Current(ArcSwap<CertifiedKey>);

impl ResolvesServerCert for Current {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.load_full())
    }
}

impl Tls {
    /// Read a PEM certificate chain and private key from files.
    ///
    /// The chain starts with the server's own certificate, followed by intermediates. Certbot
    /// calls it `fullchain.pem`. The files are read again by [`Tls::reload`].
    pub fn pem_files(chain: impl Into<PathBuf>, key: impl Into<PathBuf>) -> Result<Self, Error> {
        let files = Files {
            chain: chain.into(),
            key: key.into(),
        };
        let current = files.load()?;
        Ok(Self::new(current, Some(files)))
    }

    /// Generate a certificate for development that no client trusts.
    ///
    /// It lasts until the process ends. Browsers show a warning and refuse HTTP/3 with such a
    /// certificate.
    #[cfg(feature = "self-signed")]
    pub fn self_signed<N: Into<String>>(names: impl IntoIterator<Item = N>) -> Result<Self, Error> {
        let names: Vec<_> = names.into_iter().map(Into::into).collect();
        let generated = rcgen::generate_simple_self_signed(names).map_err(Error::Generate)?;
        let key = PrivateKeyDer::Pkcs8(generated.signing_key.serialize_der().into());
        let current = certified(vec![generated.cert.into()], key)?;
        Ok(Self::new(current, None))
    }

    /// Read the files given to [`Tls::pem_files`] again.
    ///
    /// On failure the previous certificate stays in use. A generated certificate has no files,
    /// so reloading it changes nothing.
    pub fn reload(&self) -> Result<(), Error> {
        if let Some(files) = &self.files {
            self.current.0.store(Arc::new(files.load()?));
        }
        Ok(())
    }

    /// The certificate that is presented now.
    #[cfg(all(test, feature = "http3", feature = "self-signed"))]
    pub(crate) fn certificate(&self) -> CertificateDer<'static> {
        self.current.0.load().cert[0].clone()
    }

    /// Whether [`Tls::reload`] has files to read.
    #[cfg(unix)]
    pub(crate) fn reloads(&self) -> bool {
        self.files.is_some()
    }

    fn new(current: CertifiedKey, files: Option<Files>) -> Self {
        Self {
            current: Arc::new(Current(ArcSwap::from_pointee(current))),
            files: files.map(Arc::new),
        }
    }

    /// Server configuration for connections that negotiate one of these ALPN protocols.
    pub(crate) fn config(
        &self,
        protocols: &[&[u8]],
        versions: &[&'static rustls::SupportedProtocolVersion],
    ) -> Result<ServerConfig, Error> {
        // Naming the provider avoids depending on a process-wide default, which another
        // library in the application could have installed or left out.
        let mut config = ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(versions)
            .map_err(Error::Tls)?
            .with_no_client_auth()
            .with_cert_resolver(self.current.clone());
        config.alpn_protocols = protocols.iter().map(|name| name.to_vec()).collect();
        Ok(config)
    }
}

impl fmt::Debug for Tls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("Tls");
        if let Some(files) = &self.files {
            debug.field("chain", &files.chain).field("key", &files.key);
        }
        debug.finish_non_exhaustive()
    }
}

impl Files {
    fn load(&self) -> Result<CertifiedKey, Error> {
        let chain = CertificateDer::pem_file_iter(&self.chain)
            .and_then(Iterator::collect::<Result<Vec<_>, _>>)
            .and_then(|chain| match chain.is_empty() {
                true => Err(pem::Error::NoItemsFound),
                false => Ok(chain),
            })
            .map_err(|source| Error::Read {
                path: self.chain.clone(),
                source,
            })?;
        let key = PrivateKeyDer::from_pem_file(&self.key).map_err(|source| Error::Read {
            path: self.key.clone(),
            source,
        })?;
        certified(chain, key)
    }
}

fn certified(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<CertifiedKey, Error> {
    CertifiedKey::from_der(chain, key, &provider()).map_err(Error::Certificate)
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(aws_lc_rs::default_provider())
}
