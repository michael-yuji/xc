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

use super::ImageManager;
use super::SharedContext;
use crate::task::*;

use oci_util::digest::OciDigest;
use oci_util::distribution::client::*;
use oci_util::image_reference::ImageReference;
use oci_util::models::Descriptor;
use serde::{Serialize, Deserialize};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::sync::watch::Receiver;
use tracing::{debug, info};

#[derive(Error, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PushImageError {
    #[error("no such local reference")]
    NoSuchLocalReference,
    #[error("requested registry not found")]
    RegistryNotFound,
}
impl FromId<SharedContext, String> for PushImageStatus {
    fn from_id(_context: SharedContext, _k: &String) -> (Self, TaskStatus) {
        let status = PushImageStatus::default();
        (status, TaskStatus::InProgress)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PushImageStatusDesc {
    pub layers: Vec<OciDigest>,
    pub current_upload: Option<usize>,
    pub push_config: bool,
    pub push_manifest: bool,
    pub done: bool,
    pub fault: Option<String>,

    pub bytes: Option<usize>,
    pub duration_secs: Option<u64>,
}

#[derive(Clone, Default)]
pub struct PushImageStatus {
    pub layers: Vec<OciDigest>,
    /// the layer we are pushing, identify by the position of it in the layers stack
    pub current_upload: Option<usize>,
    pub push_config: bool,
    pub push_manifest: bool,
    pub done: bool,
    pub fault: Option<String>,
    pub upload_status: Option<Receiver<UploadStat>>,
}

impl PushImageStatus {
    pub(super) fn to_desc(&self) -> PushImageStatusDesc {
        let (bytes, duration_secs) = match &self.upload_status {
            None => (None, None),
            Some(receiver) => {
                let stat = receiver.borrow();
                if let Some((bytes, elapsed)) = stat.started_at.and_then(|started_at| {
                    stat.uploaded
                        .map(|bytes| (bytes, started_at.elapsed().unwrap().as_secs()))
                }) {
                    (Some(bytes), Some(elapsed))
                } else {
                    (None, None)
                }
            }
        };
        PushImageStatusDesc {
            layers: self.layers.clone(),
            current_upload: self.current_upload,
            push_config: self.push_config,
            push_manifest: self.push_manifest,
            done: self.done,
            fault: self.fault.clone(),
            bytes,
            duration_secs,
        }
    }
}
pub async fn push_image(
    this: Arc<RwLock<ImageManager>>,
    layers_dir: &str,
    reference: ImageReference,
    remote_reference: ImageReference,
) -> Result<Receiver<Task<String, PushImageStatus>>, PushImageError> {
    let id = format!("{reference}->{remote_reference}");
    let name = remote_reference.name;
    let tag = remote_reference.tag.to_string();

    let (registry, record) = {
        let this = this.clone();
        let this = this.read().await;
        let reg = this.context.registries.lock().await;

        let record = this
            .query_manifest(&reference.name, reference.tag.as_ref())
            .await
            .map_err(|_| PushImageError::NoSuchLocalReference)?;

        let registry = remote_reference
            .hostname
            .and_then(|registry| reg.get_registry_by_name(&registry))
            .ok_or(PushImageError::RegistryNotFound)?;

        (registry, record)
    };

    let maybe_task = { this.clone().write().await.push_image.get(&id) };
    if let Some(task_receiver) = maybe_task {
        if !(*task_receiver.borrow()).has_failed() {
            return Ok(task_receiver);
        }
    }

    let (mut emitter, rx) = { this.clone().write().await.push_image.register(&id) };
    let name = name.to_string();
    let tag = tag.to_string();
    let layers_dir = layers_dir.to_string();

    tokio::spawn(async move {
        let this = this.clone();
        let mut session = registry.new_session(name.to_string());
        let layers = record.manifest.layers().clone();
        let mut selections = Vec::new();
        //            let (tx, upload_status) = tokio::sync::watch::channel(UploadStat::default());

        'layer_loop: for layer in layers.iter() {
            let maps = {
                let this = this.clone();
                let this = this.read().await;
                this.query_archives(layer).await?
            };

            for map in maps.iter() {
                let layer_file = format!("{}/{}", layers_dir, map.archive_digest);
                let layer_file_path = std::path::Path::new(&layer_file);
                if layer_file_path.exists() {
                    selections.push(map.clone());
                    continue 'layer_loop;
                }
            }

            // XXX: we actually can recover by generating those layers, just not in this version
            {
                tracing::error!("cannot find archive layer for {layer}");
                emitter.set_faulted(&format!("cannot find archive layer for {layer}"));
                anyhow::bail!("cannot find archive layer for {layer}");
            }

            break;
        }

        _ = emitter.use_try(|state| {
            state.layers = layers.clone();
            Ok(())
        });

        if layers.len() != selections.len() {
            todo!()
        }

        let mut uploads = Vec::new();
        for map in selections.iter() {
            let content_type = match map.algorithm.as_str() {
                "gzip" => "application/vnd.oci.image.layer.v1.tar+gzip",
                "zstd" => "application/vnd.oci.image.layer.v1.tar+zstd",
                "plain" => "application/vnd.oci.image.layer.v1.tar",
                _ => unreachable!(),
            };

            let path = format!("{layers_dir}/{}", map.archive_digest);
            let path = std::path::Path::new(&path);
            let file = std::fs::OpenOptions::new().read(true).open(path)?;

            //                let dedup_check = Ok::<bool, std::io::Error>(false);
            let dedup_check = session.exists_digest(&map.archive_digest).await;

            _ = emitter.use_try(|state| {
                if state.current_upload.is_none() {
                    state.current_upload = Some(0);
                }
                Ok(())
            });
            let descriptor = if dedup_check.is_ok() && dedup_check.unwrap() {
                let metadata = file.metadata().unwrap();
                _ = emitter.use_try(|state| {
                    if state.current_upload.is_none() {
                        state.current_upload = Some(0);
                    }
                    Ok(())
                });

                Descriptor {
                    digest: map.archive_digest.clone(),
                    media_type: content_type.to_string(),
                    size: metadata.len() as usize,
                }
            } else {
                info!("pushing {path:?}");
                let (tx, upload_status) = tokio::sync::watch::channel(UploadStat::default());
                _ = emitter.use_try(|state| {
                    state.upload_status = Some(upload_status.clone());
                    Ok(())
                });

                let maybe_descriptor = session
                    .upload_content(Some(tx), content_type.to_string(), file)
                    .await;
                maybe_descriptor?
            };
            uploads.push(descriptor);
            _ = emitter.use_try(|state| {
                state.current_upload = Some(state.current_upload.unwrap() + 1);
                Ok(())
            });
        }
        // upload config
        let config = serde_json::to_vec(&record.manifest)?;
        _ = emitter.use_try(|state| {
            state.push_config = true;
            Ok(())
        });
        debug!("pushing config");
        let config_descriptor = session
            .upload_content(
                None,
                "application/vnd.oci.image.config.v1+json".to_string(),
                config.as_slice(),
            )
            .await?;
        let manifest = oci_util::models::ImageManifest {
            schema_version: 2,
            media_type: "application/vnd.oci.image.manifest.v1+json".to_owned(),
            config: config_descriptor, //,config.unwrap(),
            layers: uploads,
        };

        debug!("Registering manifest: {manifest:#?}");
        _ = emitter.use_try(|state| {
            state.push_manifest = true;
            Ok(())
        });
        session.register_manifest(&tag, &manifest).await?;

        debug!("Registering manifest: {manifest:#?}");
        _ = emitter.use_try(|state| {
            state.done = true;
            Ok(())
        });
        emitter.set_completed();

        Ok::<(), anyhow::Error>(())
    });

    Ok(rx)
}

