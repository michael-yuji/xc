// Copyright (c) 2023 Yan Ka, Chiu.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
// 1. Redistributions of source code must retain the above copyright
//    notice, this list of conditions, and the following disclaimer,
//    without modification, immediately at the beginning of the file.
// 2. The name of the author may not be used to endorse or promote products
//    derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
// ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED. IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE FOR
// ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
// DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
// OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
// HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
// LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
// OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
// SUCH DAMAGE.

use crate::digest::{Hasher, OciDigest};
use crate::models::{
    AnyOciConfig, Descriptor, ImageManifest, ImageManifestList, ManifestDesc, ManifestVariant,
    Platform, DOCKER_MANIFEST, DOCKER_MANIFESTS, OCI_ARTIFACT, OCI_IMAGE_INDEX, OCI_MANIFEST,
};
use futures::future::Either;
use reqwest::{Client, ClientBuilder, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};
use thiserror::Error;
use tokio::sync::watch::Sender;
use tracing::{debug, info};

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("invalid request: {0:?}")]
    InvalidRequest(RequestBuilder),
    #[error("failed response {0:?}")]
    UnsuccessfulResponse(Response),
    #[error("unexpected schema: {0:?}")]
    DecodingFailure(serde_json::Error),
    #[error("cannot send request: {0:?}")]
    ReqwestError(reqwest::Error),
    #[error("cannot convert header value to string: {0:?}")]
    NonStringHttpHeader(reqwest::header::ToStrError),
    #[error("unknown/unsupported content-type: {0}")]
    UnsupportedContentType(String),
    #[error("response missing required header: {0}")]
    MissingHeader(String),
    #[error("ioError: {0}")]
    IoError(std::io::Error),
    #[error("Missing Bearer Token")]
    MissingBearerToken,
    #[error("digest mismatch, expected: {0}, got: {1}")]
    DigestMismatched(OciDigest, OciDigest),
}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> ClientError {
        ClientError::IoError(e)
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(e: serde_json::Error) -> ClientError {
        ClientError::DecodingFailure(e)
    }
}

impl From<reqwest::Error> for ClientError {
    fn from(e: reqwest::Error) -> ClientError {
        ClientError::ReqwestError(e)
    }
}

impl From<reqwest::header::ToStrError> for ClientError {
    fn from(e: reqwest::header::ToStrError) -> ClientError {
        ClientError::NonStringHttpHeader(e)
    }
}

#[derive(Default, Debug)]
pub struct UploadStat {
    pub uploaded: Option<usize>,
    pub started_at: Option<SystemTime>,
    pub completed_at: Option<Either<Duration, Duration>>,
}

pub enum UploadProgress {
    Upload(usize, std::time::Duration),
    Done(std::time::Duration),
    Pending,
    Started,
    Finalize,
    Failed(std::time::Duration),
}

impl UploadStat {
    pub fn is_finished(&self) -> bool {
        self.completed_at.is_some()
    }

