use crate::{Error, Server, Tls, server::is_loopback};
use std::{net::SocketAddr, str::FromStr};

const HTTPS_ADDR: &str = "WARDED_HTTPS_ADDR";
const HTTP_ADDR: &str = "WARDED_HTTP_ADDR";
const DEV_ADDR: &str = "WARDED_DEV_ADDR";
const TLS_CERT: &str = "WARDED_TLS_CERT";
const TLS_KEY: &str = "WARDED_TLS_KEY";
const HTTP3: &str = "WARDED_HTTP3";
const PUBLIC_PORT: &str = "WARDED_PUBLIC_PORT";

impl Server {
    /// Configure a server from environment variables.
    ///
    /// | Variable             | Meaning                                     | Default          |
    /// | -------------------- | ------------------------------------------- | ---------------- |
    /// | `WARDED_HTTPS_ADDR`  | HTTPS address, over TCP and UDP             | `127.0.0.1:8443` |
    /// | `WARDED_HTTP_ADDR`   | Cleartext address that redirects to HTTPS   | none             |
    /// | `WARDED_DEV_ADDR`    | [Development address], on loopback          | none             |
    /// | `WARDED_TLS_CERT`    | PEM certificate chain file                  | see below        |
    /// | `WARDED_TLS_KEY`     | PEM private key file                        | see below        |
    /// | `WARDED_HTTP3`       | `on` or `off`                               | `on`             |
    /// | `WARDED_PUBLIC_PORT` | HTTPS port that clients use, if it differs  | the bound port   |
    ///
    /// Without a certificate and key, and only while HTTPS listens on a loopback address, the
    /// server generates a certificate for `localhost` that no client trusts. Listening anywhere
    /// else needs a real one. An empty variable counts as unset.
    ///
    /// The templates set `WARDED_DEV_ADDR` in `.cargo/config.toml`, which `cargo run` reads and a
    /// program started without Cargo does not. A variable that is already set takes precedence.
    ///
    /// [Development address]: Server::development
    pub fn from_env() -> Result<Self, Error> {
        Self::from_variables(|name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(invalid(name, "is not valid Unicode")),
        })
    }

    fn from_variables(
        read: impl Fn(&'static str) -> Result<Option<String>, Error>,
    ) -> Result<Self, Error> {
        let get = |name| Ok(read(name)?.filter(|value| !value.is_empty()));
        let https: Option<SocketAddr> = parse(HTTPS_ADDR, get(HTTPS_ADDR)?, ADDRESS)?;
        let local = https.is_none_or(|address| address.ip().to_canonical().is_loopback());
        let tls = match (get(TLS_CERT)?, get(TLS_KEY)?) {
            (Some(chain), Some(key)) => Tls::pem_files(chain, key)?,
            (Some(_), None) => return Err(invalid(TLS_KEY, "is needed with WARDED_TLS_CERT")),
            (None, Some(_)) => return Err(invalid(TLS_CERT, "is needed with WARDED_TLS_KEY")),
            (None, None) if local => development()?,
            (None, None) => {
                let reason = "is needed, with WARDED_TLS_KEY, to listen beyond this computer";
                return Err(invalid(TLS_CERT, reason));
            }
        };
        let mut server = Self::new(tls);
        if let Some(address) = https {
            server = server.https(address);
        }
        if let Some(address) = parse(HTTP_ADDR, get(HTTP_ADDR)?, ADDRESS)? {
            server = server.redirect_http(address);
        }
        if let Some(address) = parse(DEV_ADDR, get(DEV_ADDR)?, LOOPBACK)? {
            if !is_loopback(address) {
                let reason =
                    "must be a loopback address, such as 127.0.0.1:3000, because it has no TLS";
                return Err(invalid(DEV_ADDR, reason));
            }
            server = server.development(address);
        }
        if let Some(port) = parse(PUBLIC_PORT, get(PUBLIC_PORT)?, "a port number")? {
            server = server.public_port(port);
        }
        match get(HTTP3)?.as_deref() {
            None | Some("on") => Ok(server),
            Some("off") => Ok(server.http3(false)),
            Some(_) => Err(invalid(HTTP3, "is neither `on` nor `off`")),
        }
    }
}

