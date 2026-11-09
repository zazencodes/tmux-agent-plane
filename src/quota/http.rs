//! The one HTTP client: rustls, the proxy from HTTPS_PROXY/NO_PROXY, no redirects, 15 s.
//! Requests carry bearer tokens, so nothing here is Debug and no header is ever logged.

use std::io::Read;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(15);

pub struct Request<'a> {
    pub method: Method,
    pub url: &'a str,
    pub headers: Vec<(&'static str, String)>,
    pub body: &'a [u8],
    /// Largest body accepted; a larger one is an error.
    pub limit: u64,
}

#[derive(Clone, Copy)]
pub enum Method {
    Get,
    Post,
}

pub struct Response {
    pub status: u16,
    /// Lowercased names.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Why no response came back.
#[derive(Debug, PartialEq)]
pub enum Error {
    /// Timeout or network failure: transient.
    Transport(String),
    /// The body is larger than the request allows.
    TooLarge,
}

pub trait Http: Sync {
    fn send(&self, request: Request<'_>) -> Result<Response, Error>;
}

pub struct Client(ureq::Agent);

impl Client {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .max_redirects(0)
            .http_status_as_error(false)
            .proxy(ureq::Proxy::try_from_env())
            .build();
        Self(config.into())
    }
}

impl Http for Client {
    fn send(&self, request: Request<'_>) -> Result<Response, Error> {
        let transport = |e: ureq::Error| match e {
            ureq::Error::BodyExceedsLimit(_) => Error::TooLarge,
            ureq::Error::Timeout(_) => Error::Transport("timed out".into()),
            other => Error::Transport(other.to_string()),
        };
        let mut response = match request.method {
            Method::Get => {
                let mut builder = self.0.get(request.url);
                for (name, value) in &request.headers {
                    builder = builder.header(*name, value);
                }
                builder.call()
            }
            Method::Post => {
                let mut builder = self.0.post(request.url);
                for (name, value) in &request.headers {
                    builder = builder.header(*name, value);
                }
                builder.send(request.body)
            }
        }
        .map_err(transport)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(n, v)| {
                (
                    n.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect();
        let mut body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(request.limit + 1)
            .read_to_end(&mut body)
            .map_err(|e| Error::Transport(e.to_string()))?;
        if body.len() as u64 > request.limit {
            return Err(Error::TooLarge);
        }
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::Mutex;

    /// A request as the fake saw it: its URL and headers.
    pub type Asked = (String, Vec<(&'static str, String)>);

    /// Answers each request with the next canned result and records what was asked, so tests
    /// can check which endpoint got which token.
    #[derive(Default)]
    pub struct Fake {
        pub answers: Mutex<Vec<Result<Response, Error>>>,
        pub asked: Mutex<Vec<Asked>>,
    }

    impl Fake {
        pub fn with(answers: Vec<Result<Response, Error>>) -> Self {
            Self {
                answers: Mutex::new(answers),
                asked: Mutex::default(),
            }
        }
    }

    pub fn response(status: u16, body: &[u8]) -> Result<Response, Error> {
        Ok(Response {
            status,
            headers: Vec::new(),
            body: body.to_vec(),
        })
    }

    impl Http for Fake {
        fn send(&self, request: Request<'_>) -> Result<Response, Error> {
            self.asked
                .lock()
                .unwrap()
                .push((request.url.to_string(), request.headers));
            self.answers.lock().unwrap().remove(0)
        }
    }
}
