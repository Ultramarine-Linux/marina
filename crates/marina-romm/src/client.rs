use crate::error::Error;
use crate::models::{Heartbeat, Platform, PlatformQuery, Rom, RomPage, RomQuery};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{Stream, StreamExt, stream};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Serialize, de::DeserializeOwned};
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

#[derive(Debug, Serialize)]
struct RomActivityUpdate {}

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

    /// Stamps RomM's `last_played` value for the authenticated user.
    pub async fn record_play_activity(
        &self,
        rom_id: i32,
    ) -> Result<crate::models::metadata::RomUser, Error> {
        let request = self.rom_activity_request(rom_id)?;
        let response = self.http.execute(request).await?;
        self.decode_response(response).await
    }

    fn rom_activity_request(&self, rom_id: i32) -> Result<reqwest::Request, Error> {
        Ok(self
            .http
            .put(format!("{}/api/roms/{rom_id}/props", self.base_url))
            .headers(self.auth_headers()?)
            .query(&[("update_last_played", true)])
            .json(&RomActivityUpdate {})
            .build()?)
    }

    pub async fn list_saves(
        &self,
        rom_id: i32,
        slot: &str,
    ) -> Result<Vec<crate::models::metadata::Save>, Error> {
        let response = self
            .http
            .get(format!("{}/api/saves", self.base_url))
            .headers(self.auth_headers()?)
            .query(&[("rom_id", rom_id.to_string()), ("slot", slot.to_owned())])
            .send()
            .await?;
        self.decode_response(response).await
    }

    pub async fn download_save(
        &self,
        save_id: i32,
        destination: impl AsRef<Path>,
    ) -> Result<(), Error> {
        let response = self
            .http
            .get(format!("{}/api/saves/{save_id}/content", self.base_url))
            .headers(self.auth_headers()?)
            .send()
            .await?;
        self.write_download(response, destination.as_ref()).await
    }

    /// Uploads one immutable save snapshot for a ROM.
    ///
    /// RomM's `overwrite` and `autocleanup` flags are explicitly disabled so
    /// older snapshots in the same slot remain available.
    pub async fn upload_save_snapshot(
        &self,
        rom_id: i32,
        path: impl AsRef<Path>,
        snapshot_name: &str,
        emulator: &str,
        slot: &str,
    ) -> Result<crate::models::metadata::Save, Error> {
        let part = reqwest::multipart::Part::file(path)
            .await?
            .file_name(snapshot_name.to_owned());
        let request = self.save_upload_request(rom_id, part, emulator, slot)?;
        let response = self.http.execute(request).await?;
        self.decode_response(response).await
    }

    fn save_upload_request(
        &self,
        rom_id: i32,
        part: reqwest::multipart::Part,
        emulator: &str,
        slot: &str,
    ) -> Result<reqwest::Request, Error> {
        Ok(self
            .http
            .post(format!("{}/api/saves", self.base_url))
            .headers(self.auth_headers()?)
            .query(&[
                ("rom_id", rom_id.to_string()),
                ("emulator", emulator.to_owned()),
                ("slot", slot.to_owned()),
                ("overwrite", false.to_string()),
                ("autocleanup", false.to_string()),
            ])
            .multipart(reqwest::multipart::Form::new().part("saveFile", part))
            .build()?)
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

    async fn write_download(
        &self,
        response: reqwest::Response,
        destination: &Path,
    ) -> Result<(), Error> {
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Http {
                status: status.as_u16(),
                body: response.text().await?,
            });
        }
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let temporary = destination.with_extension(format!(
            "{}.part",
            destination
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
        ));
        let mut output = tokio::fs::File::create(&temporary).await?;
        let mut response = response;
        while let Some(chunk) = response.chunk().await? {
            output.write_all(&chunk).await?;
        }
        output.flush().await?;
        drop(output);
        tokio::fs::rename(temporary, destination).await?;
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
    use reqwest::{
        Method,
        header::{AUTHORIZATION, CONTENT_TYPE},
        multipart::Part,
    };

    use super::{Auth, Client};

    #[test]
    fn encodes_basic_auth_bytes() {
        assert_eq!(STANDARD.encode(b"user:password"), "dXNlcjpwYXNzd29yZA==");
    }

    #[test]
    fn builds_rom_activity_request() {
        let client = Client::new("https://romm.example.com/")
            .with_auth(Auth::Bearer("activity-token".into()));
        let request = client.rom_activity_request(42).expect("activity request");

        assert_eq!(request.method(), Method::PUT);
        assert_eq!(
            request.url().as_str(),
            "https://romm.example.com/api/roms/42/props?update_last_played=true"
        );
        assert_eq!(
            request.headers().get(AUTHORIZATION).unwrap(),
            "Bearer activity-token"
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                request.body().and_then(reqwest::Body::as_bytes).unwrap()
            )
            .unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn builds_append_only_autosave_request() {
        let client =
            Client::new("https://romm.example.com").with_auth(Auth::Bearer("save-token".into()));
        let request = client
            .save_upload_request(
                42,
                Part::bytes(b"save data".to_vec()).file_name("snapshot.srm"),
                "marina",
                "autosave",
            )
            .expect("save upload request");
        let query = request
            .url()
            .query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.url().path(), "/api/saves");
        assert_eq!(query.get("rom_id").map(String::as_str), Some("42"));
        assert_eq!(query.get("emulator").map(String::as_str), Some("marina"));
        assert_eq!(query.get("slot").map(String::as_str), Some("autosave"));
        assert_eq!(query.get("overwrite").map(String::as_str), Some("false"));
        assert_eq!(query.get("autocleanup").map(String::as_str), Some("false"));
        assert!(
            request.headers()[CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("multipart/form-data; boundary=")
        );
    }
}
