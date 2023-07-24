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
pub mod sqlite;

use crate::models::jail_image::JailImage;
use oci_util::digest::OciDigest;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ImageRecord {
    pub name: String,
    pub tag: String,
    pub digest: String,
    pub manifest: JailImage,
}

#[derive(Clone, Debug)]
pub struct DiffIdMap {
    pub diff_id: OciDigest,
    pub archive_digest: OciDigest,
    pub algorithm: String,
    pub origin: Option<String>,
}

pub trait ImageStore {
    /// Delete any registered manifests, if exists, from this image store, along
    /// with any tags referencing to the manifest
    /// # Parameters
    /// * `digest`: digest of the manifest to be removed
    fn delete_manifest(&self, digest: &OciDigest) -> Result<(), ImageStoreError>;

    /// dereference a name:tag from the manifest it is referencing to
    fn untag(&self, name: &str, tag: &str) -> Result<(), ImageStoreError>;

    fn list_all_tagged(&self) -> Result<Vec<ImageRecord>, ImageStoreError>;

    fn list_all_tags(&self, name: &str) -> Result<Vec<ImageRecord>, ImageStoreError>;

    fn list_all_manifests(&self) -> Result<HashMap<OciDigest, JailImage>, ImageStoreError>;

    fn register_manifest(&self, manifest: &JailImage) -> Result<OciDigest, ImageStoreError>;

    fn purge_all_untagged_manifest(&self) -> Result<(), ImageStoreError>;

    fn tag_manifest(
        &self,
        manifest: &OciDigest,
        name: &str,
        tag: &str,
    ) -> Result<(), ImageStoreError>;

    fn register_and_tag_manifest(
        &self,
        name: &str,
        tag: &str,
        manifest: &JailImage,
    ) -> Result<OciDigest, ImageStoreError>;

    fn query_manifest(&self, name: &str, tag: &str) -> Result<ImageRecord, ImageStoreError>;

    fn query_records_using_commit(
        &self,
        commit_id: &str,
    ) -> Result<Vec<ImageRecord>, ImageStoreError>;

    fn associate_commit_manifest(
        &self,
        commit_id: &str,
        manifest: &JailImage,
    ) -> Result<(), ImageStoreError>;

    fn query_diff_id(&self, digest: &OciDigest) -> Result<Option<DiffIdMap>, ImageStoreError>;

    fn query_archives(&self, diff_id: &OciDigest) -> Result<Vec<DiffIdMap>, ImageStoreError>;

    fn map_diff_id(
        &self,
        diff_id: &OciDigest,
        archive: &OciDigest,
        content_type: &str,
        origin: Option<String>,
    ) -> Result<(), ImageStoreError>;
}

#[derive(Error, Debug)]
pub enum ImageStoreError {
    #[error("error from underlying engine: {0}")]
    EngineError(anyhow::Error),
    #[error("requested manifest ({0}) not found")]
    ManifestNotFound(OciDigest),
    #[error("requested tag ({1}) not found in repo {0}")]
    TagNotFound(String, String),
}

impl From<anyhow::Error> for ImageStoreError {
    fn from(value: anyhow::Error) -> ImageStoreError {
        ImageStoreError::EngineError(value)
    }
}
