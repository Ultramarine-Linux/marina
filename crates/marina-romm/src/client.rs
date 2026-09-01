use crate::error::Error;
use crate::models::{Heartbeat, Platform, PlatformQuery, Rom, RomPage, RomQuery};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{Stream, StreamExt, stream};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::de::DeserializeOwned;
use std::path::Path;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info};

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

    /// Resolves RomM resource paths using RomM's asset namespace.
    /// Relative resource paths are served below `/assets/romm/resources`.
    pub fn resource_url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            return path.to_owned();
        }
        let path = path.trim_start_matches('/');
        let path = if path.starts_with("assets/romm/resources/") {
            path.to_owned()
        } else {
            format!("assets/romm/resources/{path}")
        };
        format!("{}/{path}", self.base_url)
    }

    pub async fn heartbeat(&self) -> Result<Heartbeat, Error> {
        self.get("/api/heartbeat").await
    }

    /// Hydrate one ROM, including its concrete file entries.
    pub async fn get_rom(&self, rom_id: i32) -> Result<Rom, Error> {
        self.get(&format!("/api/roms/{rom_id}?with_files=true"))
            .await
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

    /// Streams ROM pages matching `query`, following RomM's limit/offset pages.
    pub fn paginate_roms(
        &self,
        query: RomQuery,
    ) -> impl Stream<Item = Result<RomPage, Error>> + '_ {
        let offset = query.offset.unwrap_or_default();
        stream::try_unfold(
            (self, query, offset, false),
            |(client, mut query, offset, finished)| async move {
                if finished {
                    return Ok(None);
                }

                query.offset = Some(offset);
                let page = client.list_roms(&query).await?;
                let page_len = page.items.len() as i64;
                if page_len == 0 {
                    return Ok(None);
                }

                let next_offset = offset + page_len;
                let finished = page.total.is_some_and(|total| next_offset >= total);
                Ok(Some((page, (client, query, next_offset, finished))))
            },
        )
    }

    /// Lists every ROM matching `query` by collecting [`paginate_roms`].
    pub async fn list_all_roms(&self, query: &RomQuery) -> Result<Vec<Rom>, Error> {
        let mut roms = Vec::new();
        let pages = self.paginate_roms(query.clone());
        futures_util::pin_mut!(pages);
        while let Some(page) = pages.next().await {
            roms.extend(page?.items);
        }
        Ok(roms)
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

    /// Streams one ROM file to disk without buffering it in memory.
    pub async fn download_file(
        &self,
        rom_id: i32,
        file_name: &str,
        file_id: Option<i32>,
        destination: impl AsRef<Path>,
    ) -> Result<(), Error> {
        info!(rom_id, file_name, ?file_id, "starting RomM file download");
        let mut url = format!(
            "{}/api/roms/{}/content/{}",
            self.base_url,
            rom_id,
            urlencoding::encode(file_name)
        );
        if let Some(file_id) = file_id {
            url.push_str(&format!("?file_ids={file_id}"));
        }
        let endpoint = url.strip_prefix(&self.base_url).unwrap_or(&url).to_owned();
        debug!(
            rom_id,
            file_name,
            ?file_id,
            endpoint,
            authenticated = self.auth.is_some(),
            "starting RomM file request"
        );
        let response = self
            .http
            .get(url)
            .headers(self.auth_headers()?)
            .send()
            .await?;
        let status = response.status();
        debug!(
            rom_id,
            file_name,
            ?file_id,
            endpoint,
            %status,
            authenticated = self.auth.is_some(),
            "RomM file response received"
        );
        if !status.is_success() {
            return Err(Error::Http {
                status: status.as_u16(),
                body: response.text().await?,
            });
        }
        let destination = destination.as_ref();
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut output = tokio::fs::File::create(destination).await?;
        let mut response = response;
        while let Some(chunk) = response.chunk().await? {
            output.write_all(&chunk).await?;
        }
        output.flush().await?;
        info!(rom_id, file_name, "RomM file download completed");
        Ok(())
    }

    /// Streams an authenticated RomM asset URL to disk.
    pub async fn download_url(
        &self,
        url: &str,
        destination: impl AsRef<Path>,
    ) -> Result<(), Error> {
        let endpoint = url.strip_prefix(&self.base_url).unwrap_or(url);
        debug!(
            endpoint,
            authenticated = self.auth.is_some(),
            "starting RomM resource download"
        );
        let response = self
            .http
            .get(url)
            .headers(self.auth_headers()?)
            .send()
            .await?;
        let status = response.status();
        debug!(endpoint, %status, authenticated = self.auth.is_some(), "RomM resource response received");
        if !status.is_success() {
            return Err(Error::Http {
                status: status.as_u16(),
                body: response.text().await?,
            });
        }
        let destination = destination.as_ref();
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut output = tokio::fs::File::create(destination).await?;
        let mut response = response;
        while let Some(chunk) = response.chunk().await? {
            output.write_all(&chunk).await?;
        }
        output.flush().await?;
        Ok(())
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
        let endpoint = url.strip_prefix(&self.base_url).unwrap_or(&url).to_owned();
        debug!(endpoint = %endpoint, authenticated = self.auth.is_some(), "RomM request");
        let response = self
            .http
            .get(&url)
            .headers(self.auth_headers()?)
            .send()
            .await?;

        debug!(endpoint = %endpoint, status = %response.status(), "RomM response");
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