const ADDRESS: &str = "an address and port, such as [::]:443";
const LOOPBACK: &str = "an address and port, such as 127.0.0.1:3000";

fn parse<T: FromStr>(
    variable: &'static str,
    value: Option<String>,
    expected: &str,
) -> Result<Option<T>, Error> {
    value
        .map(|value| {
            value
                .parse()
                .map_err(|_| invalid(variable, &format!("is not {expected}")))
        })
        .transpose()
}

fn invalid(variable: &'static str, reason: &str) -> Error {
    Error::Environment {
        variable,
        reason: reason.to_owned(),
    }
}

#[cfg(feature = "self-signed")]
fn development() -> Result<Tls, Error> {
    tracing::warn!("no certificate is configured; using one that clients do not trust");
    Tls::self_signed(["localhost", "127.0.0.1", "::1"])
}

#[cfg(not(feature = "self-signed"))]
fn development() -> Result<Tls, Error> {
    let reason = "is needed, with WARDED_TLS_KEY, without the `self-signed` feature";
    Err(invalid(TLS_CERT, reason))
}

#[cfg(all(test, feature = "self-signed"))]
mod tests {
    use super::*;

    fn configure(variables: &[(&'static str, &str)]) -> Result<Server, String> {
        Server::from_variables(|name| {
            let value = variables.iter().find(|(variable, _)| *variable == name);
            Ok(value.map(|(_, value)| (*value).to_owned()))
        })
        .map_err(|error| error.to_string())
    }

    #[test]
    fn development_needs_nothing() {
        assert!(configure(&[]).is_ok());
        assert!(configure(&[(HTTPS_ADDR, ""), (HTTP3, ""), (DEV_ADDR, "")]).is_ok());
        assert!(configure(&[(HTTPS_ADDR, "[::1]:9443"), (HTTP3, "off")]).is_ok());
    }

    #[test]
    fn the_development_address_stays_on_this_computer() {
        for address in ["127.0.0.1:3000", "[::1]:3000", "[::ffff:127.0.0.1]:3000"] {
            assert!(configure(&[(DEV_ADDR, address)]).is_ok(), "{address}");
        }
        for address in ["0.0.0.0:3000", "[::]:3000", "192.0.2.1:3000"] {
            let error = configure(&[(DEV_ADDR, address)]).unwrap_err();
            assert_eq!(
                error,
                "WARDED_DEV_ADDR must be a loopback address, such as 127.0.0.1:3000, because it \
                 has no TLS"
            );
        }
    }

    #[test]
    fn a_generated_certificate_stays_on_this_computer() {
        for address in ["0.0.0.0:443", "[::]:443", "192.0.2.1:443"] {
            let error = configure(&[(HTTPS_ADDR, address)]).unwrap_err();
            assert!(error.starts_with(TLS_CERT), "{error}");
        }
        // The check sees through an IPv4-mapped address.
        assert!(configure(&[(HTTPS_ADDR, "[::ffff:127.0.0.1]:8443")]).is_ok());
    }

    #[test]
    fn invalid_values_name_their_variable() {
        for (variables, expected) in [
            (&[(HTTPS_ADDR, "localhost")][..], HTTPS_ADDR),
            (&[(HTTP_ADDR, "80")], HTTP_ADDR),
            (&[(DEV_ADDR, "localhost:3000")], DEV_ADDR),
            (&[(HTTP3, "yes")], HTTP3),
            (&[(PUBLIC_PORT, "65536")], PUBLIC_PORT),
            (&[(TLS_CERT, "chain.pem")], TLS_KEY),
            (&[(TLS_KEY, "key.pem")], TLS_CERT),
        ] {
            let error = configure(variables).unwrap_err();
            assert!(error.starts_with(expected), "{error}");
        }
    }
}