    pub fn progress(&self) -> UploadProgress {
        match self.started_at {
            None => UploadProgress::Pending,
            Some(started_at) => {
                if self.uploaded.is_none() {
                    UploadProgress::Started
                } else {
                    match &self.completed_at {
                        Some(either) => match either {
                            Either::Left(duration) => UploadProgress::Done(duration.to_owned()),
                            Either::Right(duration) => UploadProgress::Failed(duration.to_owned()),
                        },
                        None => {
                            if self.uploaded.is_some() {
                                UploadProgress::Finalize
                            } else {
                                UploadProgress::Upload(
                                    self.uploaded.unwrap_or(0),
                                    started_at.elapsed().unwrap(),
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn parse_comma_separated_quoted_kv_str(input: &str) -> Vec<(String, String)> {
    let mut ret = Vec::new();
    let mut input = input;

    while let Some((key, remaining)) = input.split_once('=') {
        // expecting double quoted value, assume there can be no escape character
        if let Some(remaining) = remaining.strip_prefix('\"') {
            if let Some((value, remaining)) = remaining.split_once('\"') {
                ret.push((key.to_string(), value.to_string()));
                match remaining.strip_prefix(',') {
                    None => break,
                    Some(remaining) => input = remaining,
                }
            }
        }
    }
    ret
}

#[derive(Clone, Debug)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

impl BasicAuth {
    pub fn new(username: String, password: String) -> BasicAuth {
        BasicAuth { username, password }
    }
}

#[derive(Clone, Debug)]
pub struct Registry {
    pub(crate) client: Client,
    pub base_url: String,
    pub(crate) upload_chunk_size: usize,
    pub basic_auth: Option<BasicAuth>,
}

impl Registry {
    pub fn new(base_url: String, basic_auth: Option<BasicAuth>) -> Registry {
        let client = ClientBuilder::new()
            //.redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Registry {
            client,
            base_url,
            basic_auth,
            upload_chunk_size: 2 * 1024 * 1024,
        }
    }

    pub fn new_session(&self, repository: String) -> Session {
        Session {
            registry: self.clone(),
            repository,
            bearer_token: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Session {
    registry: Registry,
    repository: String,
    bearer_token: Option<crate::models::DockerAuthToken>,
}

impl Session {
    fn extract_next_location(&self, response: &Response) -> Result<String, ClientError> {
        let headers = response.headers();
        match headers.get("Location") {
            None => Err(ClientError::MissingHeader("Location".to_string())),
            Some(value) => {
                if value.to_str()?.starts_with('/') {
                    Ok(format!("{}{}", &self.registry.base_url, value.to_str()?))
                } else {
                    Ok(value.to_str()?.to_string())
                }
            }
        }
    }

    async fn try_authenticate(&mut self, www_auth: impl AsRef<str>) -> Result<(), ClientError> {
        let fields: std::collections::HashMap<String, String> =
            std::collections::HashMap::from_iter(parse_comma_separated_quoted_kv_str(
                www_auth.as_ref(),
            ));
        if let Some(realm) = fields.get("Bearer realm") {
            let scope = fields.get("scope").unwrap();
            let service = fields.get("service").unwrap();
            let mut request = self
                .registry
                .client
                .get(format!("{realm}?service={service}&scope={scope}"));
            if let Some(basic_auth) = self.registry.basic_auth.clone() {
                request = request.basic_auth(basic_auth.username, Some(basic_auth.password));
            }
            debug!("try_authenticate request: {request:#?}");
            let response = request.send().await?;
            debug!("try_authenticate response: {response:#?}");
            let auth_token: crate::models::DockerAuthToken = response.json().await?;
            debug!("try_authenticate auth_token: {auth_token:#?}");
            if auth_token.token().is_none() {
                return Err(ClientError::MissingBearerToken);
            } else {
                debug!("try_authenticate setting bearer token: {auth_token:#?}");
                self.bearer_token = Some(auth_token);
            }
        }
        Ok(())
    }

    pub async fn request(&self, request: RequestBuilder) -> Result<Response, ClientError> {
        let mut req = request;
        if let Some(bearer) = self.bearer_token.clone() {
            req = req.bearer_auth(bearer.token().unwrap());
        }
        Ok(req.send().await?)
    }

    pub async fn request_with_try_auth(
        &mut self,
        request: RequestBuilder,
    ) -> Result<Response, ClientError> {
        let cloned = match request.try_clone() {
            Some(clone) => clone,
            None => return Err(ClientError::InvalidRequest(request)),
        };
        let response = self.request(request).await?;
        if response.status().as_u16() == 401 {
            if let Some(www_auth) = response.headers().get("www-authenticate") {
                debug!("www-auth: {www_auth:#?}");
                self.try_authenticate(www_auth.to_str()?).await?;
                Ok(self.request(cloned).await?)
            } else {
                Err(ClientError::MissingHeader("www-authenticate".to_string()))
            }
        } else {
            Ok(response)
        }
    }

    pub async fn exists_digest(&mut self, digest: &OciDigest) -> Result<bool, ClientError> {
        let repository = &self.repository;
        let base_url = self.registry.base_url.to_string();
        let res = self
            .request_with_try_auth(
                self.registry
                    .client
                    .head(format!("{base_url}/v2/{repository}/blobs/{digest}")),
            )
            .await?;
        if res.status().as_u16() == 404 {
            Ok(false)
        } else if res.status().is_success() {
            Ok(true)
        } else {
            Err(ClientError::UnsuccessfulResponse(res))
        }
    }

    pub async fn upload_content(
        &mut self,
        progress: Option<Sender<UploadStat>>,
        media_type: String,
        mut reader: impl Read,
    ) -> Result<Descriptor, ClientError> {
        let mut hasher = Hasher::sha256();
        let repository = &self.repository;
        let base_url = self.registry.base_url.to_string();
        let init_res = self
            .request_with_try_auth(
                self.registry
                    .client
                    .post(format!("{base_url}/v2/{repository}/blobs/uploads/")),
            )
            .await?;

        if !init_res.status().is_success() {
            return Err(ClientError::UnsuccessfulResponse(init_res));
        }

        let mut cursor = 0;
        let mut buffer = vec![0u8; self.registry.upload_chunk_size];
        let mut next_location = self.extract_next_location(&init_res)?;
        if let Some(progress) = progress.as_ref() {
            progress.send_modify(|p| {
                p.started_at = Some(SystemTime::now());
            });
        }
        loop {
            let bytes = reader.read(&mut buffer)?;
            let range = format!("{cursor}-{}", cursor + bytes - 1);

            debug!("uploading range: {range}");

            let owned_buf = buffer[..bytes].to_vec();
            hasher.update(&owned_buf);
            let request = self
                .registry
                .client
                .patch(&next_location)
                .header("Range", &range)
                .header("content-type", "application/octet-stream")
                .body(owned_buf);
            let response = self.request(request).await?;

            if !response.status().is_success() {
                return Err(ClientError::UnsuccessfulResponse(response));
            }

            next_location = self.extract_next_location(&response)?;
            cursor += bytes;
            if let Some(progress) = progress.as_ref() {
                progress.send_modify(|p| {
                    p.uploaded = Some(cursor);
                });
            }
            if bytes < self.registry.upload_chunk_size {
                break;
            }
        }

        let digest = hasher.finalize();
        let request = self
            .registry
            .client
            .put(format!("{next_location}&digest={digest}"));

        let response = self.request(request).await?;

        if response.status().is_success() {
            Ok(Descriptor {
                media_type,
                digest,
                size: cursor,
            })
        } else {
            Err(ClientError::UnsuccessfulResponse(response))
        }
    }

    pub async fn upload_content_known_digest(
        &mut self,
        progress: Option<Sender<UploadStat>>,
        digest: OciDigest,
        media_type: String,
        mut reader: impl Read,
    ) -> Result<Descriptor, ClientError> {
        let base_url = self.registry.base_url.to_string();
        let repository = &self.repository;
        let init_res = self
            .request_with_try_auth(
                self.registry
                    .client
                    .post(format!("{base_url}/v2/{repository}/blobs/uploads/")),
            )
            .await?;

        if !init_res.status().is_success() {
            return Err(ClientError::UnsuccessfulResponse(init_res));
        }

        let mut cursor = 0;
        let mut buffer = vec![0u8; self.registry.upload_chunk_size];
        let mut next_location = self.extract_next_location(&init_res)?;

        if let Some(progress) = progress.as_ref() {
            progress.send_modify(|p| {
                p.started_at = Some(SystemTime::now());
            });
        }

        loop {
            let bytes = reader.read(&mut buffer)?;
            let range = format!("{cursor}-{}", cursor + bytes - 1);
            debug!("uploading range: {range}");

            let owned_buf = buffer[..bytes].to_vec();
            let request = self
                .registry
                .client
                .patch(&next_location)
                .header("Range", &range)
                .header("content-type", "application/octet-stream")
                .body(owned_buf);
            let response = self.request(request).await?;

            if !response.status().is_success() {
                return Err(ClientError::UnsuccessfulResponse(response));
            }

            next_location = self.extract_next_location(&response)?;
            cursor += bytes;
            if let Some(progress) = progress.as_ref() {
                progress.send_modify(|p| {
                    p.uploaded = Some(cursor);
                });
            }
            if bytes < self.registry.upload_chunk_size {
                break;
            }
        }

        let request = self
            .registry
            .client
            .put(format!("{next_location}&digest={digest}"));

        let response = self.request(request).await?;

        if response.status().is_success() {
            Ok(Descriptor {
                media_type,
                digest,
                size: cursor,
            })
        } else {
            Err(ClientError::UnsuccessfulResponse(response))
        }
    }

    pub async fn query_manifest(
        &mut self,
        reference: &str,
    ) -> Result<Option<ManifestVariant>, ClientError> {
        let base_url = &self.registry.base_url;
        let repository = &self.repository;

        let accept = [
            OCI_IMAGE_INDEX,
            OCI_MANIFEST,
            OCI_ARTIFACT,
            DOCKER_MANIFEST,
            DOCKER_MANIFESTS,
        ]
        .join(",");
        let request = self
            .registry
            .client
            .get(format!("{base_url}/v2/{repository}/manifests/{reference}"))
            .header("accept", accept);
        let response = self.request_with_try_auth(request).await?;
        if !response.status().is_success() {
            if response.status().as_u16() == 404 {
                Ok(None)
            } else {
                Err(ClientError::UnsuccessfulResponse(response))
            }
        } else {
            match response.headers().get("content-type") {
                None => Err(ClientError::MissingHeader("content-type".to_string())),
                Some(content_type) => {
                    let content_type = content_type.to_str()?;
                    //                    eprintln!("content-type: {content_type}");
                    match content_type {
                        OCI_MANIFEST | DOCKER_MANIFEST => Ok(Some(ManifestVariant::Manifest(
                            serde_json::from_slice(&response.bytes().await?)?,
                        ))),
                        OCI_IMAGE_INDEX | DOCKER_MANIFESTS => Ok(Some(ManifestVariant::List(
                            serde_json::from_slice(&response.bytes().await?)?,
                        ))),
                        OCI_ARTIFACT => Ok(Some(ManifestVariant::Artifact(
                            serde_json::from_slice(&response.bytes().await?)?,
                        ))),
                        tp => Err(ClientError::UnsupportedContentType(tp.to_string())),
                    }
                }
            }
        }
    }

    /// Query for image manifest, and recursively query the manifest filtered by `filter` until
    /// encountered a non manifest index manifest.
    ///
    pub async fn query_manifest_traced(
        &mut self,
        reference: &str,
        filter: impl Fn(&ImageManifestList) -> Option<ManifestDesc>,
    ) -> Result<Option<ImageManifest>, ClientError> {
        let mut manifest = self.query_manifest(reference).await?;
        while let Some(ManifestVariant::List(manifests)) = manifest {
            match filter(&manifests) {
                None => return Ok(None),
                Some(desc) => {
                    manifest = self.query_manifest(desc.digest.as_str()).await?;
                }
            }
        }
        Ok(manifest.and_then(|manifest| match manifest {
            ManifestVariant::Manifest(manifest) => Some(manifest),
            _ => None,
        }))
    }

    pub async fn merge_manifest_list(
        &mut self,
        descriptor: &Descriptor,
        platform: &Platform,
        tag: &str,
    ) -> Result<Descriptor, ClientError> {
        // see if there an existing manifest with such tag
        //
        // if it is a manifest list, check if there's one matching the platform, if so, replace it
        // and re-upload the list
        //
        // if it is a single manifest, and the platform is different, create a manifest list
        //
        // if there is none, just register this manifest
        let manifest_variant = self.query_manifest(tag).await?;
        let manifest_list = match manifest_variant {
            Some(ManifestVariant::List(mut manifest_list)) => {
                let mut manifests = manifest_list
                    .manifests
                    .into_iter()
                    .filter(|m| !m.platform.is_compatible(platform))
                    .collect::<Vec<_>>();
                let desc = ManifestDesc {
                    media_type: descriptor.media_type.to_string(),
                    size: descriptor.size,
                    platform: platform.clone(),
                    digest: descriptor.digest.clone(),
                    annotations: std::collections::HashMap::new(),
                    artifact_type: None,
                };
                manifests.push(desc);
                manifest_list.manifests = manifests;
                manifest_list
            }
            Some(ManifestVariant::Manifest(manifest)) => {
                let manifest_vec = serde_json::to_vec(&manifest).unwrap();
                let digest = crate::digest::sha256_once(&manifest_vec);

                let mut list = vec![ManifestDesc {
                    media_type: descriptor.media_type.to_string(),
                    size: descriptor.size,
                    platform: platform.clone(),
                    digest: descriptor.digest.clone(),
                    annotations: std::collections::HashMap::new(),
                    artifact_type: None,
                }];

                if let Some(config) = self
                    .fetch_blob_as::<AnyOciConfig>(&manifest.config.digest)
                    .await?
                {
                    let platform = Platform {
                        os: config.os,
                        architecture: config.architecture,
                        os_version: None,
                        os_features: Vec::new(),
                        variant: None,
                        features: Vec::new(),
                    };
                    list.push(ManifestDesc {
                        media_type: manifest.media_type,
                        size: manifest_vec.len(),
                        digest,
                        platform,
                        artifact_type: None,
                        annotations: std::collections::HashMap::new(),
                    });
                }
                ImageManifestList {
                    schema_version: 2,
                    media_type: OCI_IMAGE_INDEX.to_string(),
                    manifests: list,
                }
            }
            _ => ImageManifestList {
                schema_version: 2,
                media_type: OCI_IMAGE_INDEX.to_string(),
                manifests: vec![ManifestDesc {
                    media_type: descriptor.media_type.to_string(),
                    size: descriptor.size,
                    platform: platform.clone(),
                    digest: descriptor.digest.clone(),
                    annotations: std::collections::HashMap::new(),
                    artifact_type: None,
                }],
            },
        };
        self.register_manifest_list(tag, &manifest_list).await
    }

    pub async fn push_manifest_with_tags(
        &mut self,
        tag: &str,
        manifest: &ImageManifest,
        platform: &Platform,
        other_tags: &[impl AsRef<str>],
    ) -> Result<(), ClientError> {
        info!("registering manifest as {tag}");
        let descriptor = self.register_manifest(tag, manifest).await?;
        info!("registered manifest {tag}");
        for tag in other_tags {
            self.merge_manifest_list(&descriptor, platform, tag.as_ref())
                .await?;
        }
        Ok(())
    }

    pub async fn register_manifest_list(
        &mut self,
        tag: &str,
        manifest: &ImageManifestList,
    ) -> Result<Descriptor, ClientError> {
        let repository = &self.repository;
        let base_url = &self.registry.base_url;
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let size = bytes.len();
        let digest = crate::digest::sha256_once(&bytes);
        let request = self
            .registry
            .client
            .put(format!("{base_url}/v2/{repository}/manifests/{tag}"))
            .header("content-type", &manifest.media_type)
            .body(bytes);
        let response = self.request_with_try_auth(request).await?;
        if !response.status().is_success() {
            Err(ClientError::UnsuccessfulResponse(response))
        } else {
            Ok(Descriptor {
                media_type: manifest.media_type.to_string(),
                size,
                digest,
            })
        }
    }
    pub async fn register_manifest(
        &mut self,
        tag: &str,
        manifest: &ImageManifest,
    ) -> Result<Descriptor, ClientError> {
        let repository = &self.repository;
        let base_url = &self.registry.base_url;
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let size = bytes.len();
        let digest = crate::digest::sha256_once(&bytes);
        let request = self
            .registry
            .client
            .put(format!("{base_url}/v2/{repository}/manifests/{tag}"))
            .header("content-type", &manifest.media_type)
            .body(bytes);
        let response = self.request_with_try_auth(request).await?;
        if !response.status().is_success() {
            Err(ClientError::UnsuccessfulResponse(response))
        } else {
            Ok(Descriptor {
                media_type: manifest.media_type.to_string(),
                size,
                digest,
            })
        }
    }

    /// Download blob from the repository to path at `path`
    ///
    /// # Arguments
    ///
    /// * `digest` - The digest of the blob
    /// * `path` - The destination file path
    /// * `replace_on_exists` - if `true`, replace the file at path if it existed, otherwise fail
    pub async fn download_blob(
        &mut self,
        digest: &OciDigest,
        path: impl AsRef<Path>,
        replace_on_exists: bool,
    ) -> Result<(), ClientError> {
        let repository = &self.repository;
        let mut hasher = Hasher::new(digest.algorithm());
        let base_url = &self.registry.base_url;
        let request = self
            .registry
            .client
            .get(format!("{base_url}/v2/{repository}/blobs/{digest}"));
        let mut response = self.request_with_try_auth(request).await?;

        let mut file = std::fs::OpenOptions::new()
            .create_new(replace_on_exists)
            .write(true)
            .open(path)?;

        while let Some(bytes) = response.chunk().await? {
            hasher.update(&bytes);
            file.write_all(&bytes)?;
        }
        let sum = hasher.finalize();
        if &sum != digest {
            Err(ClientError::DigestMismatched(digest.clone(), sum))
        } else {
            Ok(())
        }
    }

    pub async fn fetch_blob_as<T: DeserializeOwned>(
        &mut self,
        digest: &OciDigest,
    ) -> Result<Option<T>, ClientError> {
        let repository = &self.repository;
        let mut hasher = Hasher::new(digest.algorithm());
        let base_url = &self.registry.base_url;
        let request = self
            .registry
            .client
            .get(format!("{base_url}/v2/{repository}/blobs/{digest}"));
        let mut response = self.request_with_try_auth(request).await?;
        if !response.status().is_success() {
            if response.status().as_u16() == 404 {
                Ok(None)
            } else {
                Err(ClientError::UnsuccessfulResponse(response))
            }
        } else {
            let mut buf = Vec::new();
            while let Some(bytes) = response.chunk().await? {
                buf.extend_from_slice(&bytes);
                hasher.update(&bytes);
            }
            let sum = hasher.finalize();
            if &sum != digest {
                Err(ClientError::DigestMismatched(digest.clone(), sum))
            } else {
                Ok(Some(serde_json::from_slice(&buf)?))
            }
        }
    }

    pub async fn fetch_blob(&mut self, digest: &OciDigest) -> Result<Response, ClientError> {
        let repository = &self.repository;
        let base_url = &self.registry.base_url;
        let request = self
            .registry
            .client
            .get(format!("{base_url}/v2/{repository}/blobs/{digest}"));
        self.request_with_try_auth(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_comma_separated_quoted_string_mulitple_items() {
        let input = "123=\"456\",789=\"abc\"";
        let vec = parse_comma_separated_quoted_kv_str(input);
        assert_eq!(
            vec,
            vec![
                ("123".to_string(), "456".to_string()),
                ("789".to_string(), "abc".to_string())
            ]
        );
    }

    #[test]
    fn test_parse_comma_separated_quoted_string_single_item_() {
        let input = "Basic realm=\"hello world\"";
        let vec = parse_comma_separated_quoted_kv_str(input);
        assert_eq!(
            vec,
            vec![("Basic realm".to_string(), "hello world".to_string())]
        );
    }

    #[test]
    fn test_parse_comma_separated_quoted_string_no_items() {
        let input = "";
        let vec = parse_comma_separated_quoted_kv_str(input);
        assert_eq!(vec, vec![]);
    }

    #[test]
    fn test_parse_comma_separated_quoted_string_non_kv_pair() {
        let input = "123";
        let vec = parse_comma_separated_quoted_kv_str(input);
        assert_eq!(vec, vec![]);
    }
}
