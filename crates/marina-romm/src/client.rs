use crate::error::Error;
use crate::models::{Heartbeat, Platform, PlatformQuery, Rom, RomPage, RomQuery};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::de::DeserializeOwned;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Auth {
    Bearer(String),
    Basic { username: String, password: String },
}

#[derive(Clone, Debug)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    auth: Option<Auth>,
}

impl Client {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            auth: None,
        }
    }

    pub fn with_auth(mut self, auth: Auth) -> Self {
        self.auth = Some(auth);
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn cover_url(&self, rom: &Rom) -> Option<String> {
        let path = rom.cover_path()?;
        if path.starts_with("http://") || path.starts_with("https://") {
            return Some(path.to_owned());
        }

        let path = path.trim_start_matches('/');
        let path = if path.starts_with("assets/romm/resources/") {
            path.to_owned()
        } else {
            format!("assets/romm/resources/{path}")
        };

        Some(format!("{}/{path}", self.base_url))
    }

    pub async fn heartbeat(&self) -> Result<Heartbeat, Error> {
        self.get("/api/heartbeat").await
    }

    pub async fn list_roms(&self, query: &RomQuery) -> Result<RomPage, Error> {
        let mut url = format!("{}/api/roms", self.base_url);
        let encoded = serde_urlencoded::to_string(query)?;
        if !encoded.is_empty() {
            url.push('?');
            url.push_str(&encoded);
        }
        for platform_id in &query.platform_ids {
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str("platform_ids=");
            url.push_str(&platform_id.to_string());
        }

        self.get_url(url).await
    }

    pub async fn list_platforms(&self, query: &PlatformQuery) -> Result<Vec<Platform>, Error> {
        let mut url = format!("{}/api/platforms", self.base_url);
        let encoded = serde_urlencoded::to_string(query)?;
        if !encoded.is_empty() {
            url.push('?');
            url.push_str(&encoded);
        }

        self.get_url(url).await
    }

    async fn get<T>(&self, path: &str) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        let url = format!("{}{path}", self.base_url);
        self.get_url(url).await
    }

    async fn get_url<T>(&self, url: String) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        let response = self
            .http
            .get(url)
            .headers(self.auth_headers()?)
            .send()
            .await?;

        self.decode_response(response).await
    }

    async fn decode_response<T>(&self, response: reqwest::Response) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Http {
                status: status.as_u16(),
                body: response.text().await?,
            });
        }

        Ok(response.json().await?)
    }

    fn auth_headers(&self) -> Result<HeaderMap, Error> {
        let mut headers = HeaderMap::new();

        let Some(auth) = &self.auth else {
            return Ok(headers);
        };

        let value = match auth {
            Auth::Bearer(token) => format!("Bearer {token}"),
            Auth::Basic { username, password } => {
                let credentials = format!("{username}:{password}");
                let encoded = STANDARD.encode(credentials.as_bytes());
                format!("Basic {encoded}")
            }
        };
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&value).map_err(|_| Error::InvalidHeader)?,
        );

        Ok(headers)
    }
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    #[test]
    fn encodes_basic_auth_bytes() {
        assert_eq!(STANDARD.encode(b"user:password"), "dXNlcjpwYXNzd29yZA==");
    }
}
